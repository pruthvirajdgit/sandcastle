# Phase 1 — Technical Specification ✅ COMPLETE

## Status

**Phase 1 is fully implemented and verified.** All components below are working end-to-end.

## Scope

Build the core Sandcastle binary with:
- MCP server (stdio transport)
- Low isolation backend (Linux namespaces + seccomp + cgroups)
- All 6 MCP tools (execute_code, create_sandbox, execute_in_sandbox, upload_file, download_file, destroy_sandbox)
- Three languages (Python, JavaScript, Bash)
- Resource limits (CPU, memory, timeout, disk, processes)
- Network-zero by default
- Host-path based file upload/download
- CLI for local testing

**Not in scope:** gVisor, Firecracker, network allowlisting, pre-warmed pools, HTTP transport, malware scanning.

## Architecture (Phase 1)

```
┌──────────────────────────┐
│   AI Agent (via stdio)   │
└──────────┬───────────────┘
           │ MCP Protocol (JSON-RPC over stdin/stdout)
┌──────────▼───────────────┐
│   Sandcastle Binary      │
│                          │
│  ┌────────────────────┐  │
│  │   MCP Server       │  │
│  │   (stdio transport)│  │
│  └────────┬───────────┘  │
│           │              │
│  ┌────────▼───────────┐  │
│  │  Sandbox Manager   │  │
│  │  (HashMap<Id,Box>) │  │
│  └────────┬───────────┘  │
│           │              │
│  ┌────────▼───────────┐  │
│  │ NamespaceSandbox   │  │
│  │ (clone + seccomp   │  │
│  │  + cgroups)        │  │
│  └────────────────────┘  │
└──────────────────────────┘
```

## Crate Structure

```
sandcastle/
├── Cargo.toml                     # workspace
├── crates/
│   ├── sandcastle-server/         # MCP server + CLI
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs            # CLI entry: parse args, start MCP server
│   │       ├── mcp.rs             # MCP protocol handler (JSON-RPC, tool dispatch)
│   │       └── transport.rs       # stdio transport (read stdin, write stdout)
│   │
│   ├── sandcastle-manager/        # Sandbox lifecycle management
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── manager.rs         # SandboxManager: create/execute/destroy dispatch
│   │       ├── config.rs          # SandboxConfig, ExecutionResult types
│   │       └── sandbox.rs         # Sandbox trait definition
│   │
│   ├── sandcastle-namespace/      # Low isolation backend
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── sandbox.rs         # NamespaceSandbox implementation
│   │       ├── cgroups.rs         # Cgroup v2 setup (memory, cpu, pids)
│   │       ├── seccomp.rs         # Seccomp-BPF filter
│   │       └── rootfs.rs          # Rootfs setup (bind mounts, pivot_root)
│   │
│   └── sandcastle-executor/       # Binary that runs inside the sandbox
│       ├── Cargo.toml
│       └── src/
│           └── main.rs            # Receives code via stdin, runs it, returns JSON result
│
├── rootfs/                        # Minimal rootfs construction
│   ├── build.sh                   # Script to build rootfs tarballs per language
│   ├── python/                    # Python rootfs additions
│   ├── node/                      # Node.js rootfs additions
│   └── bash/                      # Bash rootfs additions
│
├── tests/
│   ├── integration/               # Integration tests
│   └── e2e/                       # End-to-end MCP protocol tests
│
├── sandcastle.toml                # Default config
└── README.md
```

## Component Details

### 1. sandcastle-server (CLI + MCP)

**main.rs:**
```rust
#[tokio::main]
async fn main() {
    // Parse CLI args (serve --stdio, serve --http, version, help)
    // Load sandcastle.toml config
    // Create SandboxManager
    // Start MCP server on chosen transport
}
```

