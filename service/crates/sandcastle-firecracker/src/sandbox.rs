use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use firepilot::builder::Builder;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

use sandcastle_runtime::{
    ExecRequest, ExecResult, Language, Result, SandboxConfig, SandboxId, SandboxRuntime,
    SandboxStatus, SandcastleError,
};

use crate::config::FirecrackerConfig;
use crate::vsock::VsockConnection;

/// State held for each active Firecracker microVM.
struct VmHandle {
    /// The firepilot Machine managing this VM.
    machine: firepilot::machine::Machine,
    /// Vsock connection to the executor inside the VM.
    vsock: Option<VsockConnection>,
    /// Language for this sandbox.
    language: Language,
    /// Path to the workspace directory on the host.
    workspace_dir: PathBuf,
    /// VM ID (used for socket paths and cleanup).
    vm_id: String,
    /// Path to the vsock UDS proxy socket.
    vsock_uds_path: PathBuf,
    /// Guest CID assigned to this VM.
    guest_cid: u32,
    /// Mutex to serialize execute calls on this VM.
    exec_lock: Arc<Mutex<()>>,
    /// Whether the VM has been started.
    started: bool,
}

/// Firecracker microVM sandbox — high isolation backend.
///
/// Each sandbox is a Firecracker microVM running the executor binary as init (PID 1).
/// Communication happens via vsock through Firecracker's UDS proxy.
pub struct FirecrackerSandbox {
    config: FirecrackerConfig,
    vms: RwLock<HashMap<String, VmHandle>>,
    /// Monotonic CID counter to assign unique CIDs to each VM.
    cid_counter: AtomicU32,
}

impl FirecrackerSandbox {
    pub fn new(config: FirecrackerConfig) -> Self {
        let base_cid = config.guest_cid_base;
        Self {
            config,
            vms: RwLock::new(HashMap::new()),
            cid_counter: AtomicU32::new(base_cid),
        }
    }

    /// Check if the Firecracker binary and kernel are available.
    pub fn is_available(&self) -> bool {
        self.config.is_available()
    }

    /// Ensure all required directories exist.
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.config.state_dir)?;
        std::fs::create_dir_all(&self.config.workspace_dir)?;
        Ok(())
    }

    /// Allocate the next unique guest CID.
    fn next_cid(&self) -> u32 {
        self.cid_counter.fetch_add(1, Ordering::SeqCst)
    }

    /// Get the rootfs ext4 image path for a language.
    fn rootfs_image_path(&self, language: Language) -> PathBuf {
        let name = match language {
            Language::Python => "python",
            Language::Javascript => "javascript",
            Language::Bash => "bash",
        };
        self.config.rootfs_dir.join(format!("{}.ext4", name))
    }

    /// Configure machine resources (vCPU/memory) on a Firecracker instance via its API socket.
    async fn configure_machine(
        socket_path: &Path,
        vcpu_count: u32,
        memory_mb: u32,
    ) -> Result<()> {
        use hyper::{Body, Client, Method, Request};
        use hyperlocal::{UnixClientExt, Uri};

        let client = Client::unix();

        let machine_config = serde_json::json!({
            "vcpu_count": vcpu_count,
            "mem_size_mib": memory_mb,
        });

        let url = Uri::new(socket_path, "/machine-config");
        let req = Request::builder()
            .method(Method::PUT)
            .uri(url)
            .header("Content-Type", "application/json")
            .body(Body::from(machine_config.to_string()))
            .map_err(|e| SandcastleError::RuntimeError(
                format!("failed to build machine config request: {}", e)
            ))?;

        let resp = client.request(req).await.map_err(|e| {
            SandcastleError::RuntimeError(format!("failed to configure machine: {}", e))
        })?;

        if !resp.status().is_success() {
            let body = hyper::body::to_bytes(resp.into_body()).await.unwrap_or_default();
            return Err(SandcastleError::RuntimeError(
                format!("machine config failed: {}", String::from_utf8_lossy(&body))
            ));
        }

        debug!("machine configured: vcpu={}, memory={}MB", vcpu_count, memory_mb);
        Ok(())
    }

    /// Configure vsock on a running Firecracker instance via its API socket.
    async fn configure_vsock(
        socket_path: &Path,
        guest_cid: u32,
        uds_path: &Path,
    ) -> Result<()> {
        use hyper::{Body, Client, Method, Request};
        use hyperlocal::{UnixClientExt, Uri};

        let client = Client::unix();

        let vsock_config = serde_json::json!({
            "guest_cid": guest_cid,
            "uds_path": uds_path.to_string_lossy()
        });

        let url = Uri::new(socket_path, "/vsock");
        let req = Request::builder()
            .method(Method::PUT)
            .uri(url)
            .header("Content-Type", "application/json")
            .body(Body::from(vsock_config.to_string()))
            .map_err(|e| SandcastleError::RuntimeError(
                format!("failed to build vsock config request: {}", e)
            ))?;

        let resp = client.request(req).await.map_err(|e| {
            SandcastleError::RuntimeError(format!("failed to configure vsock: {}", e))
        })?;

        if !resp.status().is_success() {
            let body = hyper::body::to_bytes(resp.into_body()).await.unwrap_or_default();
            return Err(SandcastleError::RuntimeError(
                format!("vsock config failed: {}", String::from_utf8_lossy(&body))
            ));
        }

        debug!("vsock configured: guest_cid={}, uds={:?}", guest_cid, uds_path);
        Ok(())
    }
}

