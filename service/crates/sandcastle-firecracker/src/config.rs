use std::path::PathBuf;

/// Configuration for the Firecracker microVM backend.
#[derive(Debug, Clone)]
pub struct FirecrackerConfig {
    /// Path to the Firecracker binary.
    pub firecracker_path: PathBuf,
    /// Path to the vmlinux kernel image.
    pub kernel_path: PathBuf,
    /// Base directory containing ext4 rootfs images per language.
    /// Expected layout: {rootfs_dir}/{language}.ext4
    pub rootfs_dir: PathBuf,
    /// Directory for VM state (sockets, logs).
    pub state_dir: PathBuf,
    /// Directory for per-VM workspace mounts.
    pub workspace_dir: PathBuf,
    /// Guest CID for vsock (must be >= 3).
    pub guest_cid_base: u32,
    /// Vsock port the executor listens on inside the VM.
    pub vsock_port: u32,
    /// Memory size in MiB for each microVM.
    pub memory_mb: u32,
    /// Number of vCPUs for each microVM.
    pub vcpu_count: u32,
    /// Kernel boot args.
    pub boot_args: String,
}

impl Default for FirecrackerConfig {
    fn default() -> Self {
        Self {
            firecracker_path: PathBuf::from("/usr/local/bin/firecracker"),
            kernel_path: PathBuf::from("/var/lib/sandcastle/kernel/vmlinux"),
            rootfs_dir: PathBuf::from("/var/lib/sandcastle/rootfs"),
            state_dir: PathBuf::from("/run/sandcastle/firecracker"),
            workspace_dir: PathBuf::from("/var/lib/sandcastle/fc-workspaces"),
            guest_cid_base: 100,
            vsock_port: 5000,
            memory_mb: 256,
            vcpu_count: 1,
            boot_args: "console=ttyS0 reboot=k panic=1 pci=off init=/sandbox/executor -- --vsock".to_string(),
        }
    }
}

impl FirecrackerConfig {
    /// Check if the Firecracker binary is available.
    pub fn is_available(&self) -> bool {
        self.firecracker_path.exists() && self.kernel_path.exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = FirecrackerConfig::default();
        assert_eq!(config.memory_mb, 256);
        assert_eq!(config.vcpu_count, 1);
        assert_eq!(config.vsock_port, 5000);
        assert_eq!(config.guest_cid_base, 100);
        assert!(config.boot_args.contains("--vsock"));
    }
}
