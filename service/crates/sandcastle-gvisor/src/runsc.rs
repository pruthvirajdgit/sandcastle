//! runsc CLI wrapper — the ONLY module in Sandcastle that uses std::process::Command.
//! All gVisor/runsc interactions are isolated here.

use std::path::Path;
use std::process::Stdio;
use tracing::{debug, warn};
use tokio::process::{Child, Command};

/// Spawn `runsc run` as a long-lived subprocess.
/// Returns the Child process with stdin/stdout pipes.
/// The container starts immediately and the executor binary runs as PID 1.
pub async fn spawn_run(
    runsc_path: &Path,
    state_dir: &Path,
    bundle_dir: &Path,
    container_id: &str,
    platform: &str,
) -> Result<Child, String> {
    let child = Command::new(runsc_path)
        .arg("--root")
        .arg(state_dir)
        .arg("--platform")
        .arg(platform)
        .arg("run")
        .arg("--bundle")
        .arg(bundle_dir)
        .arg(container_id)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null()) // redirect runsc logs away from executor JSON
        .spawn()
        .map_err(|e| format!("failed to spawn runsc run: {e}"))?;

    debug!("spawned runsc run for container {container_id} (pid={})", child.id().unwrap_or(0));
    Ok(child)
}

/// Kill a container via `runsc kill`.
pub async fn kill(
    runsc_path: &Path,
    state_dir: &Path,
    container_id: &str,
) -> Result<(), String> {
    let output = Command::new(runsc_path)
        .arg("--root")
        .arg(state_dir)
        .arg("kill")
        .arg(container_id)
        .arg("SIGKILL")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("failed to run runsc kill: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!("runsc kill {container_id} failed: {stderr}");
        // Don't error — container may already be dead
    } else {
        debug!("runsc kill {container_id} sent SIGKILL");
    }

    Ok(())
}

/// Delete a container via `runsc delete --force`.
pub async fn delete(
    runsc_path: &Path,
    state_dir: &Path,
    container_id: &str,
) -> Result<(), String> {
    let output = Command::new(runsc_path)
        .arg("--root")
        .arg(state_dir)
        .arg("delete")
        .arg("--force")
        .arg(container_id)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("failed to run runsc delete: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!("runsc delete {container_id} failed: {stderr}");
    } else {
        debug!("runsc delete {container_id} completed");
    }

    Ok(())
}

/// Check if runsc is installed and accessible.
pub fn is_available(runsc_path: &Path) -> bool {
    std::process::Command::new(runsc_path)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