#[async_trait]
impl SandboxRuntime for FirecrackerSandbox {
    async fn create(&self, config: &SandboxConfig) -> Result<SandboxId> {
        let sandbox_id = format!("fc-{}", uuid::Uuid::new_v4().simple());
        let vm_id = sandbox_id.clone();
        let guest_cid = self.next_cid();

        // Verify rootfs image exists
        let rootfs_path = self.rootfs_image_path(config.language);
        if !rootfs_path.exists() {
            return Err(SandcastleError::SandboxCreationFailed(
                format!("rootfs image not found: {:?}", rootfs_path)
            ));
        }

        // Create workspace directory for this VM
        let workspace_dir = self.config.workspace_dir.join(&sandbox_id);
        std::fs::create_dir_all(&workspace_dir).map_err(|e| {
            SandcastleError::SandboxCreationFailed(
                format!("failed to create workspace dir: {}", e)
            )
        })?;

        // Set permissions so the VM guest can write
        #[allow(clippy::unnecessary_cast)]
        std::fs::set_permissions(&workspace_dir, std::fs::Permissions::from_mode(0o777 as u32))
            .map_err(|e| {
                SandcastleError::SandboxCreationFailed(
                    format!("failed to set workspace permissions: {}", e)
                )
            })?;

        let vsock_uds_path = self.config.state_dir.join(format!("{}.vsock", sandbox_id));

        // Build the Firecracker VM configuration using firepilot
        let executor = firepilot::builder::executor::FirecrackerExecutorBuilder::new()
            .with_chroot(self.config.state_dir.to_string_lossy().to_string())
            .with_exec_binary(self.config.firecracker_path.clone())
            .try_build()
            .map_err(|e| SandcastleError::SandboxCreationFailed(
                format!("failed to build executor config: {:?}", e)
            ))?;

        let kernel = firepilot::builder::kernel::KernelBuilder::new()
            .with_kernel_image_path(self.config.kernel_path.to_string_lossy().to_string())
            .with_boot_args(self.config.boot_args.clone())
            .try_build()
            .map_err(|e| SandcastleError::SandboxCreationFailed(
                format!("failed to build kernel config: {:?}", e)
            ))?;

        let drive = firepilot::builder::drive::DriveBuilder::new()
            .with_drive_id("rootfs".to_string())
            .with_path_on_host(rootfs_path)
            .as_root_device()
            .try_build()
            .map_err(|e| SandcastleError::SandboxCreationFailed(
                format!("failed to build drive config: {:?}", e)
            ))?;

        let fc_config = firepilot::builder::Configuration::new(vm_id.clone())
            .with_executor(executor)
            .with_kernel(kernel)
            .with_drive(drive);

        let mut machine = firepilot::machine::Machine::new();

        // Create the VM (configures Firecracker, does NOT start the guest)
        machine.create(fc_config).await.map_err(|e| {
            SandcastleError::SandboxCreationFailed(format!("failed to create VM: {:?}", e))
        })?;

        info!("created Firecracker VM: {} (cid={})", sandbox_id, guest_cid);

        let handle = VmHandle {
            machine,
            vsock: None,
            language: config.language,
            workspace_dir,
            vm_id,
            vsock_uds_path,
            guest_cid,
            exec_lock: Arc::new(Mutex::new(())),
            started: false,
        };

        self.vms.write().await.insert(sandbox_id.clone(), handle);

        Ok(SandboxId(sandbox_id))
    }

