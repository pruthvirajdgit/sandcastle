///! Integration test: GvisorSandbox end-to-end.
///! Requires root, runsc installed, and pre-built rootfs (scripts/build-rootfs.sh).
///! Run: sudo cargo test -p sandcastle-gvisor --test e2e -- --nocapture

use std::time::Duration;

use sandcastle_gvisor::{GvisorConfig, GvisorSandbox};
use sandcastle_runtime::{
    ExecRequest, IsolationLevel, Language, ResourceLimits, SandboxConfig, SandboxRuntime,
};

fn gvisor_config() -> GvisorConfig {
    GvisorConfig {
        runsc_path: "/usr/local/bin/runsc".into(),
        rootfs_dir: "/var/lib/sandcastle/rootfs".into(),
        state_dir: "/run/sandcastle-gvisor-test".into(),
        bundle_dir: "/tmp/sandcastle-gvisor-test/bundles".into(),
        workspace_dir: "/tmp/sandcastle-gvisor-test/workspaces".into(),
        executor_path: "/var/lib/sandcastle/rootfs/python/sandbox/executor".into(),
        platform: "ptrace".to_string(),
    }
}

#[tokio::test]
async fn test_gvisor_python_hello_world() {
    // Requires root and runsc
    if !nix::unistd::geteuid().is_root() {
        eprintln!("Skipping gVisor e2e test: must run as root (sudo)");
        return;
    }

    let config = gvisor_config();
    let sandbox = GvisorSandbox::new(config);

    if !sandbox.is_available() {
        eprintln!("Skipping gVisor e2e test: runsc not found");
        return;
    }

    sandbox.ensure_dirs().expect("ensure dirs");

    let sandbox_config = SandboxConfig {
        language: Language::Python,
        isolation: IsolationLevel::Medium,
        limits: ResourceLimits::default(),
        env_vars: Default::default(),
    };

    // Create
    println!("Creating gVisor sandbox...");
    let id = sandbox
        .create(&sandbox_config)
        .await
        .expect("create failed");
    println!("Created gVisor sandbox: {id}");

    // Start
    println!("Starting gVisor sandbox...");
    sandbox.start(&id).await.expect("start failed");
    println!("gVisor sandbox running.");

    // Execute Python code
    println!("Executing Python code in gVisor...");
    let request = ExecRequest {
        code: "print('Hello from gVisor Sandcastle!')".to_string(),
        timeout: Duration::from_secs(30),
    };
    let result = sandbox
        .execute(&id, &request)
        .await
        .expect("execute failed");
    println!("Result: {:?}", result);

    assert_eq!(result.stdout.trim(), "Hello from gVisor Sandcastle!");
    assert_eq!(result.exit_code, 0);
    assert!(!result.timed_out);
    assert!(!result.oom_killed);

    // Execute more complex Python
    println!("Executing complex Python in gVisor...");
    let request2 = ExecRequest {
        code: r#"
import json
data = {"source": "gvisor", "numbers": [i**2 for i in range(5)]}
print(json.dumps(data))
"#
        .to_string(),
        timeout: Duration::from_secs(30),
    };
    let result2 = sandbox
        .execute(&id, &request2)
        .await
        .expect("execute2 failed");
    println!("Result2: {:?}", result2);

    assert_eq!(result2.exit_code, 0);
    let parsed: serde_json::Value =
        serde_json::from_str(result2.stdout.trim()).expect("parse JSON output");
    assert_eq!(parsed["source"], "gvisor");
    assert_eq!(parsed["numbers"], serde_json::json!([0, 1, 4, 9, 16]));

    // Stop and destroy
    println!("Stopping gVisor sandbox...");
    sandbox.stop(&id).await.expect("stop failed");
    println!("Destroying gVisor sandbox...");
    sandbox.destroy(&id).await.expect("destroy failed");
    println!("gVisor e2e test passed! ✅");
}
