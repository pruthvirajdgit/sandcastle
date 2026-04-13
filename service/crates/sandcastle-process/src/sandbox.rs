use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use libcontainer::container::builder::ContainerBuilder;
use libcontainer::container::Container;
use libcontainer::syscall::syscall::SyscallType;

use sandcastle_runtime::{
    ExecRequest, ExecResult, Language, Result, SandboxConfig, SandboxId, SandboxRuntime,
    SandboxStatus, SandcastleError,
};

use crate::config::ProcessConfig;

/// State held for each active container.
struct ContainerHandle {
    /// Write end of stdin pipe — we send JSON commands here.
    stdin_writer: std::fs::File,
    /// Read end of stdout pipe — we receive JSON responses here.
    stdout_reader: Option<BufReader<std::fs::File>>,
    /// Language for this sandbox.
    language: Language,
    /// Path to the bundle directory.
    bundle_dir: PathBuf,
    /// Path to the workspace directory on the host.
    workspace_dir: PathBuf,
    /// Current status.
    status: SandboxStatus,
    /// Mutex to serialize execute calls on this container.
    exec_lock: Arc<Mutex<()>>,
}

/// Process-level sandbox using Linux namespaces via libcontainer (youki).
///
/// Each sandbox is an OCI container with:
/// - PID, mount, IPC, UTS, network namespace isolation
/// - Resource limits via cgroups v2
/// - The executor binary as the init process
/// - Communication via stdin/stdout pipes
pub struct ProcessSandbox {
    config: ProcessConfig,
    containers: tokio::sync::RwLock<HashMap<String, ContainerHandle>>,
}

impl ProcessSandbox {
    pub fn new(config: ProcessConfig) -> Self {
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

    /// Prepare the bundle directory for a container:
    /// - Create {bundle_dir}/{id}/
    /// - Copy/link rootfs
    /// - Write config.json
    /// - Copy executor binary into rootfs
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

        // Rootfs: copy from the language-specific rootfs directory
        let lang_name = match language {
            Language::Python => "python",
            Language::Javascript => "javascript",
            Language::Bash => "bash",
        };
        let source_rootfs = self.config.rootfs_dir.join(lang_name);
        if !source_rootfs.exists() {
            return Err(SandcastleError::SandboxCreationFailed(format!(
                "rootfs not found for {lang_name}: {}",
                source_rootfs.display()
            )));
        }

        let container_rootfs = bundle_dir.join("rootfs");

        // Create an overlay-style rootfs by bind-mounting or symlinking
        // For simplicity, we symlink to the pre-built rootfs
        // The OCI spec root.path will point to the actual rootfs
        // We don't modify the rootfs — workspace is a separate bind mount

        // Verify executor binary exists in rootfs (baked in by build-rootfs.sh)
        let executor_dest = source_rootfs.join("sandbox").join("executor");
        if !executor_dest.exists() {
            return Err(SandcastleError::SandboxCreationFailed(format!(
                "executor binary not found in rootfs at {}. Run scripts/build-rootfs.sh to bake it in.",
                executor_dest.display()
            )));
        }

        // Symlink rootfs into bundle
        std::os::unix::fs::symlink(&source_rootfs, &container_rootfs).map_err(|e| {
            SandcastleError::SandboxCreationFailed(format!("symlink rootfs: {e}"))
        })?;

        // Create workspace dir on host
        std::fs::create_dir_all(workspace_dir).map_err(|e| {
            SandcastleError::SandboxCreationFailed(format!("create workspace dir: {e}"))
        })?;

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

        info!("prepared bundle at {}", bundle_dir.display());
        Ok(bundle_dir)
    }
}

