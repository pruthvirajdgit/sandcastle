use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Child;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use sandcastle_runtime::{
    ExecRequest, ExecResult, Language, Result, SandboxConfig, SandboxId, SandboxRuntime,
    SandboxStatus, SandcastleError,
};

use crate::config::GvisorConfig;

/// State held for each active gVisor container.
struct ContainerHandle {
    /// The runsc child process.
    child: Child,
    /// Language for this sandbox.
    language: Language,
    /// Path to the bundle directory.
    bundle_dir: PathBuf,
    /// Path to the workspace directory on the host.
    workspace_dir: PathBuf,
    /// Container ID for runsc commands.
    container_id: String,
    /// Mutex to serialize execute calls on this container.
    exec_lock: Arc<Mutex<()>>,
}

/// gVisor sandbox using runsc as the OCI runtime.
///
/// Each sandbox is a gVisor container running the executor binary as PID 1.
/// Communication happens via stdin/stdout pipes of the `runsc run` subprocess.
pub struct GvisorSandbox {
    config: GvisorConfig,
    containers: tokio::sync::RwLock<HashMap<String, ContainerHandle>>,
}

impl GvisorSandbox {
    pub fn new(config: GvisorConfig) -> Self {
        Self {
            config,
            containers: tokio::sync::RwLock::new(HashMap::new()),
        }
    }

    /// Ensure all required directories exist.
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.config.state_dir)?;
        std::fs::create_dir_all(&self.config.bundle_dir)?;
        std::fs::create_dir_all(&self.config.workspace_dir)?;
        Ok(())
    }

    /// Check if runsc is available.
    pub fn is_available(&self) -> bool {
        crate::runsc::is_available(&self.config.runsc_path)
    }

    fn lang_dir_name(language: Language) -> &'static str {
        match language {
            Language::Python => "python",
            Language::Javascript => "javascript",
            Language::Bash => "bash",
        }
    }

    /// Prepare the OCI bundle for a container.
    fn prepare_bundle(
        &self,
        id: &str,
        language: Language,
        config: &SandboxConfig,
        workspace_dir: &Path,
    ) -> Result<PathBuf> {
        let bundle_dir = self.config.bundle_dir.join(id);
        std::fs::create_dir_all(&bundle_dir).map_err(|e| {
            SandcastleError::SandboxCreationFailed(format!("create bundle dir: {e}"))
        })?;

        let lang_name = Self::lang_dir_name(language);
        let source_rootfs = self.config.rootfs_dir.join(lang_name);
        if !source_rootfs.exists() {
            return Err(SandcastleError::SandboxCreationFailed(format!(
                "rootfs not found for {lang_name}: {}",
                source_rootfs.display()
            )));
        }

        // Ensure executor binary exists in rootfs
        let executor_dest = source_rootfs.join("sandbox").join("executor");
        if !executor_dest.exists() {
            if let Some(parent) = executor_dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    SandcastleError::SandboxCreationFailed(format!(
                        "create executor dir in rootfs: {e}"
                    ))
                })?;
            }
            std::fs::copy(&self.config.executor_path, &executor_dest).map_err(|e| {
                SandcastleError::SandboxCreationFailed(format!("copy executor binary: {e}"))
            })?;
        }

        // Create workspace dir on host
        std::fs::create_dir_all(workspace_dir).map_err(|e| {
            SandcastleError::SandboxCreationFailed(format!("create workspace dir: {e}"))
        })?;
        // Ensure workspace is writable by container processes
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(workspace_dir, std::fs::Permissions::from_mode(0o777))
                .map_err(|e| {
                    SandcastleError::SandboxCreationFailed(format!(
                        "set workspace permissions: {e}"
                    ))
                })?;
        }

        // Generate OCI config.json
        let spec = crate::oci::generate_spec(
            language,
            &config.limits,
            &source_rootfs,
            workspace_dir,
            "/sandbox/executor",
            &config.env_vars,
        )
        .map_err(SandcastleError::SandboxCreationFailed)?;

        let config_path = bundle_dir.join("config.json");
        let config_json = serde_json::to_string_pretty(&spec).map_err(|e| {
            SandcastleError::SandboxCreationFailed(format!("serialize config.json: {e}"))
        })?;
        std::fs::write(&config_path, config_json).map_err(|e| {
            SandcastleError::SandboxCreationFailed(format!("write config.json: {e}"))
        })?;

        info!("prepared gVisor bundle at {}", bundle_dir.display());
        Ok(bundle_dir)
    }
}