**mcp.rs — MCP Protocol Handler:**
- Implements JSON-RPC 2.0 over stdio
- Handles `initialize`, `tools/list`, `tools/call` methods
- Dispatches tool calls to SandboxManager
- Tool definitions:
  - `execute_code` → manager.execute_oneshot()
  - `create_sandbox` → manager.create()
  - `execute_in_sandbox` → manager.execute()
  - `upload_file` → manager.upload_file()
  - `download_file` → manager.download_file()
  - `destroy_sandbox` → manager.destroy()

**transport.rs:**
- Reads JSON-RPC messages from stdin (Content-Length header framing)
- Writes JSON-RPC responses to stdout
- Phase 2 will add HTTP+SSE transport

### 2. sandcastle-manager

**Sandbox trait:**
```rust
#[async_trait]
pub trait Sandbox: Send + Sync {
    /// Execute code inside this sandbox.
    async fn execute(&self, code: &str, language: &str, timeout: Duration) -> Result<ExecutionResult>;

    /// Copy a file from host into the sandbox.
    async fn upload_file(&self, host_path: &Path, sandbox_path: &str) -> Result<FileInfo>;

    /// Copy a file from sandbox to host.
    async fn download_file(&self, sandbox_path: &str, host_path: &Path) -> Result<FileInfo>;

    /// Destroy this sandbox and all its resources.
    async fn destroy(&mut self) -> Result<()>;

    /// Check if sandbox is still alive.
    fn is_alive(&self) -> bool;
}
```

**SandboxManager:**
```rust
pub struct SandboxManager {
    config: Config,
    /// Active sandboxes: sandbox_id → Box<dyn Sandbox>
    sandboxes: HashMap<String, Box<dyn Sandbox>>,
    /// Sandbox metadata: sandbox_id → SandboxMeta (language, created_at, timeout)
    metadata: HashMap<String, SandboxMeta>,
}

impl SandboxManager {
    /// One-shot: create sandbox, run code, destroy, return result
    pub async fn execute_oneshot(&mut self, req: ExecuteRequest) -> Result<ExecutionResult>;

    /// Create a persistent sandbox session
    pub async fn create(&mut self, config: SandboxConfig) -> Result<SandboxId>;

    /// Execute in existing sandbox
    pub async fn execute(&self, id: &str, code: &str, timeout: Duration) -> Result<ExecutionResult>;

    /// Upload file from host to sandbox
    pub async fn upload_file(&self, id: &str, host_path: &Path, sandbox_path: &str) -> Result<FileInfo>;

    /// Download file from sandbox to host
    pub async fn download_file(&self, id: &str, sandbox_path: &str) -> Result<FileInfo>;

    /// Destroy sandbox
    pub async fn destroy(&mut self, id: &str) -> Result<()>;

    /// Background: reap expired sandboxes
    pub async fn reap_expired(&mut self);
}
```

**Types:**
```rust
pub struct ExecuteRequest {
    pub code: String,
    pub language: String,
    pub isolation: IsolationLevel,
    pub timeout_seconds: u32,
    pub memory_mb: u32,
    pub allowed_domains: Vec<String>,  // Phase 2: ignored in Phase 1
}

pub struct ExecutionResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub execution_time_ms: u64,
    pub timed_out: bool,
    pub oom_killed: bool,
}

pub struct SandboxConfig {
    pub language: String,
    pub isolation: IsolationLevel,
    pub timeout_seconds: u32,
    pub memory_mb: u32,
    pub cpu_cores: u32,
    pub allowed_domains: Vec<String>,
    pub env_vars: HashMap<String, String>,
}

pub struct FileInfo {
    pub path: String,
    pub size_bytes: u64,
}

pub enum IsolationLevel {
    Low,
    Medium,  // Phase 2
    High,    // Phase 2
}
```

### 3. sandcastle-namespace (Low Isolation Backend)