    async fn start(&self, id: &SandboxId) -> Result<()> {
        // Read config data with a brief lock, then release
        let (vm_id, guest_cid, vsock_uds_path, vsock_port) = {
            let vms = self.vms.read().await;
            let handle = vms.get(&id.0).ok_or_else(|| {
                SandcastleError::RuntimeError(format!("VM {} not found", id.0))
            })?;
            (
                handle.vm_id.clone(),
                handle.guest_cid,
                handle.vsock_uds_path.clone(),
                self.config.vsock_port,
            )
        };

        // Get the API socket path (firepilot creates it during machine.create())
        let socket_path = self.config.state_dir.join(&vm_id).join("firecracker.socket");

        // Configure machine resources (vCPU/memory) via Firecracker API
        Self::configure_machine(&socket_path, self.config.vcpu_count, self.config.memory_mb).await?;

        // Configure vsock before starting the VM
        Self::configure_vsock(&socket_path, guest_cid, &vsock_uds_path).await?;

        // Start the VM without holding the global lock across .await
        let machine = {
            let mut vms = self.vms.write().await;
            let handle = vms.get_mut(&id.0).ok_or_else(|| {
                SandcastleError::RuntimeError(format!("VM {} not found", id.0))
            })?;
            // Temporarily take the machine out of the handle
            std::mem::replace(&mut handle.machine, firepilot::machine::Machine::new())
        };

        let start_result = machine.start().await;

        // Put the machine back regardless of result
        {
            let mut vms = self.vms.write().await;
            if let Some(handle) = vms.get_mut(&id.0) {
                handle.machine = machine;
                if start_result.is_ok() {
                    handle.started = true;
                }
            }
        }

        start_result.map_err(|e| {
            SandcastleError::SandboxCreationFailed(format!("failed to start VM: {:?}", e))
        })?;

        // Connect to the executor via vsock with retry loop.
        // The VM needs time to boot and the executor needs to bind its vsock listener.
        let boot_timeout = std::time::Duration::from_secs(30);
        let retry_interval = std::time::Duration::from_millis(500);
        let deadline = tokio::time::Instant::now() + boot_timeout;

        let mut vsock_conn = loop {
            match VsockConnection::connect(&vsock_uds_path, vsock_port).await {
                Ok(conn) => break conn,
                Err(e) => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(SandcastleError::SandboxCreationFailed(
                            format!("VM boot timeout — failed to connect to executor vsock: {}", e)
                        ));
                    }
                    debug!("vsock not ready yet, retrying: {}", e);
                    tokio::time::sleep(retry_interval).await;
                }
            }
        };

        // Wait for executor readiness
        vsock_conn.wait_ready().await?;

        // Store the vsock connection
        {
            let mut vms = self.vms.write().await;
            let handle = vms.get_mut(&id.0).ok_or_else(|| {
                SandcastleError::RuntimeError(format!("VM {} not found", id.0))
            })?;
            handle.vsock = Some(vsock_conn);
        }

        info!("Firecracker VM {} started and executor ready", id.0);
        Ok(())
    }

    async fn execute(&self, id: &SandboxId, request: &ExecRequest) -> Result<ExecResult> {
        let exec_lock = {
            let vms = self.vms.read().await;
            let handle = vms.get(&id.0).ok_or_else(|| {
                SandcastleError::RuntimeError(format!("VM {} not found", id.0))
            })?;
            handle.exec_lock.clone()
        };

        let _guard = exec_lock.lock().await;

        // Get language from handle
        let language = {
            let vms = self.vms.read().await;
            let handle = vms.get(&id.0).ok_or_else(|| {
                SandcastleError::RuntimeError(format!("VM {} not found", id.0))
            })?;
            handle.language
        };

        let exec_cmd = serde_json::json!({
            "action": "exec",
            "language": language.to_string(),
            "code": request.code,
            "timeout_ms": request.timeout.as_millis() as u64,
        });

        let timeout = request.timeout + std::time::Duration::from_secs(5);

        let response_line = {
            // Take vsock connection briefly under write lock, then release
            let mut vsock = {
                let mut vms = self.vms.write().await;
                let handle = vms.get_mut(&id.0).ok_or_else(|| {
                    SandcastleError::RuntimeError(format!("VM {} not found", id.0))
                })?;
                handle.vsock.take().ok_or_else(|| {
                    SandcastleError::RuntimeError("VM not started (no vsock connection)".to_string())
                })?
            };

            let result = vsock.execute_json(&exec_cmd.to_string(), timeout).await;

            // Put vsock connection back
            {
                let mut vms = self.vms.write().await;
                if let Some(handle) = vms.get_mut(&id.0) {
                    handle.vsock = Some(vsock);
                }
            }

            result?
        };

        let result: ExecResult = serde_json::from_str(response_line.trim()).map_err(|e| {
            SandcastleError::ExecutionFailed(format!(
                "failed to parse executor response: {} (raw: {})",
                e,
                response_line.trim()
            ))
        })?;

        Ok(result)
    }

    async fn stop(&self, id: &SandboxId) -> Result<()> {
        // Take machine out under brief lock, then operate without lock
        let machine_opt = {
            let mut vms = self.vms.write().await;
            let handle = vms.get_mut(&id.0).ok_or_else(|| {
                SandcastleError::RuntimeError(format!("VM {} not found", id.0))
            })?;
            // Drop vsock connection
            handle.vsock.take();
            if handle.started {
                handle.started = false;
                Some(std::mem::replace(&mut handle.machine, firepilot::machine::Machine::new()))
            } else {
                None
            }
        };

        // Perform the async operation without holding the lock
        if let Some(mut machine) = machine_opt {
            if let Err(e) = machine.kill().await {
                warn!("failed to stop VM {}: {:?}", id.0, e);
            }
            // Put machine back
            let mut vms = self.vms.write().await;
            if let Some(handle) = vms.get_mut(&id.0) {
                handle.machine = machine;
            }
        }

        info!("stopped Firecracker VM {}", id.0);
        Ok(())
    }

    async fn destroy(&self, id: &SandboxId) -> Result<()> {
        // Stop first if still running
        let _ = self.stop(id).await;

        let mut vms = self.vms.write().await;
        if let Some(handle) = vms.remove(&id.0) {
            // Clean up workspace directory
            if handle.workspace_dir.exists() {
                let _ = std::fs::remove_dir_all(&handle.workspace_dir);
            }

            // Clean up vsock UDS socket
            if handle.vsock_uds_path.exists() {
                let _ = std::fs::remove_file(&handle.vsock_uds_path);
            }

            // Clean up VM state directory
            let vm_state = self.config.state_dir.join(&handle.vm_id);
            if vm_state.exists() {
                let _ = std::fs::remove_dir_all(&vm_state);
            }

            info!("destroyed Firecracker VM {}", id.0);
        }

        Ok(())
    }

    async fn upload_file(
        &self,
        id: &SandboxId,
        host_path: &Path,
        sandbox_path: &Path,
    ) -> Result<u64> {
        let relative = sanitize_sandbox_path(sandbox_path)?;

        // Read the file from host
        let data = std::fs::read(host_path).map_err(|e| {
            SandcastleError::RuntimeError(format!("failed to read host file: {}", e))
        })?;
        let file_size = data.len() as u64;

        // Base64 encode
        let encoded = base64_encode(&data);

        // Send upload command via vsock
        let upload_cmd = serde_json::json!({
            "action": "upload",
            "path": relative.to_string_lossy(),
            "content_base64": encoded,
        });

        let exec_lock = {
            let vms = self.vms.read().await;
            let handle = vms.get(&id.0).ok_or_else(|| {
                SandcastleError::RuntimeError(format!("VM {} not found", id.0))
            })?;
            handle.exec_lock.clone()
        };

        let _guard = exec_lock.lock().await;

        let response_line = {
            let mut vsock = {
                let mut vms = self.vms.write().await;
                let handle = vms.get_mut(&id.0).ok_or_else(|| {
                    SandcastleError::RuntimeError(format!("VM {} not found", id.0))
                })?;
                handle.vsock.take().ok_or_else(|| {
                    SandcastleError::RuntimeError("VM not started (no vsock connection)".to_string())
                })?
            };

            let result = vsock.execute_json(
                &upload_cmd.to_string(),
                std::time::Duration::from_secs(30),
            ).await;

            {
                let mut vms = self.vms.write().await;
                if let Some(handle) = vms.get_mut(&id.0) {
                    handle.vsock = Some(vsock);
                }
            }

            result?
        };

        let resp: serde_json::Value = serde_json::from_str(response_line.trim()).map_err(|e| {
            SandcastleError::RuntimeError(format!("failed to parse upload response: {}", e))
        })?;

        if resp.get("exit_code").and_then(|v| v.as_i64()) != Some(0) {
            let stderr = resp.get("stderr").and_then(|v| v.as_str()).unwrap_or("unknown error");
            return Err(SandcastleError::RuntimeError(
                format!("upload failed: {}", stderr)
            ));
        }

        debug!("uploaded {} bytes to VM {} via vsock", file_size, id.0);
        Ok(file_size)
    }

    async fn download_file(
        &self,
        id: &SandboxId,
        sandbox_path: &Path,
        host_path: &Path,
    ) -> Result<u64> {
        let relative = sanitize_sandbox_path(sandbox_path)?;

        // Send download command via vsock
        let download_cmd = serde_json::json!({
            "action": "download",
            "path": relative.to_string_lossy(),
        });

        let exec_lock = {
            let vms = self.vms.read().await;
            let handle = vms.get(&id.0).ok_or_else(|| {
                SandcastleError::RuntimeError(format!("VM {} not found", id.0))
            })?;
            handle.exec_lock.clone()
        };

        let _guard = exec_lock.lock().await;

        let response_line = {
            let mut vsock = {
                let mut vms = self.vms.write().await;
                let handle = vms.get_mut(&id.0).ok_or_else(|| {
                    SandcastleError::RuntimeError(format!("VM {} not found", id.0))
                })?;
                handle.vsock.take().ok_or_else(|| {
                    SandcastleError::RuntimeError("VM not started (no vsock connection)".to_string())
                })?
            };

            let result = vsock.execute_json(
                &download_cmd.to_string(),
                std::time::Duration::from_secs(30),
            ).await;

            {
                let mut vms = self.vms.write().await;
                if let Some(handle) = vms.get_mut(&id.0) {
                    handle.vsock = Some(vsock);
                }
            }

            result?
        };

        let resp: serde_json::Value = serde_json::from_str(response_line.trim()).map_err(|e| {
            SandcastleError::RuntimeError(format!("failed to parse download response: {}", e))
        })?;

        if resp.get("exit_code").and_then(|v| v.as_i64()) != Some(0) {
            let stderr = resp.get("stderr").and_then(|v| v.as_str()).unwrap_or("unknown error");
            return Err(SandcastleError::RuntimeError(
                format!("download failed: {}", stderr)
            ));
        }

        // Decode base64 content from stdout
        let encoded = resp.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
        let data = base64_decode(encoded).map_err(|e| {
            SandcastleError::RuntimeError(format!("failed to decode downloaded file: {}", e))
        })?;

        let file_size = data.len() as u64;
        std::fs::write(host_path, &data).map_err(|e| {
            SandcastleError::RuntimeError(format!("failed to write host file: {}", e))
        })?;

        debug!("downloaded {} bytes from VM {} via vsock", file_size, id.0);
        Ok(file_size)
    }

    async fn status(&self, id: &SandboxId) -> Result<SandboxStatus> {
        let vms = self.vms.read().await;
        match vms.get(&id.0) {
            None => Ok(SandboxStatus::Stopped),
            Some(handle) => {
                if handle.started && handle.vsock.is_some() {
                    Ok(SandboxStatus::Running)
                } else if handle.started {
                    Ok(SandboxStatus::Failed("vsock connection lost".to_string()))
                } else {
                    Ok(SandboxStatus::Created)
                }
            }
        }
    }
}

