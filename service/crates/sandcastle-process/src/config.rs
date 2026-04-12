use std::path::PathBuf;

/// Configuration for the ProcessSandbox runtime.
#[derive(Debug, Clone)]
pub struct ProcessConfig {
    /// Directory containing per-language rootfs trees.
    /// Expected structure: {rootfs_dir}/{language}/ (e.g., rootfs/python/, rootfs/bash/)
    pub rootfs_dir: PathBuf,

    /// Directory where container state is stored (like /run/youki).
    pub state_dir: PathBuf,

    /// Directory for per-container bundles (config.json + rootfs link).
    pub bundle_dir: PathBuf,

    /// Directory for per-container workspace bind-mounts on the host.
    pub workspace_dir: PathBuf,

    /// Path to the executor binary that runs inside the container.
    pub executor_path: PathBuf,
}

impl Default for ProcessConfig {
    fn default() -> Self {
        Self {
            rootfs_dir: PathBuf::from("/var/lib/sandcastle/rootfs"),
            state_dir: PathBuf::from("/run/sandcastle"),
            bundle_dir: PathBuf::from("/var/lib/sandcastle/bundles"),
            workspace_dir: PathBuf::from("/var/lib/sandcastle/workspaces"),
            executor_path: PathBuf::from("/var/lib/sandcastle/bin/executor"),
        }
    }
}