**Sandbox creation flow:**
```
1. Generate sandbox_id (UUID)
2. Create sandbox directory: /tmp/sandcastle/{sandbox_id}/
3. Prepare rootfs:
   a. Create directory structure (bin, lib, workspace, tmp, etc)
   b. Bind-mount language runtime from host (read-only)
   c. Copy sandcastle-executor binary into rootfs
4. Create cgroup: /sys/fs/cgroup/sandcastle/{sandbox_id}/
   a. Set memory.max
   b. Set cpu.max (quota/period)
   c. Set pids.max
5. clone() with CLONE_NEWPID | CLONE_NEWNS | CLONE_NEWNET | CLONE_NEWUSER
6. In child:
   a. pivot_root to sandbox rootfs
   b. Mount /proc, /dev/null, /dev/urandom
   c. Apply seccomp filter
   d. Drop all capabilities
   e. exec(sandcastle-executor) — wait for commands on stdin
7. Parent holds child PID + stdin/stdout pipes
```

**Code execution flow:**
```
1. Write to child's stdin: { "code": "...", "language": "python", "timeout_ms": 30000 }
2. Executor inside sandbox:
   a. Writes code to /workspace/code.{ext}
   b. Spawns: python /workspace/code.py (or node, bash)
   c. Captures stdout/stderr with timeout
   d. Kills child if timeout exceeded
   e. Writes result to stdout: { "stdout": "...", "stderr": "...", "exit_code": 0, ... }
3. Parent reads result from child's stdout
4. Return ExecutionResult
```

**File upload flow:**
```
1. Validate host_path is within allowed_input_dirs
2. Validate sandbox exists and is alive
3. Copy file from host_path into sandbox's rootfs at /workspace/{sandbox_path}
   (via /proc/{pid}/root/workspace/{sandbox_path} from host)
```

**File download flow:**
```
1. Validate sandbox exists
2. Read file from sandbox's rootfs at /workspace/{sandbox_path}
   (via /proc/{pid}/root/workspace/{sandbox_path})
3. Copy to host output_dir/{sandbox_id}/{filename}
4. Return host path
```

**Destroy flow:**
```
1. Send SIGKILL to child process
2. Wait for child to exit
3. Remove cgroup directory
4. Unmount and remove sandbox rootfs directory
```

### 4. sandcastle-executor (In-Sandbox Binary)

Minimal static binary that runs inside the sandbox. Compiled with `musl` for portability.

```rust
fn main() {
    // Read JSON commands from stdin, one per line
    loop {
        let cmd: Command = read_json_line(stdin);
        match cmd {
            Command::Execute { code, language, timeout_ms } => {
                // Write code to file
                let ext = match language { "python" => "py", "javascript" => "js", "bash" => "sh" };
                fs::write(format!("/workspace/code.{}", ext), &code);

                // Run it
                let runtime = match language {
                    "python" => "python3",
                    "javascript" => "node",
                    "bash" => "bash",
                };

                let start = Instant::now();
                let child = ProcessCommand::new(runtime)
                    .arg(format!("/workspace/code.{}", ext))
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn();

                // Wait with timeout
                let result = wait_with_timeout(child, timeout_ms);
                let elapsed = start.elapsed();

                // Return result
                println!("{}", json!({
                    "stdout": result.stdout,
                    "stderr": result.stderr,
                    "exit_code": result.exit_code,
                    "execution_time_ms": elapsed.as_millis(),
                    "timed_out": result.timed_out,
                    "oom_killed": result.oom_killed,
                }));
            }
        }
    }
}
```

## Rootfs Construction

Each language rootfs is a minimal directory tree built from the host system or a Docker export:

```bash
# build.sh — builds rootfs for each language
build_python_rootfs() {
    mkdir -p rootfs/python/{bin,lib,usr,workspace,tmp,dev,proc,etc}
    # Copy Python runtime + dependencies
    cp /usr/bin/python3 rootfs/python/usr/bin/
    # Copy shared libraries (ldd python3 | ...)
    # Copy sandcastle-executor
    cp target/release/sandcastle-executor rootfs/python/sandcastle/executor
    # Create tarball
    tar -czf rootfs/python.tar.gz -C rootfs/python .
}
```

