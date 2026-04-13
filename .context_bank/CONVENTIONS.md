# Conventions & Rules

Coding standards, build commands, and workflow rules for this project.

---

## Absolute Rules (Non-Negotiable)

### 1. No Shell Commands in Rust Host Code
**NEVER** use `std::process::Command`, `Command::new()`, or any shell command execution in Rust code that runs on the host. Always use Rust crates for functionality.

**Exceptions**:
- The executor binary (`sandcastle-executor`) runs INSIDE the sandbox and IS allowed to use `Command::new()` to spawn language runtimes. This is safe because it's already isolated.
- The `sandcastle-gvisor::runsc` module (`runsc.rs`) is allowed to use `tokio::process::Command` to invoke the `runsc` CLI. No other module may spawn host commands.
- The `firepilot` crate (external dependency) internally spawns the Firecracker binary. This is an unavoidable library behavior, not our code.

### 2. Git Workflow — Always Use PRs
- **Never push directly to `main`**. Always create a feature branch and raise a PR.
- Branch naming: `feat/description`, `fix/description`, `refactor/description`
- Include `Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>` in commit messages when AI-assisted.

### 3. Static Executor Binary
The executor binary MUST be statically linked with musl:
```bash
# For containers (stdio mode)
cargo build -p sandcastle-executor --target x86_64-unknown-linux-musl

# For Firecracker VMs (vsock mode)
cargo build -p sandcastle-executor --target x86_64-unknown-linux-musl --features vsock-mode
```
A dynamically linked executor will die inside containers/VMs due to glibc version mismatch.

---

## Build Commands

```bash
# All commands run from service/ directory unless noted

# Build all crates
cargo build

# Build in release mode
cargo build --release

# Build static executor (REQUIRED for containers)
cargo build -p sandcastle-executor --target x86_64-unknown-linux-musl

# Run all unit tests (27 tests, no root needed)
cargo test

# Run only lib tests (no integration tests)
cargo test --lib

# Run specific crate tests
cargo test -p sandcastle-runtime
cargo test -p sandcastle-manager
cargo test -p sandcastle-process

# Run e2e test (requires root + rootfs images)
sudo $(which cargo) test -p sandcastle-process --test e2e -- --nocapture

# Run gVisor e2e test (requires root + runsc + rootfs images)
sudo $(which cargo) test -p sandcastle-gvisor --test e2e -- --nocapture

# Run Firecracker e2e test (requires root + KVM + firecracker + kernel + ext4 rootfs)
sudo $(which cargo) test -p sandcastle-firecracker --test e2e -- --nocapture

# Build rootfs images (requires Docker + root, run from repo root)
sudo ./scripts/build-rootfs.sh

# Build ext4 rootfs images for Firecracker (run from repo root)
sudo ./scripts/build-fc-rootfs.sh

# Run shell-based integration tests (7 tests, from repo root)
sudo ./tests/integration.sh
```

---

## Code Style

- **Rust edition**: 2021
- **Error handling**: Use `thiserror` for error enums, `anyhow` in binaries
- **Async**: `tokio` with full features, `async-trait` for trait objects
- **Logging**: `tracing` crate (not `log`), logs to stderr
- **Serialization**: `serde` + `serde_json` everywhere
- **Comments**: Only add comments that clarify non-obvious logic. Don't comment obvious code.
- **Tests**: Unit tests in `#[cfg(test)] mod tests` within source files. Integration tests in `tests/` directory.

---

## Workspace Dependencies

Shared dependency versions are declared in the workspace `Cargo.toml` and referenced with `{ workspace = true }` in crate `Cargo.toml` files:

```toml
[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
async-trait = "0.1"
thiserror = "2"
tracing = "0.1"
tracing-subscriber = "0.3"
uuid = { version = "1", features = ["v4"] }
clap = { version = "4", features = ["derive"] }
toml = "0.8"
anyhow = "1"
```

---

## Key Dependency Versions (Verified Working)

| Crate | Version | Notes |
|-------|---------|-------|
| rmcp | 1 (v1.4) | Uses `schemars` v1.0, NOT v0.8 |
| schemars | 1.0 | Must be added as direct dep if using rmcp |
| libcontainer | 0.6.0 | Features: `v2`, `systemd` (required) |
| oci-spec | 0.9.0 | Must match libcontainer's internal version |
| nix | 0.29 | Features: `fs`, `process`, `signal` |

---

