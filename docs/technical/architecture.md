# Sandcastle Architecture

## Overview

Sandcastle is a Rust-based MCP server that provides sandboxed code execution for AI agents. It currently implements process-level isolation via Linux namespaces (libcontainer/youki) and exposes 6 MCP tools. The architecture supports pluggable backends — gVisor and Firecracker backends are planned for future phases.

> **Note**: For detailed code-level architecture, crate APIs, and implementation details, see the `.context_bank/` directory (especially `ARCHITECTURE.md` and `CRATE_REFERENCE.md`). This document covers the high-level design.

## System Components

### 1. MCP Server Layer

The entry point. Handles MCP protocol (stdio or HTTP transport), parses tool calls, and routes them to the Sandbox Manager.

```
MCP Request → Parse Tool Call → Validate Inputs → Route to Sandbox Manager → Return Result
```

**Responsibilities:**
- MCP protocol handling (stdio for local agents, HTTP/SSE for remote)
- Input validation (language, timeout bounds, memory limits)
- Authentication (API key for hosted mode)
- Rate limiting
- Request/response serialization

### 2. Sandbox Manager

Central coordinator. Maintains three sandbox pools (one per isolation level) and handles lifecycle.

```rust
trait SandboxBackend {
    async fn create(&self, config: SandboxConfig) -> Result<SandboxId>;
    async fn execute(&self, id: &SandboxId, code: &str, timeout: Duration) -> Result<ExecutionResult>;
    async fn upload_file(&self, id: &SandboxId, host_path: &Path, sandbox_path: &str) -> Result<FileInfo>;
    async fn download_file(&self, id: &SandboxId, sandbox_path: &str, host_path: &Path) -> Result<FileInfo>;
    async fn destroy(&self, id: &SandboxId) -> Result<()>;
}
```

Three implementations:
- `NamespaceSandbox` (low)
- `GVisorSandbox` (medium)
- `FirecrackerSandbox` (high)

**Pool Management:**
- Each isolation level maintains a pool of pre-warmed sandboxes
- When an `execute_code` request arrives, grab a sandbox from the pool, run code, return to pool (or destroy if one-shot)
- Background replenisher keeps the pool at target size
- Idle sandboxes are destroyed after configurable timeout

### 3. Low Isolation — Linux Namespaces

Uses Linux kernel namespaces + seccomp + cgroups directly. No external runtime needed.

```
Host Process
  └── clone(CLONE_NEWNS | CLONE_NEWPID | CLONE_NEWNET | CLONE_NEWUSER)
        └── seccomp filter (block dangerous syscalls)
        └── cgroup limits (CPU, memory, disk I/O)
        └── pivot_root to minimal rootfs
        └── exec(language runtime)
```

