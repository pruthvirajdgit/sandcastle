use std::path::Path;

use oci_spec::runtime::{
    LinuxBuilder, LinuxNamespace, LinuxNamespaceBuilder, LinuxNamespaceType,
    LinuxPidsBuilder, LinuxResourcesBuilder, MountBuilder, ProcessBuilder, RootBuilder,
    Spec, SpecBuilder,
};
use sandcastle_runtime::{Language, ResourceLimits};

/// Generate an OCI runtime spec for a sandbox container.
///
/// The spec configures:
/// - Namespaces: pid, mount, ipc, uts, network (full isolation)
/// - Resource limits: memory, CPU, PIDs from ResourceLimits
/// - Rootfs: read-only bind of language-specific rootfs
/// - Workspace: bind-mounted read-write directory
/// - Process: runs the executor binary
pub fn generate_spec(
    language: Language,
    limits: &ResourceLimits,
    rootfs_path: &Path,
    workspace_host_path: &Path,
    executor_container_path: &str,
    env_vars: &std::collections::HashMap<String, String>,
) -> Result<Spec, String> {
    let process = build_process(language, executor_container_path, env_vars)?;
    let root = build_root(rootfs_path)?;
    let mounts = build_mounts(workspace_host_path);
    let linux = build_linux(limits)?;

    SpecBuilder::default()
        .version("1.0.2")
        .process(process)
        .root(root)
        .mounts(mounts)
        .linux(linux)
        .build()
        .map_err(|e| format!("failed to build OCI spec: {e}"))
}

fn build_process(
    language: Language,
    executor_path: &str,
    env_vars: &std::collections::HashMap<String, String>,
) -> Result<oci_spec::runtime::Process, String> {
    let mut env: Vec<String> = vec![
        format!("PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"),
        format!("LANG=C.UTF-8"),
        format!("SANDBOX_LANGUAGE={}", language),
        format!("HOME=/workspace"),
        format!("TERM=xterm"),
    ];

    for (k, v) in env_vars {
        env.push(format!("{k}={v}"));
    }

    ProcessBuilder::default()
        .terminal(false)
        .args(vec![executor_path.to_string()])
        .env(env)
        .cwd("/workspace")
        .build()
        .map_err(|e| format!("failed to build process spec: {e}"))
}

fn build_root(rootfs_path: &Path) -> Result<oci_spec::runtime::Root, String> {
    RootBuilder::default()
        .path(rootfs_path)
        .readonly(false)
        .build()
        .map_err(|e| format!("failed to build root spec: {e}"))
}

fn build_mounts(
    workspace_host_path: &Path,
) -> Vec<oci_spec::runtime::Mount> {
    vec![
        // /proc
        MountBuilder::default()
            .destination("/proc")
            .typ("proc")
            .source("proc")
            .build()
            .expect("proc mount"),
        // /dev as tmpfs — libcontainer creates device nodes itself
        MountBuilder::default()
            .destination("/dev")
            .typ("tmpfs")
            .source("tmpfs")
            .options(vec![
                "nosuid".into(),
                "strictatime".into(),
                "mode=755".into(),
                "size=65536k".into(),
            ])
            .build()
            .expect("dev mount"),
        // /workspace (bind-mounted from host)
        MountBuilder::default()
            .destination("/workspace")
            .typ("bind")
            .source(workspace_host_path)
            .options(vec!["bind".into(), "rw".into()])
            .build()
            .expect("workspace mount"),
    ]
}

fn build_linux(limits: &ResourceLimits) -> Result<oci_spec::runtime::Linux, String> {
    let namespaces: Vec<LinuxNamespace> = vec![
        LinuxNamespaceBuilder::default()
            .typ(LinuxNamespaceType::Pid)
            .build()
            .unwrap(),
        LinuxNamespaceBuilder::default()
            .typ(LinuxNamespaceType::Mount)
            .build()
            .unwrap(),
        LinuxNamespaceBuilder::default()
            .typ(LinuxNamespaceType::Ipc)
            .build()
            .unwrap(),
        LinuxNamespaceBuilder::default()
            .typ(LinuxNamespaceType::Uts)
            .build()
            .unwrap(),
        LinuxNamespaceBuilder::default()
            .typ(LinuxNamespaceType::Network)
            .build()
            .unwrap(),
    ];

    let pids = LinuxPidsBuilder::default()
        .limit(limits.max_pids as i64)
        .build()
        .map_err(|e| format!("failed to build pids limit: {e}"))?;

    let resources = LinuxResourcesBuilder::default()
        .pids(pids)
        .build()
        .map_err(|e| format!("failed to build resources: {e}"))?;

    LinuxBuilder::default()
        .namespaces(namespaces)
        .resources(resources)
        .build()
        .map_err(|e| format!("failed to build linux spec: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn test_generate_spec() {
        let limits = ResourceLimits::default();
        let rootfs = PathBuf::from("/tmp/rootfs/python");
        let workspace = PathBuf::from("/tmp/workspace/test");

        let spec = generate_spec(
            Language::Python,
            &limits,
            &rootfs,
            &workspace,
            "/sandbox/executor",
            &HashMap::new(),
        );

        assert!(spec.is_ok(), "spec generation failed: {:?}", spec.err());
        let spec = spec.unwrap();

        // Check version
        assert_eq!(spec.version(), "1.0.2");

        // Check process
        let process = spec.process().as_ref().unwrap();
        assert_eq!(process.args().as_ref().unwrap(), &["/sandbox/executor"]);
        assert_eq!(process.cwd().to_str().unwrap(), "/workspace");

        // Check root
        let root = spec.root().as_ref().unwrap();
        assert_eq!(root.path().to_str().unwrap(), "/tmp/rootfs/python");

        // Check linux namespaces
        let linux = spec.linux().as_ref().unwrap();
        let ns_types: Vec<_> = linux
            .namespaces()
            .as_ref()
            .unwrap()
            .iter()
            .map(|ns| ns.typ())
            .collect();
        assert!(ns_types.contains(&LinuxNamespaceType::Pid));
        assert!(ns_types.contains(&LinuxNamespaceType::Mount));
        assert!(ns_types.contains(&LinuxNamespaceType::Ipc));
        assert!(ns_types.contains(&LinuxNamespaceType::Uts));
        assert!(ns_types.contains(&LinuxNamespaceType::Network));
        assert_eq!(ns_types.len(), 5);
    }
}
