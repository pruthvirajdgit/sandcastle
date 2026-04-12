use std::path::PathBuf;

/// Configuration for the GvisorSandbox runtime.
#[derive(Debug, Clone)]
pub struct GvisorConfig {
    /// Path to the runsc binary.
    pub runsc_path: PathBuf,

    /// Directory containing per-language rootfs trees.
    pub rootfs_dir: PathBuf,

    /// Directory where runsc stores container state.
    pub state_dir: PathBuf,

    /// Directory for per-container bundles (config.json + rootfs link).
    pub bundle_dir: PathBuf,

    /// Directory for per-container workspace bind-mounts on the host.
    pub workspace_dir: PathBuf,

    /// Path to the executor binary that runs inside the container.
    pub executor_path: PathBuf,

    /// Platform for gVisor: "ptrace" (default, no KVM needed) or "kvm".
    pub platform: String,
}

impl Default for GvisorConfig {
    fn default() -> Self {
        Self {
            runsc_path: PathBuf::from("/usr/local/bin/runsc"),
            rootfs_dir: PathBuf::from("/var/lib/sandcastle/rootfs"),
            state_dir: PathBuf::from("/run/sandcastle/gvisor"),
            bundle_dir: PathBuf::from("/var/lib/sandcastle/gvisor-bundles"),
            workspace_dir: PathBuf::from("/var/lib/sandcastle/gvisor-workspaces"),
            executor_path: PathBuf::from("/var/lib/sandcastle/bin/executor"),
            platform: "ptrace".to_string(),
        }
    }
}
