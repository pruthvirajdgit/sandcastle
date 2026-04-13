# Known Issues & Gotchas

Hard-won lessons from debugging. Read this before modifying container-related code.

---

## libcontainer (youki) v0.6.0

### 1. `systemd` feature is REQUIRED
On systems with systemd cgroup management, libcontainer MUST be compiled with the `systemd` feature. Without it:
```
Error: "systemd cgroup feature is required, but was not enabled during compilation"
```
**Fix**: `libcontainer = { version = "0.6.0", features = ["v2", "systemd"] }`

### 2. Do NOT add device bind mounts
libcontainer creates `/dev/null`, `/dev/zero`, `/dev/urandom`, etc. internally during rootfs preparation. If you add explicit bind mounts for these on top of a `/dev` tmpfs, you get:
```
Error: "failed to prepare rootfs"
```
**Fix**: Only provide `/dev` as a tmpfs mount. Do NOT bind-mount individual device nodes.

### 3. `Container::load()` takes a single PathBuf
```rust
// WRONG: Container::load(state_dir, container_id)
// RIGHT:
Container::load(state_dir.join(&container_id))
```
The argument is the full path to the container's state directory, not separate components.

### 4. OCI spec MUST include `user` field
The `ProcessBuilder` from `oci-spec` includes a `user` field by default. If you manually construct the process JSON without it, libcontainer fails with:
```
Error: "missing field `user`"
```

### 5. Minimal working namespace set
The minimum set of namespaces that works with libcontainer is **PID + Mount**. Adding IPC, UTS, Network namespaces is optional and may introduce issues depending on the host configuration.

### 6. `build()` blocks the thread
`ContainerBuilder::build()` forks a child process and blocks until the container init is ready (via a notify socket). Always call it inside `tokio::task::spawn_blocking()`.

### 7. Container deletion can be slow
`Container::delete(true)` (force delete) can take several seconds, especially if the container process is still running. The `kill()` → `delete()` sequence may hang briefly.

---

## Executor Binary

### 1. MUST be statically linked
The executor runs inside containers with Docker-exported rootfs images (e.g., `python:3.12-slim`). These have a different glibc version than the host. A dynamically linked executor becomes a zombie:
```
# Build static:
cargo build -p sandcastle-executor --target x86_64-unknown-linux-musl
```

### 2. musl target must be installed
```bash
rustup target add x86_64-unknown-linux-musl
```

### 3. Executor stays alive between exec calls
The executor reads stdin in a loop. Each `execute()` call from the host sends one JSON line and reads one JSON line back. The executor does NOT exit between executions — this is how persistent sessions work.

However, **Python/JS/Bash state does NOT persist** between calls. Each call writes a new code file and spawns a new language runtime process. Only files in `/workspace` persist.

---

## OCI Spec

### 1. Version must start with "1."
libcontainer validates that `ociVersion` starts with `"1."`. Use `"1.0.2"`.

### 2. `root.path` can be absolute
libcontainer canonicalizes the root path internally. Both relative and absolute paths work.

### 3. Don't use `LinuxPidsResourcesBuilder`
Use `LinuxPidsBuilder` instead:
```rust
// WRONG: LinuxPidsResourcesBuilder
// RIGHT:
LinuxPidsBuilder::default().limit(64).build()
```

---

## rmcp (MCP SDK)

### 1. schemars version mismatch
rmcp v1.4 re-exports `schemars` v1.0. If you add `schemars = "0.8"` as a dependency, you get:
```
Error: trait bound `JsonSchema` is not satisfied
```
**Fix**: Use `schemars = "1.0"` or use `rmcp::schemars::JsonSchema`.

### 2. `Parameters<T>` is a newtype
Access the inner value with `.0`, not `.into_inner()`:
```rust
async fn my_tool(&self, params: Parameters<MyParams>) -> String {
    let p = params.0;  // Not params.into_inner()
    // ...
}
```

### 3. Logs MUST go to stderr
The MCP server uses stdio for protocol communication. All logging MUST go to stderr:
```rust
tracing_subscriber::fmt()
    .with_writer(std::io::stderr)
    .init();
```

---

## Test Environment

### 1. E2E tests require root
Container creation requires root privileges. The e2e test checks for root and skips gracefully:
```rust
if !nix::unistd::geteuid().is_root() {
    eprintln!("Skipping e2e test: must run as root (sudo)");
    return;
}
```

### 2. `sudo cargo` doesn't find cargo
Use `sudo $(which cargo)` to preserve the cargo path:
```bash
sudo $(which cargo) test -p sandcastle-process --test e2e
```

### 3. Permission issues after running sudo
Running `cargo test` as root creates files owned by root in `target/`. Fix:
```bash
sudo chown -R $USER:$USER service/target
```

### 4. Stale container state
If a test crashes, container state may be left in `/run/sandcastle/`. Clean up:
```bash
sudo rm -rf /run/sandcastle-test /tmp/sandcastle-test
sudo rm -rf /run/sandcastle-gvisor-test /tmp/sandcastle-gvisor-test
```

