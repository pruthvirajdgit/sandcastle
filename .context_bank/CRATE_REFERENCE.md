# Crate Reference

Detailed per-crate documentation of public APIs, key types, and internal design.

---

## sandcastle-runtime

**Role**: Shared interface layer — defines the trait and types used by all other crates.

**Path**: `service/crates/sandcastle-runtime/`

### Key Types (`src/types.rs`)

```rust
// Unique sandbox identifier (wraps String)
pub struct SandboxId(pub String);

// Supported languages
pub enum Language { Python, Javascript, Bash }

// Isolation levels for tiered sandbox backends
pub enum IsolationLevel { Low, Medium, High }

// Configuration for creating a sandbox
pub struct SandboxConfig {
    pub language: Language,
    pub isolation: IsolationLevel,
    pub limits: ResourceLimits,
    pub env_vars: HashMap<String, String>,
}

// Resource constraints
pub struct ResourceLimits {
    pub memory_mb: u32,         // Default: 512
    pub cpu_quota: u32,         // Default: 100 (1 core)
    pub timeout: Duration,      // Default: 30s
    pub max_pids: u32,          // Default: 64
    pub max_disk_mb: u32,       // Default: 512
    pub max_output_bytes: u64,  // Default: 1MB
}

// Request to execute code
pub struct ExecRequest {
    pub code: String,
    pub timeout: Duration,
}

// Result of execution
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub execution_time_ms: u64,
    pub timed_out: bool,
    pub oom_killed: bool,
}

// Sandbox lifecycle states
pub enum SandboxStatus { Created, Running, Stopped, Failed(String) }
```

### SandboxRuntime Trait (`src/runtime.rs`)

```rust
#[async_trait]
pub trait SandboxRuntime: Send + Sync {
    async fn create(&self, config: &SandboxConfig) -> Result<SandboxId>;
    async fn start(&self, id: &SandboxId) -> Result<()>;
    async fn execute(&self, id: &SandboxId, request: &ExecRequest) -> Result<ExecResult>;
    async fn stop(&self, id: &SandboxId) -> Result<()>;
    async fn destroy(&self, id: &SandboxId) -> Result<()>;
    async fn upload_file(&self, id: &SandboxId, host_path: &Path, sandbox_path: &Path) -> Result<u64>;
    async fn download_file(&self, id: &SandboxId, sandbox_path: &Path, host_path: &Path) -> Result<u64>;
    async fn status(&self, id: &SandboxId) -> Result<SandboxStatus>;
}
```

### Error Types (`src/error.rs`)

```rust
pub enum SandcastleError {
    SessionNotFound(String),      // MCP error code: -1
    SessionExpired(String),       // -3
    MaxSessionsReached(usize),    // -2
    PathNotAllowed(PathBuf),
    PathTraversal(String),
    FileNotFound(PathBuf),
    FileTooLarge { size, max },   // -2
    ExecutionFailed(String),
    Timeout,
    OomKilled,
    SandboxCreationFailed(String),
    RuntimeError(String),
    InvalidParams(String),        // -32602
    UnknownTool(String),          // -32602
    UnsupportedIsolation(String),
    UnsupportedLanguage(String),
}
```

**Tests**: 12 unit tests in `types.rs` (serde roundtrips, defaults, display impls, isolation levels)

---

## sandcastle-executor

**Role**: Binary that runs **inside** the sandbox as PID 1. Supports two modes: **stdio** (containers) and **vsock** (Firecracker VMs). Receives code execution commands, runs them, returns results.

**Path**: `service/crates/sandcastle-executor/`

**Binary**: Must be compiled with `--target x86_64-unknown-linux-musl` (static linking required)
- Containers: `cargo build -p sandcastle-executor --target x86_64-unknown-linux-musl`
- Firecracker VMs: `cargo build -p sandcastle-executor --target x86_64-unknown-linux-musl --features vsock-mode`

### Dual-Mode Operation

| Mode | Transport | Activated By | Used In |
|------|-----------|-------------|---------|
| stdio | stdin/stdout pipes | Default (no flags) | ProcessSandbox, GvisorSandbox |
| vsock | vsock port 5000 | `--vsock` CLI flag | FirecrackerSandbox |

### Internal Types