#[async_trait]
impl SandboxRuntime for GvisorSandbox {
    async fn create(&self, config: &SandboxConfig) -> Result<SandboxId> {
        let id = format!("gv-{}", uuid::Uuid::new_v4().simple());
        let workspace_dir = self.config.workspace_dir.join(&id);

        // Prepare bundle (sync filesystem ops)
        let bundle_dir = {
            let gvisor = GvisorSandbox::new(self.config.clone());
            let id = id.clone();
            let language = config.language;
            let sandbox_config = config.clone();
            let ws = workspace_dir.clone();

            tokio::task::spawn_blocking(move || {
                gvisor.prepare_bundle(&id, language, &sandbox_config, &ws)
            })
            .await
            .map_err(|e| SandcastleError::SandboxCreationFailed(format!("spawn blocking: {e}")))?
        }?;

        info!("gVisor sandbox {id} bundle prepared");
        // Store placeholder — actual child is spawned in start()
        let handle = ContainerHandle {
            child: tokio::process::Command::new("true")
                .spawn()
                .map_err(|e| {
                    SandcastleError::SandboxCreationFailed(format!("placeholder spawn: {e}"))
                })?,
            language: config.language,
            bundle_dir,
            workspace_dir,
            container_id: id.clone(),
            exec_lock: Arc::new(Mutex::new(())),
        };

        let mut containers = self.containers.write().await;
        containers.insert(id.clone(), handle);

        Ok(SandboxId(id))
    }

    async fn start(&self, id: &SandboxId) -> Result<()> {
        let mut containers = self.containers.write().await;
        let handle = containers.get_mut(&id.0).ok_or_else(|| {
            SandcastleError::RuntimeError(format!("container {} not found", id.0))
        })?;

        // Spawn `runsc run` — this starts the container and connects stdin/stdout
        let child = crate::runsc::spawn_run(
            &self.config.runsc_path,
            &self.config.state_dir,
            &handle.bundle_dir,
            &handle.container_id,
            &self.config.platform,
        )
        .await
        .map_err(SandcastleError::SandboxCreationFailed)?;

        handle.child = child;

        // Wait briefly for the executor to be ready
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        info!("gVisor sandbox {} started", id.0);
        Ok(())
    }

    async fn execute(&self, id: &SandboxId, request: &ExecRequest) -> Result<ExecResult> {
        let mut containers = self.containers.write().await;
        let handle = containers.get_mut(&id.0).ok_or_else(|| {
            SandcastleError::RuntimeError(format!("container {} not found", id.0))
        })?;

        let _lock = handle.exec_lock.clone();
        let _guard = _lock.lock().await;

        // Build the JSON command for the executor
        let exec_json = serde_json::json!({
            "action": "exec",
            "language": handle.language.to_string(),
            "code": request.code,
            "timeout_ms": request.timeout.as_millis() as u64,
            "max_output_bytes": 1_048_576u64,
        });

        let mut command_line = serde_json::to_string(&exec_json)
            .map_err(|e| SandcastleError::ExecutionFailed(format!("serialize command: {e}")))?;
        command_line.push('\n');

        // Write to stdin
        let stdin = handle.child.stdin.as_mut().ok_or_else(|| {
            SandcastleError::ExecutionFailed("stdin not available".to_string())
        })?;

        stdin
            .write_all(command_line.as_bytes())
            .await
            .map_err(|e| SandcastleError::ExecutionFailed(format!("write to stdin: {e}")))?;
        stdin
            .flush()
            .await
            .map_err(|e| SandcastleError::ExecutionFailed(format!("flush stdin: {e}")))?;

        debug!("sent exec command to gVisor container {}", id.0);

        // Read response from stdout
        let stdout = handle.child.stdout.as_mut().ok_or_else(|| {
            SandcastleError::ExecutionFailed("stdout not available".to_string())
        })?;

        let mut reader = BufReader::new(stdout);
        let mut response_line = String::new();

        // Read with timeout
        let read_result = tokio::time::timeout(
            request.timeout + std::time::Duration::from_secs(5),
            reader.read_line(&mut response_line),
        )
        .await;

        match read_result {
            Ok(Ok(0)) => {
                Err(SandcastleError::ExecutionFailed(
                    "executor closed stdout (container may have crashed)".to_string(),
                ))
            }
            Ok(Ok(_)) => {
                let result: ExecResult = serde_json::from_str(response_line.trim())
                    .map_err(|e| {
                        SandcastleError::ExecutionFailed(format!(
                            "parse executor response: {e}. Raw: {response_line}"
                        ))
                    })?;

                debug!(
                    "gVisor container {} executed in {}ms (exit={})",
                    id.0, result.execution_time_ms, result.exit_code
                );
                Ok(result)
            }
            Ok(Err(e)) => Err(SandcastleError::ExecutionFailed(format!(
                "read stdout: {e}"
            ))),
            Err(_) => Err(SandcastleError::Timeout),
        }
    }