## File System Layout (Runtime)

```
/var/lib/sandcastle/
├── rootfs/                 # Pre-built per-language root filesystems
│   ├── python/             # python:3.12-slim Docker export (for containers)
│   │   └── sandbox/executor  # Static executor binary (stdio mode)
│   ├── javascript/         # node:20-slim Docker export
│   │   └── sandbox/executor
│   ├── bash/               # bash:5 Docker export
│   │   └── sandbox/executor
│   ├── python.ext4         # Ext4 block device (for Firecracker VMs)
│   ├── javascript.ext4     # Contains vsock-mode executor at /sandbox/executor
│   └── bash.ext4
├── kernel/
│   └── vmlinux             # Linux 6.1 kernel for Firecracker VMs
├── bundles/                # Per-container OCI bundles (temporary)
│   └── sc-{uuid}/
│       ├── config.json     # OCI runtime spec
│       └── rootfs → symlink to /var/lib/sandcastle/rootfs/{lang}
├── workspaces/             # Per-container workspace dirs (bind-mounted)
│   └── sc-{uuid}/
├── gvisor/                 # gVisor-specific directories
│   ├── bundles/
│   └── workspaces/
├── firecracker/            # Firecracker-specific directories
│   ├── {vm-id}/            # Per-VM state (firepilot chroot)
│   └── workspaces/
│       └── fc-{uuid}/
└── bin/
    └── executor            # Canonical executor binary location

/run/sandcastle/            # Container runtime state (libcontainer)
├── sc-{uuid}/              # Per-container state (created by libcontainer)
└── gvisor/                 # gVisor runtime state (created by runsc)
    └── gv-{uuid}/
```

---

## Naming Conventions

| Entity | Pattern | Example |
|--------|---------|---------|
| Container/sandbox ID (low) | `sc-{uuid_simple}` | `sc-a1b2c3d4e5f6...` |
| Container/sandbox ID (medium) | `gv-{uuid_simple}` | `gv-a1b2c3d4e5f6...` |
| VM/sandbox ID (high) | `fc-{uuid_simple}` | `fc-a1b2c3d4e5f6...` |
| Session ID | `sb-{uuid_v4}` | `sb-12345678-1234-...` |
| Rootfs language dir | lowercase language name | `python`, `javascript`, `bash` |
| Ext4 rootfs image | `{language}.ext4` | `python.ext4`, `bash.ext4` |
| Bundle dir | Same as container ID | `/var/lib/sandcastle/bundles/sc-...` |
| Workspace dir | Same as container ID | `/var/lib/sandcastle/workspaces/sc-...` |

---

## Adding a New Language

1. Add variant to `Language` enum in `sandcastle-runtime/src/types.rs`
2. Implement `extension()` and `runtime_binary()` for the new variant
3. Add `Display` impl format string
4. Add case to `parse_language()` in `sandcastle-server/src/tools.rs`
5. Add case to executor's `execute_code()` in `sandcastle-executor/src/main.rs`
6. Add rootfs build line to `scripts/build-rootfs.sh`
7. Build rootfs: `sudo ./scripts/build-rootfs.sh`

---

## Adding a New Backend

1. Create new crate: `service/crates/sandcastle-{name}/`
2. Add to workspace members in `service/Cargo.toml`
3. Implement `SandboxRuntime` trait from `sandcastle-runtime`
4. Wire it up in `sandcastle-server/src/main.rs` (replace or select alongside ProcessSandbox)
5. The manager and server code should NOT need changes — they're backend-agnostic

---

## Git Identity

- Name: `pruthvirajdgit`
- Email: `pruthvirajdgit@users.noreply.github.com`

---

## Copilot MCP Integration

Sandcastle is configured as a Copilot CLI MCP server via:

**Config file**: `~/.copilot/mcp-config.json`

```json
{
  "mcpServers": {
    "sandcastle": {
      "type": "stdio",
      "command": "sudo",
      "args": [
        "/home/azureuser/sandcastle/service/target/debug/sandcastle",
        "serve", "--transport", "stdio"
      ]
    }
  }
}
```

**Activation**: After editing the config, run `/restart` in Copilot CLI or start a new session.

**Verification**: Run `/mcp` in Copilot CLI — should show `sandcastle` as connected with 6 tools.

**Note**: The server requires root (`sudo`) because container creation needs root privileges. Ensure passwordless sudo is configured or the binary has appropriate capabilities.
