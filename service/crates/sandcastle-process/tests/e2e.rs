///! Integration test: ProcessSandbox end-to-end.
///! Requires root and pre-built rootfs (scripts/build-rootfs.sh).
///! Run: sudo cargo test -p sandcastle-process --test e2e -- --nocapture

use std::time::Duration;

use sandcastle_process::{ProcessConfig, ProcessSandbox};
use sandcastle_runtime::{
    ExecRequest, Language, ResourceLimits, SandboxConfig, SandboxRuntime, SandboxStatus,
};

fn process_config() -> ProcessConfig {
    ProcessConfig {
        rootfs_dir: "/var/lib/sandcastle/rootfs".into(),
        state_dir: "/run/sandcastle-test".into(),
        bundle_dir: "/tmp/sandcastle-test/bundles".into(),
        workspace_dir: "/tmp/sandcastle-test/workspaces".into(),
        executor_path: "/var/lib/sandcastle/rootfs/python/sandbox/executor".into(),
    }
}

/// Cleanup guard: ensures sandbox is destroyed even if test panics.
struct SandboxGuard<'a> {
    sandbox: &'a ProcessSandbox,
    id: Option<sandcastle_runtime::SandboxId>,
}

impl<'a> Drop for SandboxGuard<'a> {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            eprintln!("cleanup: destroying sandbox {id}");
            // Use a new runtime to block on async cleanup in Drop
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let _ = rt.block_on(self.sandbox.stop(&id));
            let _ = rt.block_on(self.sandbox.destroy(&id));
        }
    }
}

#[tokio::test]
async fn test_python_hello_world() {
    // This test requires root privileges and pre-built rootfs images.
    if !nix::unistd::geteuid().is_root() {
        eprintln!("Skipping e2e test: must run as root (sudo)");
        return;
    }

    let config = process_config();
    let sandbox = ProcessSandbox::new(config);
    sandbox.ensure_dirs().expect("ensure dirs");

    let sandbox_config = SandboxConfig {
        language: Language::Python,
        isolation: sandcastle_runtime::IsolationLevel::Low,
        limits: ResourceLimits::default(),
        env_vars: Default::default(),
    };

    // Create
    println!("Creating sandbox...");
    let id = sandbox.create(&sandbox_config).await.expect("create failed");
    println!("Created sandbox: {id}");

    // Set up cleanup guard
    let mut guard = SandboxGuard {
        sandbox: &sandbox,
        id: Some(id.clone()),
    };

    // Check status
    let status = sandbox.status(&id).await.expect("status failed");
    assert_eq!(status, SandboxStatus::Created);

    // Start
    println!("Starting sandbox...");
    sandbox.start(&id).await.expect("start failed");
    let status = sandbox.status(&id).await.unwrap();
    assert_eq!(status, SandboxStatus::Running);
    println!("Sandbox running.");

    // Execute Python code
    println!("Executing Python code...");
    let request = ExecRequest {
        code: "print('Hello from Sandcastle!')".to_string(),
        timeout: Duration::from_secs(10),
    };
    let result = sandbox.execute(&id, &request).await.expect("execute failed");
    println!("Result: {:?}", result);

    assert_eq!(result.stdout.trim(), "Hello from Sandcastle!");
    assert_eq!(result.exit_code, 0);
    assert!(!result.timed_out);
    assert!(!result.oom_killed);

    // Execute more complex Python
    println!("Executing complex Python...");
    let request2 = ExecRequest {
        code: r#"
import json
data = {"numbers": [i**2 for i in range(5)], "message": "sandbox works"}
print(json.dumps(data))
"#
        .to_string(),
        timeout: Duration::from_secs(10),
    };
    let result2 = sandbox.execute(&id, &request2).await.expect("execute2 failed");
    println!("Result2: {:?}", result2);

    assert_eq!(result2.exit_code, 0);
    let parsed: serde_json::Value =
        serde_json::from_str(result2.stdout.trim()).expect("parse JSON output");
    assert_eq!(parsed["message"], "sandbox works");
    assert_eq!(parsed["numbers"], serde_json::json!([0, 1, 4, 9, 16]));

    // Clean shutdown (guard will handle cleanup if we panic before here)
    println!("Stopping sandbox...");
    sandbox.stop(&id).await.expect("stop failed");
    println!("Destroying sandbox...");
    sandbox.destroy(&id).await.expect("destroy failed");

    // Disarm guard — clean shutdown succeeded
    guard.id = None;
    println!("Done!");
}