/// Sanitize sandbox paths to prevent path traversal attacks.
fn sanitize_sandbox_path(sandbox_path: &Path) -> Result<PathBuf> {
    use std::path::Component;

    // Strip /workspace or workspace prefix
    let stripped = sandbox_path
        .strip_prefix("/workspace")
        .or_else(|_| sandbox_path.strip_prefix("workspace"))
        .unwrap_or(sandbox_path);

    // Reject absolute paths
    if stripped.is_absolute() {
        return Err(SandcastleError::PathTraversal(
            format!("absolute path not allowed: {}", sandbox_path.display())
        ));
    }

    // Reject .. and other dangerous components
    for component in stripped.components() {
        match component {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(SandcastleError::PathTraversal(
                    format!("invalid path component in: {}", sandbox_path.display())
                ));
            }
            _ => {}
        }
    }

    Ok(stripped.to_path_buf())
}

/// Simple base64 encoder.
fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((n >> 18) & 63) as usize] as char);
        result.push(CHARS[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((n >> 6) & 63) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[(n & 63) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

/// Simple base64 decoder.
fn base64_decode(input: &str) -> std::result::Result<Vec<u8>, String> {
    fn char_to_val(c: u8) -> std::result::Result<u8, String> {
        match c {
            b'A'..=b'Z' => Ok(c - b'A'),
            b'a'..=b'z' => Ok(c - b'a' + 26),
            b'0'..=b'9' => Ok(c - b'0' + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => Err(format!("invalid base64 char: {}", c as char)),
        }
    }

    let input = input.trim();
    if input.is_empty() {
        return Ok(Vec::new());
    }

    let bytes: Vec<u8> = input.bytes().filter(|&b| b != b'\n' && b != b'\r').collect();
    if bytes.len() % 4 != 0 {
        return Err("invalid base64 length".to_string());
    }

    let mut result = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let pad = chunk.iter().filter(|&&b| b == b'=').count();
        let v0 = char_to_val(chunk[0])?;
        let v1 = char_to_val(chunk[1])?;
        result.push((v0 << 2) | (v1 >> 4));
        if pad < 2 {
            let v2 = char_to_val(chunk[2])?;
            result.push((v1 << 4) | (v2 >> 2));
            if pad < 1 {
                let v3 = char_to_val(chunk[3])?;
                result.push((v2 << 6) | v3);
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_sandbox_path_valid() {
        let p = sanitize_sandbox_path(Path::new("/workspace/file.txt")).unwrap();
        assert_eq!(p, PathBuf::from("file.txt"));

        let p = sanitize_sandbox_path(Path::new("workspace/subdir/file.txt")).unwrap();
        assert_eq!(p, PathBuf::from("subdir/file.txt"));

        let p = sanitize_sandbox_path(Path::new("file.txt")).unwrap();
        assert_eq!(p, PathBuf::from("file.txt"));
    }

    #[test]
    fn test_sanitize_sandbox_path_traversal() {
        assert!(sanitize_sandbox_path(Path::new("/workspace/../etc/passwd")).is_err());
        assert!(sanitize_sandbox_path(Path::new("/etc/passwd")).is_err());
        assert!(sanitize_sandbox_path(Path::new("../../../etc/passwd")).is_err());
    }

    #[test]
    fn test_firecracker_sandbox_new() {
        let config = FirecrackerConfig::default();
        let sandbox = FirecrackerSandbox::new(config);
        // Just verify it constructs without panic
        let _ = sandbox.is_available();
    }
}
