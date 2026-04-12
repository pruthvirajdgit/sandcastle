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
│       └── sandcastle-server/     # MCP server binary (entry point)
├── tests/
│   └── integration.sh      # Shell-based integration tests
├── README.md               # Product overview and MCP tool reference
└── .gitignore
```

## Current Status (Phase 1 — Complete)

- ✅ 5-crate Rust workspace compiles and passes all tests
- ✅ ProcessSandbox runs Python/JS/Bash in namespace-isolated containers
- ✅ MCP server exposes 6 tools: execute_code, create_sandbox, execute_in_session, upload_file, download_file, destroy_sandbox
- ✅ E2E test verified: Python code executes inside container and returns correct output
- ⬜ gVisor backend (Phase 2)
- ⬜ Firecracker microVM backend (Phase 3)

## Quick Commands

```bash
# Build everything
cd service && cargo build

# Run all unit tests (15 tests)
cd service && cargo test

# Run e2e integration test (requires root + rootfs images)
cd service && sudo $(which cargo) test -p sandcastle-process --test e2e -- --nocapture

# Build static executor for containers
cd service && cargo build -p sandcastle-executor --target x86_64-unknown-linux-musl

# Build rootfs images (requires Docker + root)
sudo ./scripts/build-rootfs.sh

# Start MCP server
cd service && cargo run -p sandcastle-server -- serve --transport stdio
```