Alternative (cleaner): export from a Docker container:
```bash
docker create --name sc-python python:3.12-slim
docker export sc-python | tar -xf - -C rootfs/python/
docker rm sc-python
# Add sandcastle-executor
cp target/release/sandcastle-executor rootfs/python/sandcastle/executor
```

## Rust Dependencies (Phase 1)

```toml
# sandcastle-server
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
clap = { version = "4", features = ["derive"] }     # CLI args
toml = "0.8"                                         # Config parsing
tracing = "0.1"                                      # Logging
tracing-subscriber = "0.3"
uuid = { version = "1", features = ["v4"] }

# sandcastle-manager
tokio = { version = "1", features = ["full"] }
async-trait = "0.1"

# sandcastle-namespace
nix = { version = "0.29", features = ["process", "mount", "sched", "signal"] }
seccompiler = "0.4"                                   # Seccomp-BPF filter builder
libc = "0.2"

# sandcastle-executor (statically linked with musl)
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

## Configuration (Phase 1 subset)

```toml
# sandcastle.toml (Phase 1 — only low isolation)

[server]
transport = "stdio"

[defaults]
isolation = "low"           # Only "low" available in Phase 1
timeout_seconds = 30
memory_mb = 512
cpu_cores = 1
language = "python"

[files]
allowed_input_dirs = ["/tmp/sandcastle-input"]
output_dir = "/tmp/sandcastle-output"
max_file_size_bytes = 10485760    # 10 MB

[limits]
max_memory_mb = 4096
max_timeout_seconds = 600
max_disk_mb = 5120
max_output_bytes = 1048576        # 1 MB
max_processes = 64

[rootfs]
python = "/var/lib/sandcastle/rootfs/python"
node = "/var/lib/sandcastle/rootfs/node"
bash = "/var/lib/sandcastle/rootfs/bash"
```

## Testing Strategy

### Unit Tests
- Config parsing
- MCP protocol message parsing/serialization
- Seccomp filter construction
- Cgroup setup/teardown
- Rootfs bind mount setup

### Integration Tests
- Create sandbox → execute Python code → verify stdout
- Create sandbox → execute JavaScript code → verify stdout
- Timeout enforcement (run infinite loop, verify timed_out=true)
- Memory limit (allocate beyond limit, verify oom_killed=true)
- Fork bomb (verify pids.max blocks it)
- File upload → execute code that reads it → verify content
- File download → verify file appears at host_path
- Destroy → verify all resources cleaned up
- Expired sandbox reaping

### E2E Tests
- Full MCP protocol flow via stdio:
  - Send `initialize` → verify capabilities
  - Send `tools/list` → verify all 6 tools listed
  - Send `tools/call execute_code` → verify result
  - Send `tools/call create_sandbox` → get sandbox_id
  - Send `tools/call execute_in_sandbox` → verify state persists
  - Send `tools/call destroy_sandbox` → verify cleanup

## Implementation Order

1. **sandcastle-executor** — build the in-sandbox binary first (simplest, most testable independently)
2. **sandcastle-namespace** — build sandbox creation (clone + rootfs + cgroups + seccomp)
3. **sandcastle-manager** — wire up lifecycle management
4. **sandcastle-server** — add MCP protocol handling + CLI
5. **rootfs** — build language rootfs images
6. **tests** — integration + E2E
7. **polish** — error handling, logging, config validation

## Open Questions

1. **Rootfs strategy**: Build from host binaries or Docker export? Docker export is simpler but heavier (~100MB per language). Host binaries need careful ldd dependency tracking.
2. **Executor communication**: stdin/stdout JSON lines vs Unix socket? JSON lines is simpler and works across all isolation levels (Phase 2 gVisor/FC can also use stdin).
3. **Cgroup delegation**: Do we need systemd cgroup delegation or can we write directly to /sys/fs/cgroup? Direct writes work on most systems but may conflict with systemd.
4. **musl vs glibc for executor**: musl gives us a static binary (no shared lib dependencies), but some Python packages need glibc. Executor should be musl; the language runtime in rootfs uses glibc.