**Namespaces used:**
- `CLONE_NEWPID` — isolated process tree (can't see/kill host processes)
- `CLONE_NEWNS` — isolated mount namespace (can't see host filesystem)
- `CLONE_NEWNET` — empty network namespace (no network by default)
- `CLONE_NEWUSER` — unprivileged user mapping
- `CLONE_NEWUTS` — isolated hostname

**Seccomp profile:**
- Allowlist of ~100 safe syscalls
- Block: `ptrace`, `mount`, `reboot`, `kexec_load`, `init_module`, `pivot_root`
- Block: raw socket creation, `keyctl`, BPF

**Cgroups v2:**
- `memory.max` — hard memory limit
- `cpu.max` — CPU time quota
- `pids.max` — max processes (prevent fork bombs)
- `io.max` — disk I/O bandwidth limit

**Boot time:** ~5ms (just clone + exec)

### 4. Medium Isolation — gVisor

Uses Google's gVisor (`runsc`) as an OCI runtime. gVisor interposes a userspace kernel (Sentry) between the guest application and the host kernel.

```
Host Kernel
  └── runsc (gVisor runtime)
        └── Sentry (userspace kernel — intercepts all guest syscalls)
              └── Gofer (filesystem proxy)
              └── Guest Application
```

**How it works:**
- Guest application makes a syscall (e.g., `open()`)
- Sentry intercepts it (via ptrace or KVM-based platform)
- Sentry re-implements the syscall in userspace (Go code)
- Only ~70 host syscalls are ever made by Sentry itself
- Guest never touches the real kernel

**Network isolation:**
- gVisor has its own network stack (netstack)
- Default: no network interfaces created
- Allowlisted domains: create a virtual interface, use iptables on host to restrict

**OCI integration:**
- We create OCI bundles (config.json + rootfs) for each sandbox
- `runsc create` → `runsc start` → `runsc exec` → `runsc kill` → `runsc delete`

**Boot time:** ~50ms (Sentry initialization)

### 5. High Isolation — Firecracker

Uses Firecracker microVMs via KVM. Each sandbox is a full VM with its own kernel.

```
Host Kernel (KVM)
  └── Firecracker Process
        └── Guest Kernel (minimal Linux)
              └── Guest Init
                    └── Executor Process
```

**How it works:**
- Pre-warmed VMs with snapshot restore (~250ms)
- Communication via vsock (no TCP/IP needed)
- Host sends code over vsock → executor runs it → returns stdout/stderr over vsock
- VM is destroyed after use (or returned to pool if persistent session)

**Snapshot pool:**
- Boot a "golden" VM with each language runtime installed
- Snapshot it (CPU state + RAM dump)
- Restore copies of this snapshot for each execution request
- Restore time: ~250ms vs ~4s cold boot

**Network isolation:**
- No TAP device by default (VM has no network)
- If allowed_domains specified: create TAP + iptables rules restricting outbound to those IPs only

**Boot time:** ~250ms (snapshot restore)

## Data Flow

### One-shot `execute_code`

```
Agent → MCP → Sandbox Manager
                    │
                    ├── Get sandbox from pool (or create new)
                    ├── Inject code via pipe/vsock
                    ├── Wait for result (with timeout)
                    ├── Collect stdout/stderr/exit_code
                    ├── Destroy sandbox (or return to pool)
                    │
                    └── Return ExecutionResult to agent
```

### Session-based execution

```
Agent → create_sandbox → Manager creates sandbox, returns sandbox_id
Agent → execute_in_sandbox(sandbox_id, code1) → runs, returns result1
Agent → upload_file(sandbox_id, host_path="/data/input.csv", sandbox_path="input.csv")
       → copies file from host into sandbox /workspace/input.csv
Agent → execute_in_sandbox(sandbox_id, code2) → runs (can read input.csv), returns result2
Agent → download_file(sandbox_id, sandbox_path="output.json")
       → copies file from sandbox to host {output_dir}/{sandbox_id}/output.json
Agent → destroy_sandbox(sandbox_id) → cleanup
```

## Network Allowlisting

When `allowed_domains` is specified:

1. Resolve domain to IP addresses (with periodic re-resolution)
2. Create network interface in sandbox
3. Apply iptables/nftables rules:
   ```
   -A OUTPUT -d <resolved_ip> -p tcp --dport 443 -j ACCEPT
   -A OUTPUT -d <resolved_ip> -p tcp --dport 80 -j ACCEPT
   -A OUTPUT -j DROP
   ```
4. DNS is handled by a host-side proxy that only resolves allowlisted domains

## Resource Limits

| Resource | Default | Min | Max |
|----------|---------|-----|-----|
| Memory | 512 MB | 64 MB | 8 GB |
| CPU cores | 1 | 1 | 4 |
| Timeout | 30s | 1s | 600s |
| Disk | 1 GB | 100 MB | 10 GB |
| Max processes | 64 | 1 | 256 |
| Max output size | 1 MB | — | 10 MB |

## Sandbox Images

Each language has a minimal rootfs/container image:

```
base-image/
├── /usr/bin/{python3,node,bash,...}  — language runtime
├── /usr/lib/                        — shared libraries
├── /workspace/                      — user working directory
├── /tmp/                            — temp files
└── /sandcastle/executor             — our executor binary
```

The executor binary:
- Receives code via stdin/pipe/vsock
- Sets up working directory
- Runs the code as a subprocess
- Captures stdout/stderr
- Enforces timeout (kills child if exceeded)
- Returns structured result

## Pre-warming Strategy

```
Pool Configuration:
  low:
    target_size: 20        # keep 20 ready
    max_size: 100          # hard cap
    idle_timeout: 300s     # destroy idle after 5min
    replenish_threshold: 5 # replenish when pool drops below 5
  
  medium:
    target_size: 10
    max_size: 50
    idle_timeout: 300s
    replenish_threshold: 3
  
  high:
    target_size: 5
    max_size: 20
    idle_timeout: 600s
    replenish_threshold: 2
```

Background replenisher runs every second:
1. Check pool size for each level
2. If below threshold, create new sandboxes up to target_size
3. For high isolation: restore from snapshots (fast)
4. For medium: `runsc create` new containers
5. For low: pre-fork processes (near instant, so pool is less critical)

## Configuration

```toml
# sandcastle.toml

[server]
transport = "stdio"          # "stdio" or "http"
port = 8090                  # only for HTTP transport
api_key = ""                 # optional auth for hosted mode

[defaults]
isolation = "medium"
timeout_seconds = 30
memory_mb = 512
cpu_cores = 1
language = "python"

[files]
allowed_input_dirs = ["/data", "/tmp/sandcastle-input"]
output_dir = "/tmp/sandcastle-output"
max_file_size_bytes = 10485760    # 10 MB

[pool.low]
target_size = 20
max_size = 100
idle_timeout_seconds = 300

[pool.medium]
target_size = 10
max_size = 50
idle_timeout_seconds = 300

[pool.high]
target_size = 5
max_size = 20
idle_timeout_seconds = 600

[firecracker]
binary = "/usr/local/bin/firecracker"
kernel = "/opt/sandcastle/vmlinux"
snapshot_dir = "/var/lib/sandcastle/snapshots"

[gvisor]
runsc_binary = "/usr/local/bin/runsc"
rootfs_dir = "/var/lib/sandcastle/rootfs"

[network]
default = "none"                    # "none" or "restricted"
dns_proxy_port = 5353
max_allowed_domains = 10

[limits]
max_memory_mb = 8192
max_timeout_seconds = 600
max_disk_mb = 10240
max_output_bytes = 10485760         # 10 MB
```

## Tech Stack

- **Language**: Rust
- **MCP SDK**: `mcp-rust-sdk` (or custom implementation)
- **Async runtime**: Tokio
- **Firecracker SDK**: fctools
- **gVisor**: runsc CLI (OCI runtime)
- **Namespaces**: nix crate (clone, mount, seccomp)
- **Cgroups**: cgroups-rs or direct /sys/fs/cgroup writes
- **Networking**: rtnetlink (TAP/veth management), nftables

## Directory Structure

```
sandcastle/
├── Cargo.toml
├── src/
│   ├── main.rs                  # CLI entry point
│   ├── mcp/
│   │   ├── server.rs            # MCP protocol handler
│   │   ├── tools.rs             # Tool definitions
│   │   └── transport.rs         # stdio / HTTP transport
│   ├── manager/
│   │   ├── mod.rs               # SandboxManager
│   │   ├── pool.rs              # Pool management + replenisher
│   │   └── config.rs            # SandboxConfig types
│   ├── backends/
│   │   ├── mod.rs               # SandboxBackend trait
│   │   ├── namespace.rs         # Low isolation
│   │   ├── gvisor.rs            # Medium isolation
│   │   └── firecracker.rs       # High isolation
│   ├── executor/
│   │   └── mod.rs               # In-sandbox executor protocol
│   ├── network/
│   │   ├── mod.rs               # Network setup
│   │   ├── allowlist.rs         # Domain → IP resolution + rules
│   │   └── dns_proxy.rs         # DNS proxy for allowlisted domains
│   └── images/
│       └── mod.rs               # Rootfs/image management
├── executor/                    # Separate binary compiled into sandbox images
│   ├── Cargo.toml
│   └── src/main.rs
├── images/                      # Dockerfiles for sandbox images
│   ├── python.Dockerfile
│   ├── node.Dockerfile
│   └── base.Dockerfile
├── docs/
│   ├── architecture.md
│   └── security.md
├── tests/
│   ├── integration/
│   └── e2e/
└── sandcastle.toml              # Default config
```
