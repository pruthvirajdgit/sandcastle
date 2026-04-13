//! Integration test: FirecrackerSandbox end-to-end.
//! Requires root, KVM access, firecracker + kernel installed, and pre-built ext4 rootfs.
//! Run: sudo cargo test -p sandcastle-firecracker --test e2e -- --nocapture

use sandcastle_firecracker::{FirecrackerConfig, FirecrackerSandbox};
use sandcastle_runtime::{
    ExecRequest, IsolationLevel, Language, ResourceLimits, SandboxConfig, SandboxRuntime,
    SandboxStatus,
};
use std::time::Duration;

#[tokio::test]
async fn test_firecracker_python_hello_world() {
    // Skip if not root
    if !nix::unistd::Uid::effective().is_root() {
        eprintln!("SKIP: requires root");
        return;
    }

    let config = FirecrackerConfig::default();
    if !config.is_available() {
        eprintln!("SKIP: firecracker or kernel not available");
        return;
    }

    let sandbox = FirecrackerSandbox::new(config);
    sandbox.ensure_dirs().expect("failed to create dirs");

    let sandbox_config = SandboxConfig {
        language: Language::Python,
        isolation: IsolationLevel::High,
        limits: ResourceLimits::default(),
        env_vars: Default::default(),
    };

    // Create
    eprintln!("Creating Firecracker VM...");
    let id = sandbox.create(&sandbox_config).await.expect("create failed");
    eprintln!("Created: {}", id);

    // Start
    eprintln!("Starting VM...");
    sandbox.start(&id).await.expect("start failed");
    eprintln!("VM started");

    // Check status
    let status = sandbox.status(&id).await.expect("status failed");
    assert_eq!(status, SandboxStatus::Running);

    // Execute
    eprintln!("Executing Python code...");
    let result = sandbox
        .execute(
            &id,
            &ExecRequest {
                code: "print('hello from firecracker')".to_string(),
                timeout: Duration::from_secs(10),
            },
        )
        .await
        .expect("execute failed");

    eprintln!("Result: {:?}", result);
    assert_eq!(result.stdout.trim(), "hello from firecracker");
    assert_eq!(result.exit_code, 0);
    assert!(!result.timed_out);

    // Destroy
    eprintln!("Destroying VM...");
    sandbox.destroy(&id).await.expect("destroy failed");
    eprintln!("Done!");
}
