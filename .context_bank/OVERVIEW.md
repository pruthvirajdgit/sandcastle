# Sandcastle — Context Bank

> **Source of truth** for AI agents working on this codebase.
> Read this directory before making any changes.

## What is Sandcastle?

Sandcastle is an **MCP (Model Context Protocol) server** that provides sandboxed code execution for AI agents. An AI agent sends `execute_code` over MCP → Sandcastle runs the code inside an isolated Linux container → returns stdout/stderr/exit_code.

## Repository Layout

```
sandcastle/
├── .context_bank/          # ← YOU ARE HERE — architecture & design docs
│   ├── OVERVIEW.md         # This file — start here
│   ├── ARCHITECTURE.md     # System design, crate structure, data flow
│   ├── CRATE_REFERENCE.md  # Per-crate API reference and key types
│   ├── CONVENTIONS.md      # Coding rules, build commands, workflow
│   └── KNOWN_ISSUES.md     # Gotchas, libcontainer quirks, lessons learned
├── docs/                   # User-facing documentation
├── scripts/
│   └── build-rootfs.sh     # Builds per-language container root filesystems
├── service/                # All Rust backend code (Cargo workspace)
│   ├── Cargo.toml          # Workspace manifest
│   └── crates/
│       ├── sandcastle-runtime/    # Shared trait + types (the interface)
│       ├── sandcastle-executor/   # Binary that runs INSIDE the container
│       ├── sandcastle-manager/    # Session lifecycle, file transfer
│       ├── sandcastle-process/    # Linux container backend (libcontainer)
│       ├── sandcastle-gvisor/     # gVisor container backend (runsc CLI)
│       └── sandcastle-server/     # MCP server binary (entry point)
├── tests/
│   └── integration.sh      # Shell-based integration tests (6 tests)
├── README.md               # Product overview and MCP tool reference
└── .gitignore
```

## Current Status

### Phase 1 — Foundation ✅ Complete
- ✅ 5-crate Rust workspace compiles and passes all tests (15 unit tests)
- ✅ ProcessSandbox runs Python/JS/Bash in namespace-isolated containers via libcontainer
- ✅ MCP server exposes 6 tools: execute_code, create_sandbox, execute_in_session, upload_file, download_file, destroy_sandbox
- ✅ E2E integration test verified end-to-end

### Phase 2 — MCP Integration & Validation ✅ Complete
- ✅ MCP server connected to GitHub Copilot CLI as a native tool provider
- ✅ Live code execution verified: Python runs inside containers via MCP tool calls
- ✅ File transfer pipeline working end-to-end (upload → execute → download)
- ✅ Persistent sessions working (create_sandbox → execute_in_session → destroy_sandbox)
- ✅ Copilot MCP config at `~/.copilot/mcp-config.json`
- ✅ Static musl executor binary for container compatibility
- ✅ Rootfs images built for Python 3.12, Node 20, Bash 5

### Phase 3 — gVisor Backend (Medium Isolation) ✅ Complete
- ✅ `sandcastle-gvisor` crate with GvisorSandbox implementing SandboxRuntime
- ✅ IsolationLevel enum (Low/Medium/High) with per-request routing
- ✅ Manager refactored: `HashMap<IsolationLevel, Arc<dyn SandboxRuntime>>` for multi-backend
- ✅ MCP tools accept `isolation` parameter ("low", "medium") — defaults to "low"
- ✅ Server registers both backends: ProcessSandbox (Low) + GvisorSandbox (Medium)
- ✅ Graceful degradation when runsc not installed
- ✅ 23 unit tests + 6 integration tests (including gVisor e2e and routing tests)
- ✅ runsc installed (release-20260406.0, ptrace platform)

### Upcoming
- ⬜ Firecracker microVM backend (Phase 4) — hardware virtualization for high isolation
- ⬜ Network allowlisting with DNS proxy
- ⬜ Pre-warmed sandbox pools

## Quick Commands

```bash
# Build everything
cd service && cargo build

# Run all unit tests (23 tests)
cd service && cargo test

# Run e2e integration test for ProcessSandbox (requires root + rootfs images)
cd service && sudo $(which cargo) test -p sandcastle-process --test e2e -- --nocapture

# Run e2e integration test for GvisorSandbox (requires root + runsc + rootfs images)
cd service && sudo $(which cargo) test -p sandcastle-gvisor --test e2e -- --nocapture

# Run full integration test suite (6 tests, requires root + rootfs + runsc)
sudo ./tests/integration.sh

# Build static executor for containers
cd service && cargo build -p sandcastle-executor --target x86_64-unknown-linux-musl

# Build rootfs images (requires Docker + root)
sudo ./scripts/build-rootfs.sh

# Start MCP server (requires root for container creation)
sudo service/target/debug/sandcastle serve --transport stdio

# Configure as Copilot MCP server
# Config file: ~/.copilot/mcp-config.json
# After adding config, run /restart in Copilot CLI to pick it up
```
