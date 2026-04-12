use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{info, warn};

use sandcastle_runtime::{
    ExecRequest, ExecResult, Language, Result, SandboxConfig, SandboxId, SandboxRuntime,
    SandcastleError,
};

use crate::config::ManagerConfig;
use crate::session::{Session, SessionStatus};

/// Central manager for sandbox lifecycle, session tracking, and file validation.
pub struct SandboxManager {
    runtime: Arc<dyn SandboxRuntime>,
    sessions: RwLock<HashMap<String, Session>>,
    config: ManagerConfig,
}

impl SandboxManager {
    pub fn new(runtime: Arc<dyn SandboxRuntime>, config: ManagerConfig) -> Self {
        Self {
            runtime,
            sessions: RwLock::new(HashMap::new()),
            config,
        }
    }

    /// One-shot execution: create sandbox, run code, destroy, return result.
    pub async fn execute_oneshot(
        &self,
        code: &str,
        language: Language,
        timeout: std::time::Duration,
    ) -> Result<ExecResult> {
        let config = SandboxConfig {
            language,
            limits: self.config.defaults.clone(),
            env_vars: HashMap::new(),
        };

        let sandbox_id = self.runtime.create(&config).await?;
        self.runtime.start(&sandbox_id).await?;

        let request = ExecRequest {
            code: code.to_string(),
            timeout,
        };

        let result = self.runtime.execute(&sandbox_id, &request).await;

        // Always destroy, even if execute failed
        if let Err(e) = self.runtime.destroy(&sandbox_id).await {
            warn!("failed to destroy oneshot sandbox {sandbox_id}: {e}");
        }

        result
    }

    /// Create a persistent sandbox session.
    pub async fn create_session(&self, config: SandboxConfig) -> Result<String> {
        {
            let sessions = self.sessions.read().await;
            if sessions.len() >= self.config.max_sessions {
                return Err(SandcastleError::MaxSessionsReached(self.config.max_sessions));
            }
        }

        let session_id = format!("sb-{}", uuid::Uuid::new_v4());
        let sandbox_id = self.runtime.create(&config).await?;
        self.runtime.start(&sandbox_id).await?;

        let session = Session::new(sandbox_id, config.language);

        let mut sessions = self.sessions.write().await;
        sessions.insert(session_id.clone(), session);
        info!("created session {session_id}");

        Ok(session_id)
    }

    /// Execute code in an existing session.
    pub async fn execute_in_session(
        &self,
        session_id: &str,
        code: &str,
        timeout: std::time::Duration,
    ) -> Result<ExecResult> {
        let sandbox_id = {
            let mut sessions = self.sessions.write().await;
            let session = sessions
                .get_mut(session_id)
                .ok_or_else(|| SandcastleError::SessionNotFound(session_id.to_string()))?;

            if session.status != SessionStatus::Active {
                return Err(SandcastleError::SessionExpired(session_id.to_string()));
            }

            let idle_time = session.last_active.elapsed();
            if idle_time > self.config.session_timeout() {
                session.status = SessionStatus::Expired;
                return Err(SandcastleError::SessionExpired(session_id.to_string()));
            }

            session.touch();
            session.sandbox_id.clone()
        };

        let request = ExecRequest {
            code: code.to_string(),
            timeout,
        };

        self.runtime.execute(&sandbox_id, &request).await
    }

    /// Upload a file from the host into a sandbox session.
    pub async fn upload(
        &self,
        session_id: &str,
        host_path: &Path,
        sandbox_path: &str,
    ) -> Result<u64> {
        // Validate host path
        let canonical = host_path
            .canonicalize()
            .map_err(|_| SandcastleError::FileNotFound(host_path.to_path_buf()))?;

        let allowed = self
            .config
            .files
            .allowed_input_dirs
            .iter()
            .any(|dir| canonical.starts_with(dir));

        if !allowed {
            return Err(SandcastleError::PathNotAllowed(canonical));
        }

        // Check for path traversal in sandbox_path
        if sandbox_path.contains("..") {
            return Err(SandcastleError::PathTraversal(sandbox_path.to_string()));
        }

        // Check file size
        let metadata = std::fs::metadata(&canonical)
            .map_err(|_| SandcastleError::FileNotFound(canonical.clone()))?;

        if metadata.len() > self.config.files.max_file_size_bytes {
            return Err(SandcastleError::FileTooLarge {
                size: metadata.len(),
                max: self.config.files.max_file_size_bytes,
            });
        }

        let sandbox_id = self.get_active_sandbox_id(session_id).await?;
        let sandbox_dest = Path::new(sandbox_path);

        self.runtime
            .upload_file(&sandbox_id, &canonical, sandbox_dest)
            .await
    }