```rust
// Received from host (JSON line)
struct ExecCommand {
    action: String,        // "exec", "upload", or "download"
    language: String,      // "python", "javascript", "bash" (exec only)
    code: String,          // The code to execute (exec only)
    timeout_ms: u64,       // Max execution time (exec only)
    max_output_bytes: u64, // Output truncation limit (exec only)
    path: String,          // Sandbox path (upload/download only)
    content_base64: String, // Base64-encoded file data (upload only)
}

// Sent to host (JSON line)
struct ExecResponse {
    stdout: String,        // For downloads: base64-encoded file content
    stderr: String,
    exit_code: i32,        // -1 for internal errors, 124 for timeout, 137 for OOM
    execution_time_ms: u64,
    timed_out: bool,
    oom_killed: bool,
}
```

### Key Behavior
- **stdio mode**: Reads stdin line by line in a loop, writes responses to stdout
- **vsock mode**: Binds `VsockListener` on port 5000, accepts one connection, reads/writes JSON lines
- Shared `handle_line()` dispatcher for exec/upload/download actions
- Writes code to `/workspace/code.{ext}`, spawns language runtime
- Uses `Command::new()` (allowed — runs inside sandbox, not on host)
- Poll-based timeout with 10ms intervals
- Truncates output to `max_output_bytes`
- Base64 encode/decode for file transfer (inline, no external crate)

**Tests**: None (tested via e2e integration test)

---

## sandcastle-manager

**Role**: Session lifecycle management, input validation, file transfer orchestration. Backend-agnostic — works with any `SandboxRuntime` implementation.

**Path**: `service/crates/sandcastle-manager/`

### Public API

```rust
pub struct SandboxManager {
    // Holds HashMap<IsolationLevel, Arc<dyn SandboxRuntime>> + sessions map + config
}

impl SandboxManager {
    pub fn new(runtimes: HashMap<IsolationLevel, Arc<dyn SandboxRuntime>>, config: ManagerConfig) -> Self;
    pub fn with_runtime(runtime: Arc<dyn SandboxRuntime>, config: ManagerConfig) -> Self; // single-backend compat
    pub async fn execute_oneshot(&self, code: &str, language: Language, isolation: IsolationLevel, timeout: Duration) -> Result<ExecResult>;
    pub async fn create_session(&self, config: SandboxConfig) -> Result<String>;
    pub async fn execute_in_session(&self, session_id: &str, code: &str, timeout: Duration) -> Result<ExecResult>;
    pub async fn upload(&self, session_id: &str, host_path: &Path, sandbox_path: &str) -> Result<u64>;
    pub async fn download(&self, session_id: &str, sandbox_path: &str, host_path: Option<&str>) -> Result<(PathBuf, u64)>;
    pub async fn destroy_session(&self, session_id: &str) -> Result<()>;
    pub async fn reap_expired(&self);
    pub async fn list_sessions(&self) -> Vec<String>;
}
```

### Multi-Backend Routing

Sessions store their `IsolationLevel` so subsequent calls (execute, upload, download, destroy) route to the correct backend automatically. `create_session()` and `execute_oneshot()` look up the runtime by isolation level and return `UnsupportedIsolation` if no backend is registered for that level.

### Configuration

```rust
pub struct ManagerConfig {
    pub max_sessions: usize,           // Default: 50
    pub session_timeout_seconds: u64,  // Default: 300 (5 min idle)
    pub defaults: ResourceLimits,
    pub files: FileConfig,
}

pub struct FileConfig {
    pub allowed_input_dirs: Vec<PathBuf>,  // Directories allowed for upload source
    pub output_dir: PathBuf,               // Directory for downloaded files
    pub max_file_size_bytes: u64,          // Max upload/download size
}
```

### Security Features
- **Path traversal prevention**: Rejects sandbox paths containing `..`
- **Upload allowlist**: Only files from `allowed_input_dirs` can be uploaded
- **Download scoping**: Downloads go only to `output_dir`
- **File size limits**: Enforced on both upload and download
- **Session limits**: Max concurrent sessions enforced
- **Auto-expiry**: Background task reaps idle sessions

**Tests**: 6 unit tests with MockRuntime (oneshot, create+execute, max sessions, not found, destroy nonexistent, unsupported isolation)

---

## sandcastle-process

