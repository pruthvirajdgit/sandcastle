use sandcastle_runtime::{Language, ResourceLimits};
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;

/// Generate OCI runtime spec (config.json) for gVisor.
/// Similar to process backend but includes network namespace and all standard namespaces
/// that gVisor expects.
pub fn generate_spec(
    language: Language,
    limits: &ResourceLimits,
    rootfs_path: &Path,
    workspace_host_path: &Path,
    executor_container_path: &str,
    env_vars: &HashMap<String, String>,
) -> Result<serde_json::Value, String> {
    let mut env = vec![
        "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
        "LANG=C.UTF-8".to_string(),
        format!("SANDBOX_LANGUAGE={language}"),
        "HOME=/workspace".to_string(),
        "TERM=xterm".to_string(),
    ];
    for (k, v) in env_vars {
        env.push(format!("{k}={v}"));
    }

    let pids_limit = limits.max_pids as i64;

    let spec = json!({
        "ociVersion": "1.0.2",
        "process": {
            "terminal": false,
            "user": { "uid": 0, "gid": 0 },
            "args": [executor_container_path],
            "env": env,
            "cwd": "/workspace"
        },
        "root": {
            "path": rootfs_path.to_string_lossy(),
            "readonly": false
        },
        "mounts": [
            {
                "destination": "/proc",
                "type": "proc",
                "source": "proc"
            },
            {
                "destination": "/dev",
                "type": "tmpfs",
                "source": "tmpfs",
                "options": ["nosuid", "mode=755"]
            },
            {
                "destination": "/workspace",
                "type": "bind",
                "source": workspace_host_path.to_string_lossy(),
                "options": ["rbind", "rw"]
            }
        ],
        "linux": {
            "namespaces": [
                { "type": "pid" },
                { "type": "mount" },
                { "type": "ipc" },
                { "type": "uts" },
                { "type": "network" }
            ],
            "resources": {
                "pids": {
                    "limit": pids_limit
                }
            }
        }
    });

    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_generate_spec() {
        let spec = generate_spec(
            Language::Python,
            &ResourceLimits::default(),
            &PathBuf::from("/var/lib/sandcastle/rootfs/python"),
            &PathBuf::from("/tmp/workspace"),
            "/sandbox/executor",
            &HashMap::new(),
        )
        .unwrap();

        assert_eq!(spec["ociVersion"], "1.0.2");
        assert_eq!(spec["process"]["args"][0], "/sandbox/executor");
        assert_eq!(spec["root"]["path"], "/var/lib/sandcastle/rootfs/python");

        let namespaces = spec["linux"]["namespaces"].as_array().unwrap();
        assert_eq!(namespaces.len(), 5);
    }
}