    /// Download a file from a sandbox session to the host.
    pub async fn download(
        &self,
        session_id: &str,
        sandbox_path: &str,
        host_path: Option<&str>,
    ) -> Result<(PathBuf, u64)> {
        // Check for path traversal in sandbox_path
        if sandbox_path.contains("..") {
            return Err(SandcastleError::PathTraversal(sandbox_path.to_string()));
        }

        let host_dest = match host_path {
            Some(p) => {
                let path = PathBuf::from(p);
                // Validate host_path is within output_dir
                if !path.starts_with(&self.config.files.output_dir) {
                    return Err(SandcastleError::PathNotAllowed(path));
                }
                path
            }
            None => {
                // Default: {output_dir}/{session_id}/{filename}
                let filename = Path::new(sandbox_path)
                    .file_name()
                    .ok_or_else(|| SandcastleError::InvalidParams("invalid sandbox path".into()))?;
                self.config
                    .files
                    .output_dir
                    .join(session_id)
                    .join(filename)
            }
        };

        // Ensure parent directory exists
        if let Some(parent) = host_dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                SandcastleError::RuntimeError(format!("failed to create output dir: {e}"))
            })?;
        }

        let sandbox_id = self.get_active_sandbox_id(session_id).await?;
        let sandbox_src = Path::new(sandbox_path);

        let bytes = self
            .runtime
            .download_file(&sandbox_id, sandbox_src, &host_dest)
            .await?;

        // Validate downloaded file size
        if bytes > self.config.files.max_file_size_bytes {
            let _ = std::fs::remove_file(&host_dest);
            return Err(SandcastleError::FileTooLarge {
                size: bytes,
                max: self.config.files.max_file_size_bytes,
            });
        }

        Ok((host_dest, bytes))
    }

    /// Destroy a sandbox session.
    pub async fn destroy_session(&self, session_id: &str) -> Result<()> {
        let session = {
            let mut sessions = self.sessions.write().await;
            sessions
                .remove(session_id)
                .ok_or_else(|| SandcastleError::SessionNotFound(session_id.to_string()))?
        };

        self.runtime.stop(&session.sandbox_id).await?;
        self.runtime.destroy(&session.sandbox_id).await?;
        info!("destroyed session {session_id}");
        Ok(())
    }

    /// Reap expired sessions. Called periodically by background task.
    pub async fn reap_expired(&self) {
        let timeout = self.config.session_timeout();

        // Collect expired session IDs under read lock
        let expired: Vec<(String, SandboxId)> = {
            let sessions = self.sessions.read().await;
            sessions
                .iter()
                .filter(|(_, s)| s.last_active.elapsed() > timeout)
                .map(|(id, s)| (id.clone(), s.sandbox_id.clone()))
                .collect()
        };

        if expired.is_empty() {
            return;
        }

        // Remove from map under write lock
        {
            let mut sessions = self.sessions.write().await;
            for (id, _) in &expired {
                sessions.remove(id);
            }
        }

        // Destroy outside the lock
        for (session_id, sandbox_id) in expired {
            info!("reaping expired session {session_id}");
            if let Err(e) = self.runtime.stop(&sandbox_id).await {
                warn!("failed to stop expired sandbox {sandbox_id}: {e}");
            }
            if let Err(e) = self.runtime.destroy(&sandbox_id).await {
                warn!("failed to destroy expired sandbox {sandbox_id}: {e}");
            }
        }
    }

    /// List active sessions (for debugging/monitoring).
    pub async fn list_sessions(&self) -> Vec<String> {
        let sessions = self.sessions.read().await;
        sessions.keys().cloned().collect()
    }

    /// Helper: get sandbox_id for an active session.
    async fn get_active_sandbox_id(&self, session_id: &str) -> Result<SandboxId> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| SandcastleError::SessionNotFound(session_id.to_string()))?;

        if session.status != SessionStatus::Active {
            return Err(SandcastleError::SessionExpired(session_id.to_string()));
        }

        let idle_time = session.last_active.elapsed();
        if idle_time > self.config.session_timeout() {
            session.status = SessionStatus::Expired;
            return Err(SandcastleError::SessionExpired(session_id.to_string()));
        }

        session.touch();
        Ok(session.sandbox_id.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use sandcastle_runtime::{SandboxRuntime, SandboxStatus};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    /// Mock runtime for testing the manager.
    struct MockRuntime {
        counter: AtomicU32,
    }

    impl MockRuntime {
        fn new() -> Self {
            Self {
                counter: AtomicU32::new(0),
            }
        }
    }

    #[async_trait]
    impl SandboxRuntime for MockRuntime {
        async fn create(&self, _config: &SandboxConfig) -> Result<SandboxId> {
            let n = self.counter.fetch_add(1, Ordering::SeqCst);
            Ok(SandboxId(format!("mock-{n}")))
        }

        async fn start(&self, _id: &SandboxId) -> Result<()> {
            Ok(())
        }

        async fn execute(&self, _id: &SandboxId, request: &ExecRequest) -> Result<ExecResult> {
            Ok(ExecResult {
                stdout: format!("mock: {}", request.code),
                stderr: String::new(),
                exit_code: 0,
                execution_time_ms: 1,
                timed_out: false,
                oom_killed: false,
            })
        }

        async fn stop(&self, _id: &SandboxId) -> Result<()> {
            Ok(())
        }

        async fn destroy(&self, _id: &SandboxId) -> Result<()> {
            Ok(())
        }

        async fn upload_file(
            &self,
            _id: &SandboxId,
            _host: &Path,
            _sandbox: &Path,
        ) -> Result<u64> {
            Ok(0)
        }

        async fn download_file(
            &self,
            _id: &SandboxId,
            _sandbox: &Path,
            _host: &Path,
        ) -> Result<u64> {
            Ok(0)
        }

        async fn status(&self, _id: &SandboxId) -> Result<SandboxStatus> {
            Ok(SandboxStatus::Running)
        }
    }

    fn test_config() -> ManagerConfig {
        ManagerConfig {
            max_sessions: 3,
            session_timeout_seconds: 300,
            defaults: sandcastle_runtime::ResourceLimits::default(),
            files: crate::FileConfig {
                allowed_input_dirs: vec!["/tmp".into()],
                output_dir: "/tmp/sandcastle-test-out".into(),
                max_file_size_bytes: 1_048_576,
            },
        }
    }

    #[tokio::test]
    async fn test_execute_oneshot() {
        let runtime = Arc::new(MockRuntime::new());
        let manager = SandboxManager::new(runtime, test_config());

        let result = manager
            .execute_oneshot("print('hi')", Language::Python, Duration::from_secs(5))
            .await
            .unwrap();

        assert_eq!(result.stdout, "mock: print('hi')");
        assert_eq!(result.exit_code, 0);
    }

    #[tokio::test]
    async fn test_create_and_execute_session() {
        let runtime = Arc::new(MockRuntime::new());
        let manager = SandboxManager::new(runtime, test_config());

        let config = SandboxConfig {
            language: Language::Python,
            limits: sandcastle_runtime::ResourceLimits::default(),
            env_vars: HashMap::new(),
        };

        let session_id = manager.create_session(config).await.unwrap();
        assert!(session_id.starts_with("sb-"));

        let result = manager
            .execute_in_session(&session_id, "x = 1", Duration::from_secs(5))
            .await
            .unwrap();

        assert_eq!(result.stdout, "mock: x = 1");

        // Verify session is listed
        let sessions = manager.list_sessions().await;
        assert!(sessions.contains(&session_id));

        // Destroy
        manager.destroy_session(&session_id).await.unwrap();
        let sessions = manager.list_sessions().await;
        assert!(!sessions.contains(&session_id));
    }

    #[tokio::test]
    async fn test_max_sessions_limit() {
        let runtime = Arc::new(MockRuntime::new());
        let manager = SandboxManager::new(runtime, test_config());

        let config = SandboxConfig {
            language: Language::Bash,
            limits: sandcastle_runtime::ResourceLimits::default(),
            env_vars: HashMap::new(),
        };

        // Create max sessions (limit is 3)
        for _ in 0..3 {
            manager.create_session(config.clone()).await.unwrap();
        }

        // The 4th should fail
        let err = manager.create_session(config.clone()).await.unwrap_err();
        assert!(matches!(err, SandcastleError::MaxSessionsReached(3)));
    }

    #[tokio::test]
    async fn test_session_not_found() {
        let runtime = Arc::new(MockRuntime::new());
        let manager = SandboxManager::new(runtime, test_config());

        let err = manager
            .execute_in_session("nonexistent", "code", Duration::from_secs(5))
            .await
            .unwrap_err();

        assert!(matches!(err, SandcastleError::SessionNotFound(_)));
    }

    #[tokio::test]
    async fn test_destroy_nonexistent() {
        let runtime = Arc::new(MockRuntime::new());
        let manager = SandboxManager::new(runtime, test_config());

        let err = manager.destroy_session("nonexistent").await.unwrap_err();
        assert!(matches!(err, SandcastleError::SessionNotFound(_)));
    }
}