**Role**: Container backend using Linux namespaces via `libcontainer` (youki's crate). Implements `SandboxRuntime` trait.

**Path**: `service/crates/sandcastle-process/`

### Configuration

```rust
pub struct ProcessConfig {
    pub rootfs_dir: PathBuf,      // /var/lib/sandcastle/rootfs
    pub state_dir: PathBuf,       // /run/sandcastle (container runtime state)
    pub bundle_dir: PathBuf,      // /var/lib/sandcastle/bundles
    pub workspace_dir: PathBuf,   // /var/lib/sandcastle/workspaces
    pub executor_path: PathBuf,   // /var/lib/sandcastle/bin/executor
}
```

### Key Dependencies

| Crate | Version | Features | Purpose |
|-------|---------|----------|---------|
| libcontainer | 0.6.0 | v2, systemd | Container lifecycle (create, start, kill, delete) |
| oci-spec | 0.9.0 | runtime | OCI config.json generation |
| nix | 0.29 | fs, process, signal | Unix pipes, signals |

### Internal Architecture

```
ProcessSandbox {
    config: ProcessConfig,
    containers: RwLock<HashMap<String, ContainerHandle>>
}

ContainerHandle {
    stdin_writer: File,           // Write end of pipe to executor
    stdout_reader: BufReader,     // Read end of pipe from executor  
    language: Language,
    bundle_dir: PathBuf,
    workspace_dir: PathBuf,
    status: SandboxStatus,
    exec_lock: Arc<Mutex<()>>,   // Serializes concurrent execute calls
}
```

### OCI Spec Generation (`oci.rs`)

`generate_spec()` creates a minimal OCI config with:
- **Process**: `/sandbox/executor` with language-specific env vars
- **Root**: Language rootfs (e.g., `/var/lib/sandcastle/rootfs/python`)
- **Mounts**: `/proc` (proc), `/dev` (tmpfs), `/workspace` (bind)
- **Namespaces**: PID + Mount (minimum for isolation)
- **Resources**: PID limit from ResourceLimits

### Container Lifecycle

```
create() → prepare_bundle() + pipe() + ContainerBuilder::build()
  ↓
start() → Container::load().start()  (resumes blocked init)
  ↓
execute() → write JSON to stdin pipe → read JSON from stdout pipe
  ↓ (can call execute() multiple times)
stop() → Container::kill(SIGTERM)
  ↓
destroy() → Container::delete(force) + cleanup dirs
```

**Tests**: 1 unit test (OCI spec generation) + 1 e2e integration test (Python execution in real container, requires root)

---

## sandcastle-gvisor

**Role**: gVisor container backend using `runsc` CLI. Implements `SandboxRuntime` trait for medium isolation. Provides syscall-level interception via gVisor's Sentry.

**Path**: `service/crates/sandcastle-gvisor/`

### Configuration

```rust
pub struct GvisorConfig {
    pub runsc_path: PathBuf,      // /usr/local/bin/runsc
    pub rootfs_dir: PathBuf,      // /var/lib/sandcastle/rootfs
    pub state_dir: PathBuf,       // /run/sandcastle/gvisor
    pub bundle_dir: PathBuf,      // /var/lib/sandcastle/gvisor/bundles
    pub workspace_dir: PathBuf,   // /var/lib/sandcastle/gvisor/workspaces
    pub executor_path: PathBuf,   // /var/lib/sandcastle/bin/executor
    pub platform: String,         // "ptrace" (default, no KVM needed)
}
```

### Module Structure

| Module | Purpose | Uses Command::new? |
|--------|---------|--------------------|
| `config.rs` | GvisorConfig with defaults | No |
| `oci.rs` | OCI spec generation (5 namespaces) | No |
| `runsc.rs` | All runsc CLI interactions | **Yes** (approved exception) |
| `sandbox.rs` | GvisorSandbox implementing SandboxRuntime | No |

### Key Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| sandcastle-runtime | workspace | SandboxRuntime trait, types |
| tokio | workspace | Async subprocess management |
| serde_json | workspace | OCI spec serialization |
| uuid | workspace | Container ID generation |

### Internal Architecture

```
GvisorSandbox {
    config: GvisorConfig,
    containers: RwLock<HashMap<String, ContainerHandle>>
}

ContainerHandle {
    child: Option<Child>,              // tokio::process::Child (None before start)
    stdin: Option<ChildStdin>,         // Persistent write pipe to executor
    stdout: Option<BufReader<ChildStdout>>,  // Persistent buffered read pipe
    language: Language,
    bundle_dir: PathBuf,
    workspace_dir: PathBuf,
    container_id: String,
    exec_lock: Arc<Mutex<()>>,   // Serializes concurrent execute calls
}
```

### Container Lifecycle (runsc CLI)

```
create() → prepare OCI bundle + rootfs symlink + workspace dir (chmod 777)
  ↓
start() → tokio::process::Command::new("runsc").arg("run") + wait for {"ready":true} from executor
  ↓
execute() → write JSON to persistent stdin → read JSON from persistent stdout (with timeout)
  ↓ (can call execute() multiple times)
stop() → runsc kill SIGKILL + child.kill() + child.wait() (prevent zombies)
  ↓
destroy() → runsc delete --force + remove bundle + workspace dirs
```

### OCI Spec Differences from ProcessSandbox

| Feature | ProcessSandbox | GvisorSandbox |
|---------|---------------|---------------|
| OCI version | 1.0.2 | 1.0.2 |
| Namespaces | PID, Mount | PID, Mount, IPC, UTS, Network |
| Root path | Absolute path to rootfs | Absolute path to rootfs |
| /dev | tmpfs mount | tmpfs mount |
| Container runtime | libcontainer (in-process) | runsc CLI (subprocess) |
| ID prefix | `sc-` | `gv-` |

**Tests**: 1 unit test (OCI spec) + 1 e2e integration test (Python execution in gVisor, requires root + runsc)

---

## sandcastle-firecracker

**Role**: Firecracker microVM backend using the `firepilot` crate. Implements `SandboxRuntime` trait for high isolation. Provides full hardware virtualization via KVM — each sandbox is an independent virtual machine.

**Path**: `service/crates/sandcastle-firecracker/`

### Configuration

```rust
pub struct FirecrackerConfig {
    pub firecracker_path: PathBuf,  // /usr/local/bin/firecracker
    pub kernel_path: PathBuf,       // /var/lib/sandcastle/kernel/vmlinux
    pub rootfs_dir: PathBuf,        // /var/lib/sandcastle/rootfs (ext4 images)
    pub state_dir: PathBuf,         // /run/sandcastle/firecracker (VM state)
    pub workspace_dir: PathBuf,     // /var/lib/sandcastle/fc-workspaces
    pub guest_cid_base: u32,        // 100 (vsock CID base, incremented per VM)
    pub vsock_port: u32,            // 5000 (port executor listens on inside VM)
    pub memory_mb: u32,             // 256 (VM memory)
    pub vcpu_count: u32,            // 1 (VM vCPUs)
    pub boot_args: String,          // Kernel command line
}
```

### Module Structure

| Module | Purpose | Uses Command::new? |
|--------|---------|--------------------|
| `config.rs` | FirecrackerConfig with defaults, `is_available()` check | No |
| `vsock.rs` | VsockConnection: UDS proxy protocol, readiness, JSON I/O | No |
| `sandbox.rs` | FirecrackerSandbox implementing SandboxRuntime | No |

Note: The `firepilot` crate internally spawns the Firecracker binary — this is an approved exception (same as runsc).

### Key Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| firepilot | 1.2.0 | Firecracker VM lifecycle (create, start, kill) |
| hyper | 0.14 | HTTP client for Firecracker REST API (vsock config) |
| hyperlocal | 0.8 | Unix socket HTTP transport for Firecracker API |
| tokio | workspace | Async runtime, Unix streams, timers |
| sandcastle-runtime | workspace | SandboxRuntime trait, types |

### Internal Architecture

```
FirecrackerSandbox {
    config: FirecrackerConfig,
    vms: RwLock<HashMap<String, VmHandle>>,
    next_cid: AtomicU32,   // Monotonically increasing guest CID
}

VmHandle {
    machine: Machine,       // firepilot Machine (manages Firecracker process)
    language: Language,
    workspace_dir: PathBuf, // Host workspace (files staged here before vsock upload)
    vsock_uds_path: PathBuf, // Path to Firecracker's vsock UDS proxy socket
    guest_cid: u32,
}
```

### VM Lifecycle

```
create() → prepare dirs + firepilot Machine::new() + machine.create(config)
  ↓        Configures: kernel, rootfs (ext4 drive), vsock via REST API
start() → machine.start() + vsock retry loop (up to 30s, 500ms intervals)
  ↓        Waits for executor to be reachable via vsock readiness check
execute() → VsockConnection::connect() → CONNECT 5000\n → OK → JSON request/response
  ↓        Same JSON protocol as containers, but over vsock instead of stdin/stdout
upload_file() → read host file → base64 encode → send upload command over vsock
download_file() → send download command over vsock → base64 decode → write host file
  ↓
stop() → machine.kill()
  ↓
destroy() → cleanup state dirs + workspace
```

### Vsock Communication Protocol

Firecracker exposes guest vsock via a Unix domain socket (UDS proxy):
1. Host connects to `{state_dir}/{sandbox_id}.vsock`
2. Sends `CONNECT <port>\n` (e.g., `CONNECT 5000\n`)
3. Firecracker responds with `OK <port>\n`
4. Bidirectional stream established — JSON commands flow same as stdin/stdout

```
VsockConnection {
    stream: tokio::net::UnixStream,  // Connected to Firecracker UDS proxy
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
}
```

Key methods:
- `connect(uds_path, port)` — Opens UDS, sends CONNECT, validates OK response
- `wait_ready(uds_path, port, timeout)` — Retry loop until vsock is reachable
- `execute_json(request)` — Send JSON line, read JSON response line

### File Transfer

Unlike containers (where host filesystem is bind-mounted), VM filesystems are isolated:
- **Upload**: Host reads file → base64 encode → JSON `{"action":"upload","path":"...","content_base64":"..."}` over vsock
- **Download**: JSON `{"action":"download","path":"..."}` over vsock → response stdout contains base64 data → decode → write to host
- Path traversal protection: `sanitize_sandbox_path()` rejects `..` and absolute paths
- Inline base64 encode/decode (no external crate)

**Tests**: 1 unit test (config defaults) + 1 e2e integration test (Python execution in Firecracker VM, requires root + KVM)

---

## sandcastle-server

**Role**: MCP server entry point. Exposes sandbox operations as MCP tools via the `rmcp` crate.

**Path**: `service/crates/sandcastle-server/`

### CLI

```
sandcastle serve [--transport stdio]
```

Currently only `stdio` transport is supported (Phase 1).

### MCP Tools

Defined in `tools.rs` using rmcp's `#[tool]` macro:

| Method | Handler | Params |
|--------|---------|--------|
| `execute_code` | `SandcastleTools::execute_code()` | `{code, language?, timeout_seconds?, isolation?}` |
| `create_sandbox` | `SandcastleTools::create_sandbox()` | `{language?, memory_mb?, timeout_seconds?, isolation?}` |
| `execute_in_session` | `SandcastleTools::execute_in_session()` | `{session_id, code, timeout_seconds?}` |
| `upload_file` | `SandcastleTools::upload_file()` | `{session_id, host_path, sandbox_path}` |
| `download_file` | `SandcastleTools::download_file()` | `{session_id, sandbox_path, host_path?}` |
| `destroy_sandbox` | `SandcastleTools::destroy_sandbox()` | `{session_id}` |

### Startup Sequence

```rust
main():
  1. Initialize tracing (stderr, env filter)
  2. Parse CLI args
  3. Create ProcessConfig + ProcessSandbox → register at IsolationLevel::Low
  4. Check if runsc available → if yes, create GvisorConfig + GvisorSandbox → register at IsolationLevel::Medium
  5. Check if firecracker + kernel available → if yes, create FirecrackerConfig + FirecrackerSandbox → register at IsolationLevel::High
  6. Build HashMap<IsolationLevel, Arc<dyn SandboxRuntime>>
  7. Create ManagerConfig + SandboxManager with runtime map
  8. Spawn reaper background task (runs every 5s)
  9. Create SandcastleTools(manager)
  10. Start rmcp stdio transport
  11. Block on server.waiting()
```

### Key Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| rmcp | 1 (v1.4) | MCP protocol server |
| schemars | 1.0 | JSON Schema for tool params (must match rmcp's version) |
| clap | 4 | CLI argument parsing |

**Tests**: None in server (covered by e2e and manager unit tests)
