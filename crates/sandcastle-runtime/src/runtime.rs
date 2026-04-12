use async_trait::async_trait;
use std::path::Path;

use crate::{ExecRequest, ExecResult, Result, SandboxConfig, SandboxId, SandboxStatus};

/// The core trait that all sandbox implementations must satisfy.
///
/// The manager calls this interface — it never knows whether it's
/// talking to a process sandbox, gVisor, or Firecracker underneath.
#[async_trait]
pub trait SandboxRuntime: Send + Sync {
    /// Create a new sandbox (allocate resources, prepare rootfs).
    /// Does not start it.
    async fn create(&self, config: &SandboxConfig) -> Result<SandboxId>;

    /// Start the sandbox (spawn executor process, begin accepting commands).
    async fn start(&self, id: &SandboxId) -> Result<()>;

    /// Execute code inside a running sandbox.
    async fn execute(&self, id: &SandboxId, request: &ExecRequest) -> Result<ExecResult>;

    /// Stop the sandbox (graceful shutdown).
    async fn stop(&self, id: &SandboxId) -> Result<()>;

    /// Destroy the sandbox and clean up all resources.
    async fn destroy(&self, id: &SandboxId) -> Result<()>;

    /// Copy a file from host into the sandbox's /workspace.
    /// Returns the number of bytes copied.
    async fn upload_file(
        &self,
        id: &SandboxId,
        host_path: &Path,
        sandbox_path: &Path,
    ) -> Result<u64>;

    /// Copy a file from the sandbox's /workspace to the host.
    /// Returns the number of bytes copied.
    async fn download_file(
        &self,
        id: &SandboxId,
        sandbox_path: &Path,
        host_path: &Path,
    ) -> Result<u64>;

    /// Check the current status of a sandbox.
    async fn status(&self, id: &SandboxId) -> Result<SandboxStatus>;
}