    async fn stop(&self, id: &SandboxId) -> Result<()> {
        // Kill the container via runsc
        crate::runsc::kill(
            &self.config.runsc_path,
            &self.config.state_dir,
            &id.0,
        )
        .await
        .map_err(SandcastleError::RuntimeError)?;

        // Also kill and wait on the child process to avoid zombies
        let mut containers = self.containers.write().await;
        if let Some(handle) = containers.get_mut(&id.0) {
            let _ = handle.child.kill().await;
            let _ = handle.child.wait().await;
        }

        info!("gVisor sandbox {} stopped", id.0);
        Ok(())
    }

    async fn destroy(&self, id: &SandboxId) -> Result<()> {
        // Delete container state via runsc
        crate::runsc::delete(
            &self.config.runsc_path,
            &self.config.state_dir,
            &id.0,
        )
        .await
        .map_err(SandcastleError::RuntimeError)?;

        // Remove from our map and clean up directories
        let handle = {
            let mut containers = self.containers.write().await;
            containers.remove(&id.0)
        };

        if let Some(handle) = handle {
            // Clean up bundle and workspace dirs
            if handle.bundle_dir.exists() {
                if let Err(e) = std::fs::remove_dir_all(&handle.bundle_dir) {
                    warn!("failed to remove bundle dir {}: {e}", handle.bundle_dir.display());
                }
            }
            if handle.workspace_dir.exists() {
                if let Err(e) = std::fs::remove_dir_all(&handle.workspace_dir) {
                    warn!(
                        "failed to remove workspace dir {}: {e}",
                        handle.workspace_dir.display()
                    );
                }
            }
        }

        info!("gVisor sandbox {} destroyed", id.0);
        Ok(())
    }

    async fn upload_file(
        &self,
        id: &SandboxId,
        host_path: &Path,
        sandbox_path: &Path,
    ) -> Result<u64> {
        let containers = self.containers.read().await;
        let handle = containers.get(&id.0).ok_or_else(|| {
            SandcastleError::RuntimeError(format!("container {} not found", id.0))
        })?;

        let dest = handle.workspace_dir.join(sandbox_path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                SandcastleError::RuntimeError(format!("create parent dir: {e}"))
            })?;
        }

        let bytes = std::fs::copy(host_path, &dest).map_err(|e| {
            SandcastleError::RuntimeError(format!("copy file to workspace: {e}"))
        })?;

        debug!(
            "uploaded {} -> workspace/{} ({bytes} bytes) in gVisor container {}",
            host_path.display(),
            sandbox_path.display(),
            id.0
        );
        Ok(bytes)
    }

    async fn download_file(
        &self,
        id: &SandboxId,
        sandbox_path: &Path,
        host_path: &Path,
    ) -> Result<u64> {
        let containers = self.containers.read().await;
        let handle = containers.get(&id.0).ok_or_else(|| {
            SandcastleError::RuntimeError(format!("container {} not found", id.0))
        })?;

        let src = handle.workspace_dir.join(sandbox_path);
        if !src.exists() {
            return Err(SandcastleError::FileNotFound(src));
        }

        if let Some(parent) = host_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                SandcastleError::RuntimeError(format!("create parent dir: {e}"))
            })?;
        }

        let bytes = std::fs::copy(&src, host_path).map_err(|e| {
            SandcastleError::RuntimeError(format!("copy file from workspace: {e}"))
        })?;

        debug!(
            "downloaded workspace/{} -> {} ({bytes} bytes) from gVisor container {}",
            sandbox_path.display(),
            host_path.display(),
            id.0
        );
        Ok(bytes)
    }

    async fn status(&self, id: &SandboxId) -> Result<SandboxStatus> {
        let containers = self.containers.read().await;
        if containers.contains_key(&id.0) {
            Ok(SandboxStatus::Running)
        } else {
            Ok(SandboxStatus::Stopped)
        }
    }
}