#[async_trait]
impl SandboxRuntime for ProcessSandbox {
    async fn create(&self, config: &SandboxConfig) -> Result<SandboxId> {
        let id = format!("sc-{}", uuid::Uuid::new_v4().simple());
        let workspace_dir = self.config.workspace_dir.join(&id);

        // Prepare bundle (sync filesystem ops)
        let bundle_dir = {
            let id = id.clone();
            let language = config.language;
            let sandbox_config = config.clone();
            let process_config = self.config.clone();
            let ws = workspace_dir.clone();

            tokio::task::spawn_blocking(move || {
                let ps = ProcessSandbox::new(process_config);
                ps.prepare_bundle(&id, language, &sandbox_config, &ws)
            })
            .await
            .map_err(|e| SandcastleError::SandboxCreationFailed(format!("spawn blocking: {e}")))?
        }?;

        // Create pipes for stdin/stdout communication
        let (stdin_read, stdin_write) = nix::unistd::pipe()
            .map_err(|e| SandcastleError::SandboxCreationFailed(format!("create stdin pipe: {e}")))?;
        let (stdout_read, stdout_write) = nix::unistd::pipe()
            .map_err(|e| SandcastleError::SandboxCreationFailed(format!("create stdout pipe: {e}")))?;
        let (stderr_read, stderr_write) = nix::unistd::pipe()
            .map_err(|e| SandcastleError::SandboxCreationFailed(format!("create stderr pipe: {e}")))?;

        // Create container using libcontainer (sync, needs spawn_blocking)
        let container_id = id.clone();
        let state_dir = self.config.state_dir.clone();
        let bundle = bundle_dir.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            ContainerBuilder::new(container_id.clone(), SyscallType::default())
                .with_root_path(&state_dir)
                .map_err(|e| {
                    SandcastleError::SandboxCreationFailed(format!("set root path: {e}"))
                })?
                .with_stdin(stdin_read)
                .with_stdout(stdout_write)
                .with_stderr(stderr_write)
                .as_init(&bundle)
                .build()
                .map_err(|e| {
                    SandcastleError::SandboxCreationFailed(format!("build container: {e}"))
                })?;

            debug!("container {container_id} created (state: Created)");
            Ok(())
        })
        .await
        .map_err(|e| SandcastleError::SandboxCreationFailed(format!("spawn blocking: {e}")))??;

        // Drop the stderr read end (we don't read stderr separately — executor merges it)
        drop(stderr_read);

        // Store our ends of the pipes
        let handle = ContainerHandle {
            stdin_writer: std::fs::File::from(stdin_write),
            stdout_reader: Some(BufReader::new(std::fs::File::from(stdout_read))),
            language: config.language,
            bundle_dir,
            workspace_dir,
            status: SandboxStatus::Created,
            exec_lock: Arc::new(Mutex::new(())),
        };

        let mut containers = self.containers.write().await;
        containers.insert(id.clone(), handle);

        info!("sandbox {id} created");
        Ok(SandboxId(id))
    }

    async fn start(&self, id: &SandboxId) -> Result<()> {
        let container_id = id.0.clone();
        let state_dir = self.config.state_dir.clone();

        // Start the container (resumes the init process / executor)
        tokio::task::spawn_blocking(move || -> Result<()> {
            let container_root = state_dir.join(&container_id);
            let mut container = Container::load(container_root).map_err(|e| {
                SandcastleError::RuntimeError(format!("load container {container_id}: {e}"))
            })?;

            container.start().map_err(|e| {
                SandcastleError::RuntimeError(format!("start container {container_id}: {e}"))
            })?;

            debug!("container {container_id} started (state: Running)");
            Ok(())
        })
        .await
        .map_err(|e| SandcastleError::RuntimeError(format!("spawn blocking: {e}")))??;

        // Update status and wait for executor readiness with timeout
        {
            let mut containers = self.containers.write().await;
            if let Some(handle) = containers.get_mut(&id.0) {
                // Take reader out for spawn_blocking (it requires Send)
                let mut reader = handle.stdout_reader.take().ok_or_else(|| {
                    SandcastleError::RuntimeError("stdout reader not available".into())
                })?;

                let read_result = tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    tokio::task::spawn_blocking(move || {
                        let mut ready_line = String::new();
                        let result = reader.read_line(&mut ready_line);
                        (reader, ready_line, result)
                    }),
                )
                .await;

                match read_result {
                    Ok(Ok((reader_back, ready_line, Ok(_)))) => {
                        handle.stdout_reader = Some(reader_back);
                        if !ready_line.contains("\"ready\"") {
                            return Err(SandcastleError::RuntimeError(format!(
                                "unexpected executor startup message: {}",
                                ready_line.trim()
                            )));
                        }
                    }
                    Ok(Ok((_, _, Err(e)))) => {
                        return Err(SandcastleError::RuntimeError(format!(
                            "failed to read executor ready signal: {e}"
                        )));
                    }
                    Ok(Err(e)) => {
                        return Err(SandcastleError::RuntimeError(format!(
                            "readiness check panicked: {e}"
                        )));
                    }
                    Err(_) => {
                        return Err(SandcastleError::RuntimeError(
                            "timeout waiting for executor ready signal (10s)".to_string(),
                        ));
                    }
                }

                handle.status = SandboxStatus::Running;
            }
        }

        info!("sandbox {} started", id.0);
        Ok(())
    }

    async fn execute(&self, id: &SandboxId, request: &ExecRequest) -> Result<ExecResult> {
        // Get the exec lock to serialize concurrent execute calls on same sandbox
        let exec_lock = {
            let containers = self.containers.read().await;
            let handle = containers
                .get(&id.0)
                .ok_or_else(|| SandcastleError::SessionNotFound(id.0.clone()))?;

            if handle.status != SandboxStatus::Running {
                return Err(SandcastleError::ExecutionFailed(format!(
                    "sandbox {} is not running (status: {:?})",
                    id.0, handle.status
                )));
            }

            handle.exec_lock.clone()
        };

        let _guard = exec_lock.lock().await;

        // Build the executor command JSON
        let exec_cmd = serde_json::json!({
            "action": "exec",
            "language": match {
                let containers = self.containers.read().await;
                containers.get(&id.0).map(|h| h.language)
            } {
                Some(Language::Python) => "python",
                Some(Language::Javascript) => "javascript",
                Some(Language::Bash) => "bash",
                None => return Err(SandcastleError::SessionNotFound(id.0.clone())),
            },
            "code": request.code,
            "timeout_ms": request.timeout.as_millis() as u64,
        });

        let cmd_line = format!("{}\n", exec_cmd);

        // Write command and read response (sync IO in spawn_blocking)
        // We need mutable access to the pipes, so we hold a write lock
        let mut containers = self.containers.write().await;
        let handle = containers
            .get_mut(&id.0)
            .ok_or_else(|| SandcastleError::SessionNotFound(id.0.clone()))?;

        let result = {
            let stdin = &mut handle.stdin_writer;
            let stdout = handle.stdout_reader.as_mut().ok_or_else(|| {
                SandcastleError::ExecutionFailed("stdout reader not available".into())
            })?;

            // Write command
            stdin.write_all(cmd_line.as_bytes()).map_err(|e| {
                SandcastleError::ExecutionFailed(format!("write to executor stdin: {e}"))
            })?;
            stdin.flush().map_err(|e| {
                SandcastleError::ExecutionFailed(format!("flush executor stdin: {e}"))
            })?;

            // Read response (one JSON line)
            let mut response_line = String::new();
            stdout.read_line(&mut response_line).map_err(|e| {
                SandcastleError::ExecutionFailed(format!("read from executor stdout: {e}"))
            })?;

            if response_line.is_empty() {
                return Err(SandcastleError::ExecutionFailed(
                    "executor closed stdout (process may have crashed)".into(),
                ));
            }

            serde_json::from_str::<ExecResult>(&response_line).map_err(|e| {
                SandcastleError::ExecutionFailed(format!(
                    "parse executor response: {e}, raw: {response_line}"
                ))
            })?
        };

        debug!("sandbox {} execute: exit_code={}", id.0, result.exit_code);
        Ok(result)
    }

    async fn stop(&self, id: &SandboxId) -> Result<()> {
        let container_id = id.0.clone();
        let state_dir = self.config.state_dir.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let container_root = state_dir.join(&container_id);
            let mut container = Container::load(container_root).map_err(|e| {
                SandcastleError::RuntimeError(format!("load container {container_id}: {e}"))
            })?;

            container
                .kill(nix::sys::signal::Signal::SIGTERM, true)
                .map_err(|e| {
                    SandcastleError::RuntimeError(format!("kill container {container_id}: {e}"))
                })?;

            debug!("container {container_id} stopped");
            Ok(())
        })
        .await
        .map_err(|e| SandcastleError::RuntimeError(format!("spawn blocking: {e}")))??;

        let mut containers = self.containers.write().await;
        if let Some(handle) = containers.get_mut(&id.0) {
            handle.status = SandboxStatus::Stopped;
        }

        info!("sandbox {} stopped", id.0);
        Ok(())
    }

    async fn destroy(&self, id: &SandboxId) -> Result<()> {
        let container_id = id.0.clone();
        let state_dir = self.config.state_dir.clone();

        // Remove from our map first
        let handle = {
            let mut containers = self.containers.write().await;
            containers.remove(&id.0)
        };

        // Delete the container
        tokio::task::spawn_blocking(move || -> Result<()> {
            match Container::load(state_dir.join(&container_id)) {
                Ok(mut container) => {
                    container.delete(true).map_err(|e| {
                        SandcastleError::RuntimeError(format!(
                            "delete container {container_id}: {e}"
                        ))
                    })?;
                    debug!("container {container_id} deleted");
                }
                Err(e) => {
                    warn!("container {container_id} not found for deletion: {e}");
                }
            }
            Ok(())
        })
        .await
        .map_err(|e| SandcastleError::RuntimeError(format!("spawn blocking: {e}")))??;

        // Clean up bundle and workspace directories
        if let Some(handle) = handle {
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

        info!("sandbox {} destroyed", id.0);
        Ok(())
    }

    async fn upload_file(
        &self,
        id: &SandboxId,
        host_path: &Path,
        sandbox_path: &Path,
    ) -> Result<u64> {
        let containers = self.containers.read().await;
        let handle = containers
            .get(&id.0)
            .ok_or_else(|| SandcastleError::SessionNotFound(id.0.clone()))?;

        // Sanitize sandbox path: strip /workspace prefix, reject traversal
        let relative = sandbox_path
            .strip_prefix("/workspace")
            .unwrap_or(sandbox_path);
        if relative.is_absolute() || relative.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            return Err(SandcastleError::PathTraversal(sandbox_path.display().to_string()));
        }
        let dest = handle.workspace_dir.join(relative);

        // Verify dest is still under workspace_dir after resolution
        if !dest.starts_with(&handle.workspace_dir) {
            return Err(SandcastleError::PathTraversal(sandbox_path.display().to_string()));
        }

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                SandcastleError::RuntimeError(format!("create parent dir: {e}"))
            })?;
        }

        let bytes = std::fs::copy(host_path, &dest).map_err(|e| {
            SandcastleError::RuntimeError(format!(
                "copy {} -> {}: {e}",
                host_path.display(),
                dest.display()
            ))
        })?;

        debug!("uploaded {} bytes to sandbox {} at {}", bytes, id.0, dest.display());
        Ok(bytes)
    }

    async fn download_file(
        &self,
        id: &SandboxId,
        sandbox_path: &Path,
        host_path: &Path,
    ) -> Result<u64> {
        let containers = self.containers.read().await;
        let handle = containers
            .get(&id.0)
            .ok_or_else(|| SandcastleError::SessionNotFound(id.0.clone()))?;

        // Sanitize sandbox path: strip /workspace prefix, reject traversal
        let relative = sandbox_path
            .strip_prefix("/workspace")
            .unwrap_or(sandbox_path);
        if relative.is_absolute() || relative.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            return Err(SandcastleError::PathTraversal(sandbox_path.display().to_string()));
        }
        let src = handle.workspace_dir.join(relative);

        // Verify src is still under workspace_dir
        if !src.starts_with(&handle.workspace_dir) {
            return Err(SandcastleError::PathTraversal(sandbox_path.display().to_string()));
        }

        if !src.exists() {
            return Err(SandcastleError::FileNotFound(src));
        }

        // Reject symlinks to prevent escape via symlink created inside sandbox
        let src_meta = std::fs::symlink_metadata(&src).map_err(|e| {
            SandcastleError::RuntimeError(format!("stat file: {e}"))
        })?;
        if src_meta.file_type().is_symlink() {
            return Err(SandcastleError::PathTraversal(format!(
                "symlink not allowed: {}",
                sandbox_path.display()
            )));
        }

        if let Some(parent) = host_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                SandcastleError::RuntimeError(format!("create parent dir: {e}"))
            })?;
        }

        let bytes = std::fs::copy(&src, host_path).map_err(|e| {
            SandcastleError::RuntimeError(format!(
                "copy {} -> {}: {e}",
                src.display(),
                host_path.display()
            ))
        })?;

        debug!(
            "downloaded {} bytes from sandbox {} at {}",
            bytes,
            id.0,
            src.display()
        );
        Ok(bytes)
    }

    async fn status(&self, id: &SandboxId) -> Result<SandboxStatus> {
        let containers = self.containers.read().await;
        let handle = containers
            .get(&id.0)
            .ok_or_else(|| SandcastleError::SessionNotFound(id.0.clone()))?;

        Ok(handle.status.clone())
    }
}