---

## gVisor (runsc)

### 1. runsc stderr corrupts JSON protocol
runsc writes log output to stderr by default. If the GvisorSandbox doesn't redirect stderr to `Stdio::null()`, runsc logs get mixed into the JSON protocol stream on stdout, corrupting executor responses.
**Fix**: Always set `.stderr(Stdio::null())` on the runsc Command.

### 2. Workspace directories need chmod 777
Inside gVisor, the container process may run with different UID mappings. The workspace directory (bind-mounted from host) needs `chmod 777` for the executor to write code files and output.
**Fix**: `std::fs::set_permissions(workspace_dir, Permissions::from_mode(0o777))` in `create()`.

### 3. runsc run = create + start + wait
Unlike libcontainer which separates `create()` and `start()`, runsc's `run` command does both in one shot. This means the GvisorSandbox's `create()` only preps the bundle — the actual container spawn happens in `start()`.

### 4. OCI spec version
Both ProcessSandbox (libcontainer) and GvisorSandbox (runsc) use OCI spec version `1.0.2`. This works correctly with the installed runsc version despite runsc supporting newer spec versions.

### 5. Executor readiness handshake
The executor emits `{"ready":true}` on stdout at startup. The host (`start()`) waits for this line before proceeding. This replaced the earlier fragile 500ms fixed sleep. If the executor fails to start, the host gets an immediate error instead of a delayed failure on first execute.

### 6. runsc state directory must be separate
runsc uses `--root /run/sandcastle/gvisor` for container state. This MUST be separate from libcontainer's `/run/sandcastle` to avoid conflicts between the two runtimes.

---

## Firecracker

### 1. PID 1 zombie reaping (deferred)
The executor runs as PID 1 inside the VM. It does not reap orphan grandchild processes. If a spawned language runtime process forks and the child exits before the parent, zombies accumulate. This is acceptable for short-lived VM sessions but should be addressed for long-running sessions.

### 2. Ext4 rootfs images are copied on each VM create
firepilot copies the entire ext4 drive image into the chroot directory on each `machine.create()`. For large images (300MB+ for JavaScript), this adds 1-2 seconds overhead. Potential future optimization: use overlayfs or copy-on-write snapshots.

### 3. Vsock connection requires retry loop
The VM boot process takes time — the executor inside the VM isn't ready immediately after `machine.start()`. The host must retry vsock connections with a timeout (default: 30s, 500ms intervals). A fixed sleep is fragile and should never be used.

### 4. Firecracker vsock UDS proxy protocol
Firecracker exposes guest vsock via a Unix domain socket. The host MUST follow this protocol:
1. Connect to the UDS file
2. Send `CONNECT <port>\n`
3. Receive `OK <port>\n`
4. Then bidirectional JSON streaming works

If you skip the CONNECT handshake, the connection silently fails.

### 5. Each vsock connection is one-shot
Every `execute()` call opens a new vsock connection (CONNECT → OK → JSON request → JSON response). The connection is NOT persistent across execute calls (unlike container pipes which stay open).

### 6. File transfer uses base64 over vsock
Unlike containers where files are directly copied to the bind-mounted workspace directory, Firecracker VMs require file transfer over vsock using base64 encoding. This means:
- Large files are slower (base64 overhead + serialization)
- File size is limited by available memory for the base64 buffer
- Path traversal protection is enforced inside the executor

### 7. Kernel path must exist
FirecrackerConfig checks `is_available()` for both the firecracker binary and the kernel file. If either is missing, the backend is not registered and `IsolationLevel::High` returns `UnsupportedIsolation`.

### 8. KVM required
Firecracker requires `/dev/kvm` with Intel VT-x or AMD-V. On Azure VMs, ensure you're using a v3/v4 series with nested virtualization enabled. Check: `ls /dev/kvm`.

### 9. Stale VM state cleanup
If a test crashes, Firecracker processes and state directories may persist. Clean up:
```bash
# Kill any orphan Firecracker processes
ps aux | grep firecracker
sudo kill <pid>

# Clean up state directories
sudo rm -rf /var/lib/sandcastle/firecracker/fc-*
sudo rm -rf /tmp/sandcastle-fc-test*
```

---

## General

### 1. File transfers use bind mounts, not /proc
File upload/download works by directly copying files to/from the workspace directory on the host, which is bind-mounted into the container at `/workspace`. This is simpler and more reliable than using `/proc/pid/root`.

### 2. Don't modify shared rootfs
The rootfs directories (`/var/lib/sandcastle/rootfs/{lang}/`) are shared across all containers of the same language. Never write to them at runtime. All per-container state goes in the workspace bind mount.

### 3. Workspace directory is the only writable shared area
The `/workspace` directory (bind-mounted from host) is the only path where:
- Code files are written by the executor
- Users can upload/download files
- State persists across execute calls within a session
