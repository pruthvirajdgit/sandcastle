# Phase 1 — Technical Specification

## System Overview

```
┌───────────────────────────────────────────────────────────┐
│                   AI Agent (MCP Client)                    │
└────────────────────────┬──────────────────────────────────┘
                         │ stdin/stdout (JSON-RPC 2.0)
┌────────────────────────▼──────────────────────────────────┐
│                  SandcastleServer                          │
│  ┌──────────────────┐  ┌───────────────────────────────┐  │
│  │ Stdio Transport  │  │ Tool Router                   │  │
│  │ (read/write)     │──│ execute_code → oneshot        │  │
│  │                  │  │ create_sandbox → create        │  │
│  │                  │  │ execute_in_session → execute   │  │
│  │                  │  │ upload_file → upload           │  │
│  │                  │  │ download_file → download       │  │
│  │                  │  │ destroy_sandbox → destroy      │  │
│  └──────────────────┘  └──────────────┬────────────────┘  │
└───────────────────────────────────────┼───────────────────┘
                                        │
┌───────────────────────────────────────▼───────────────────┐
│                  SandboxManager                            │
│  ┌────────────────┐ ┌──────────────┐ ┌─────────────────┐ │
│  │ Session        │ │ File         │ │ Reaper          │ │
│  │ Registry       │ │ Validator    │ │ (background)    │ │
│  │ (HashMap)      │ │              │ │                 │ │
│  └───────┬────────┘ └──────┬───────┘ └────────┬────────┘ │
│          │                 │                   │          │
│  ┌───────▼─────────────────▼───────────────────▼────────┐ │
│  │ Config (sandcastle.toml)                             │ │
│  │ defaults, limits, file paths, rootfs locations       │ │
│  └──────────────────────────┬───────────────────────────┘ │
└─────────────────────────────┼────────────────────────────┘
                              │
┌─────────────────────────────▼────────────────────────────┐
│               SandboxRuntime (trait)                       │
│  create() | start() | execute() | stop() | destroy()      │
│  upload_file() | download_file() | status()               │
│                                                           │
│  Phase 1 impl: ProcessSandbox                             │
│  (namespaces + seccomp + cgroups)                         │
└───────────────────────────────────────────────────────────┘
```

## Crate Structure

```
sandcastle/
├── Cargo.toml                   # workspace
├── crates/
│   ├── sandcastle-server/       # MCP protocol + CLI
│   ├── sandcastle-manager/      # Session lifecycle + file validation
│   ├── sandcastle-runtime/      # SandboxRuntime trait + shared types
│   └── sandcastle-executor/     # Binary that runs inside sandbox
├── rootfs/                      # Per-language rootfs build scripts
├── tests/                       # Integration + E2E tests
└── sandcastle.toml              # Default config
```

---

## 1. SandcastleServer

The server is a **thin protocol layer** with zero business logic. It deserializes MCP requests, routes them to the manager, and serializes responses.

### Responsibilities

- Parse MCP JSON-RPC 2.0 messages from stdin
- Route `tools/call` to the correct manager method
- Respond to `initialize` and `tools/list`
- Serialize results and errors to stdout
- No session state, no validation, no sandbox awareness

### MCP Protocol

MCP uses **JSON-RPC 2.0** with Content-Length header framing (like LSP):

```
Content-Length: 123\r\n
\r\n
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"execute_code","arguments":{"code":"print(1)"}}}
```

### Methods Handled

| Method | Handler |
|--------|---------|
| `initialize` | Return server capabilities (tool list, version) |
| `tools/list` | Return all 6 tool definitions with JSON schemas |
| `tools/call` | Dispatch to manager based on tool name |

### Tool Routing

```rust
pub struct SandcastleServer {
    manager: Arc<SandboxManager>,
}

impl SandcastleServer {
    pub async fn handle_tool_call(&self, name: &str, args: Value) -> Result<Value> {
        match name {
            "execute_code" => {
                let req: ExecuteRequest = serde_json::from_value(args)?;
                let result = self.manager.execute_oneshot(req).await?;
                Ok(serde_json::to_value(result)?)
            }
            "create_sandbox" => {
                let config: SandboxConfig = serde_json::from_value(args)?;
                let session = self.manager.create_session(config).await?;
                Ok(serde_json::to_value(session)?)
            }
            "execute_in_session" => {
                let session_id = args["session_id"].as_str().required()?;
                let code = args["code"].as_str().required()?;
                let timeout = args.get("timeout_seconds");
                let result = self.manager.execute_in_session(session_id, code, timeout).await?;
                Ok(serde_json::to_value(result)?)
            }
            "upload_file" => {
                let session_id = args["session_id"].as_str().required()?;
                let host_path = Path::new(args["host_path"].as_str().required()?);
                let sandbox_path = args["sandbox_path"].as_str().required()?;
                let info = self.manager.upload(session_id, host_path, sandbox_path).await?;
                Ok(serde_json::to_value(info)?)
            }
            "download_file" => {
                let session_id = args["session_id"].as_str().required()?;
                let sandbox_path = args["sandbox_path"].as_str().required()?;
                let host_path = args.get("host_path").and_then(|v| v.as_str());
                let info = self.manager.download(session_id, sandbox_path, host_path).await?;
                Ok(serde_json::to_value(info)?)
            }
            "destroy_sandbox" => {
                let session_id = args["session_id"].as_str().required()?;
                self.manager.destroy_session(session_id).await?;
                Ok(json!({ "destroyed": true }))
            }
            _ => Err(SandcastleError::UnknownTool(name.to_string())),
        }
    }
}
```

### Stdio Transport

```rust
pub struct StdioTransport {
    reader: BufReader<Stdin>,
    writer: Stdout,
}

impl StdioTransport {
    /// Read one JSON-RPC message (Content-Length framed)
    pub async fn read_message(&mut self) -> Result<JsonRpcRequest> {
        // 1. Read "Content-Length: N\r\n\r\n"
        // 2. Read exactly N bytes
        // 3. Deserialize JSON-RPC
    }

    /// Write one JSON-RPC response
    pub async fn write_message(&mut self, response: &JsonRpcResponse) -> Result<()> {
        // 1. Serialize to JSON
        // 2. Write "Content-Length: {len}\r\n\r\n{json}"
    }
}
```

### Main Loop

```rust
#[tokio::main]
async fn main() {
    let config = load_config("sandcastle.toml");
    let runtime = ProcessSandbox::new(&config);
    let manager = Arc::new(SandboxManager::new(runtime, config));
    let server = SandcastleServer::new(manager.clone());

    // Start reaper background task
    let reaper_manager = manager.clone();
    tokio::spawn(async move {
        loop {
            reaper_manager.reap_expired().await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });

    // Main MCP loop
    let mut transport = StdioTransport::new();
    loop {
        let request = transport.read_message().await?;
        let response = server.handle_request(request).await;
        transport.write_message(&response).await?;
    }
}
```

### CLI (for testing, not MCP)

```bash
sandcastle serve --stdio           # Start MCP server (main mode)
sandcastle run -l python -c "..."  # Quick one-shot (debugging)
sandcastle config show             # Print loaded config
sandcastle config validate         # Validate sandcastle.toml
```

---

## 2. SandboxManager

The manager is the **brain** of the system. It owns sessions, validates files, enforces limits, and delegates to the runtime. It is the only component that talks to both the server and the runtime.

### Struct

```rust
pub struct SandboxManager {
    runtime: Arc<dyn SandboxRuntime>,
    sessions: RwLock<HashMap<String, Session>>,
    config: ManagerConfig,
}

struct Session {
    sandbox_id: SandboxId,
    language: Language,
    created_at: Instant,
    last_active: RwLock<Instant>,
    status: RwLock<SessionStatus>,
}

enum SessionStatus {
    Active,
    Expired,
    Destroying,
}

pub struct ManagerConfig {
    pub max_sessions: usize,
    pub session_timeout: Duration,
    pub default_limits: ResourceLimits,
    pub file_config: FileConfig,
}

pub struct FileConfig {
    pub allowed_input_dirs: Vec<PathBuf>,
    pub output_dir: PathBuf,
    pub max_file_size_bytes: u64,
}
```

### Methods

#### `execute_oneshot`

One-shot execution. No session created.

```
1. Validate request (language supported, limits within bounds)
2. runtime.create(config) → sandbox_id
3. runtime.start(sandbox_id)
4. runtime.execute(sandbox_id, code, timeout) → result
5. runtime.destroy(sandbox_id)
6. Return result
```

No entry in sessions map. Sandbox lives for the duration of one call.

#### `create_session`

Create a persistent sandbox and return a session_id.

```
1. Check sessions.len() < max_sessions
2. Generate session_id (UUID v4, prefixed "sb-")
3. Merge request config with defaults (fill missing fields)
4. runtime.create(config) → sandbox_id
5. runtime.start(sandbox_id)
6. Insert into sessions map
7. Return { session_id, language, isolation, expires_at }
```

#### `execute_in_session`

Run code in an existing session.

```
1. Lookup session_id in sessions map → Session
2. Check status == Active (not expired/destroying)
3. Check now - created_at < session_timeout
4. Update last_active = now
5. runtime.execute(session.sandbox_id, code, timeout) → result
6. Return result
```

#### `upload`

Copy a file from host into a sandbox.

```
1. Lookup session
2. Validate host_path:
   a. Canonicalize (resolve symlinks)
   b. Check starts_with one of allowed_input_dirs
   c. Check no ".." components
   d. Check file exists and is a regular file
   e. Check file size <= max_file_size_bytes
3. Validate sandbox_path:
   a. Check no ".." components
   b. Check no absolute path (must be relative to /workspace)
4. runtime.upload_file(sandbox_id, host_path, sandbox_path)
5. Return { sandbox_path: "/workspace/{sandbox_path}", size_bytes }
```

#### `download`

Copy a file from sandbox to host.

```
1. Lookup session
2. Validate sandbox_path:
   a. No ".." components
   b. Relative to /workspace
3. Determine host destination:
   a. If host_path provided: validate it starts with output_dir
   b. Else: use {output_dir}/{session_id}/{filename}
4. runtime.download_file(sandbox_id, sandbox_path, host_dest)
5. Check downloaded file size <= max_file_size_bytes
6. Return { host_path, size_bytes }
```

#### `destroy_session`

```
1. Lookup session, set status = Destroying
2. Remove from sessions map
3. runtime.stop(sandbox_id)
4. runtime.destroy(sandbox_id)
```

#### `reap_expired`

Background task, runs every 5 seconds.

```
1. Lock sessions map
2. For each session:
   a. If now - created_at > session_timeout:
      - Log "session {id} expired"
      - Set status = Expired
      - runtime.destroy(sandbox_id)
      - Remove from map
```

### Error Types

```rust
pub enum SandcastleError {
    // Session errors
    SessionNotFound(String),
    SessionExpired(String),
    MaxSessionsReached,

    // File errors
    PathNotAllowed(PathBuf),
    PathTraversal(String),
    FileNotFound(PathBuf),
    FileTooLarge { size: u64, max: u64 },

    // Execution errors
    ExecutionFailed(String),
    Timeout,
    OomKilled,

    // Runtime errors
    SandboxCreationFailed(String),
    RuntimeError(String),

    // Protocol errors
    InvalidParams(String),
    UnknownTool(String),
}

impl SandcastleError {
    /// Map to MCP JSON-RPC error code
    pub fn error_code(&self) -> i32 {
        match self {
            Self::InvalidParams(_) | Self::UnknownTool(_) => -32602,
            Self::SessionNotFound(_) => -1,
            Self::MaxSessionsReached | Self::FileTooLarge { .. } => -2,
            Self::SessionExpired(_) => -3,
            _ => -32603,
        }
    }
}
```

---

## 3. SandboxRuntime (Trait)

The abstract interface that all sandbox implementations must satisfy. The manager calls this — it never knows whether it's talking to a process, gVisor, or Firecracker.

```rust
#[async_trait]
pub trait SandboxRuntime: Send + Sync {
    /// Create a new sandbox (allocate resources, prepare rootfs). Does not start it.
    async fn create(&self, config: &SandboxConfig) -> Result<SandboxId>;

    /// Start the sandbox (spawn executor process, begin accepting commands).
    async fn start(&self, id: &SandboxId) -> Result<()>;

    /// Execute code inside a running sandbox.
    async fn execute(&self, id: &SandboxId, request: &ExecRequest) -> Result<ExecResult>;

    /// Stop the sandbox (graceful shutdown).
    async fn stop(&self, id: &SandboxId) -> Result<()>;

    /// Destroy the sandbox and clean up all resources.
    async fn destroy(&self, id: &SandboxId) -> Result<()>;

    /// Copy a file from host into the sandbox.
    async fn upload_file(
        &self,
        id: &SandboxId,
        host_path: &Path,
        sandbox_path: &Path,
    ) -> Result<u64>; // returns bytes copied

    /// Copy a file from sandbox to host.
    async fn download_file(
        &self,
        id: &SandboxId,
        sandbox_path: &Path,
        host_path: &Path,
    ) -> Result<u64>; // returns bytes copied

    /// Check sandbox status.
    async fn status(&self, id: &SandboxId) -> Result<SandboxStatus>;
}
```

### Shared Types

```rust
/// Unique sandbox identifier
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct SandboxId(pub String);

/// Supported languages
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Python,
    Javascript,
    Bash,
}

/// Sandbox configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    pub language: Language,
    pub limits: ResourceLimits,
    pub env_vars: HashMap<String, String>,
}

/// Resource constraints
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub memory_mb: u32,        // cgroup memory.max
    pub cpu_quota: u32,        // cgroup cpu.max (percentage, 100 = 1 core)
    pub timeout_seconds: u32,  // execution timeout
    pub max_pids: u32,         // cgroup pids.max
    pub max_disk_mb: u32,      // tmpfs size for /workspace
    pub max_output_bytes: u64, // stdout/stderr cap
}

/// Code execution request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecRequest {
    pub code: String,
    pub timeout: Duration,
}

/// Code execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub execution_time_ms: u64,
    pub timed_out: bool,
    pub oom_killed: bool,
}

/// Sandbox lifecycle status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SandboxStatus {
    Created,     // resources allocated, not started
    Running,     // executor active, accepting commands
    Stopped,     // gracefully stopped
    Failed(String), // error state
}
```

---

## 4. Executor (In-Sandbox Binary)

A minimal static binary (compiled with musl) that runs inside the sandbox. It is the **only trusted process** inside the sandbox.

### Protocol

Reads JSON lines from stdin, writes JSON lines to stdout.

**Input:**
```json
{"action":"exec","language":"python","code":"print(42)","timeout_ms":30000}
```

**Output:**
```json
{"stdout":"42\n","stderr":"","exit_code":0,"execution_time_ms":8,"timed_out":false,"oom_killed":false}
```

### Behavior

1. Read one JSON line from stdin
2. Write code to `/workspace/code.{ext}`
3. Spawn language runtime: `python3 /workspace/code.py`
4. Capture stdout and stderr (capped at `max_output_bytes`)
5. Wait with timeout — kill child if exceeded
6. Detect OOM kill via exit code 137 or cgroup memory events
7. Write result JSON to stdout
8. Loop (wait for next command, or exit on stdin EOF)

The executor does NOT validate, authenticate, or enforce limits — that's the manager's job. The executor only runs code and reports results.

---

## 5. ProcessSandbox (SandboxRuntime implementation)

Uses youki's `libcontainer` crate for container lifecycle and `oci-spec` crate for generating OCI config. No shell commands — everything is native Rust API calls.

### Dependencies

```rust
libcontainer   // youki's container management library
oci-spec       // OCI runtime spec types (config.json structs)
nix            // fd/pipe management, signal handling
```

### Internal State

```rust
pub struct ProcessSandbox {
    config: ProcessSandboxConfig,
    /// Active containers: sandbox_id → ContainerHandle
    containers: RwLock<HashMap<SandboxId, ContainerHandle>>,
}

struct ContainerHandle {
    container_id: String,
    bundle_path: PathBuf,
    stdin: ChildStdin,       // pipe to executor
    stdout: BufReader<ChildStdout>, // pipe from executor
}

struct ProcessSandboxConfig {
    rootfs_base: HashMap<Language, PathBuf>,  // /var/lib/sandcastle/rootfs/{lang}
    executor_path: PathBuf,                    // /var/lib/sandcastle/bin/sandcastle-executor
    sandbox_dir: PathBuf,                      // /tmp/sandcastle/
}
```

### Lifecycle Implementation

#### `create(config) → SandboxId`

```
1. Generate sandbox_id (UUID v4, prefixed "sb-")
2. Create bundle directory: /tmp/sandcastle/{sandbox_id}/
3. Prepare rootfs:
   a. Create /tmp/sandcastle/{sandbox_id}/rootfs/
   b. Bind-mount (or overlay-mount) base rootfs for language (read-only layer)
   c. Add writable upper layer for /workspace, /tmp
   d. Copy executor binary into rootfs
4. Generate OCI config.json using oci-spec crate:
   - process.args = ["/sandcastle/executor"]
   - process.cwd = "/workspace"
   - process.user = { uid: 1000, gid: 1000 }
   - linux.namespaces = [pid, mount, network, user, uts]
   - linux.resources.memory.limit = config.limits.memory_mb * 1MB
   - linux.resources.cpu.quota/period = config.limits.cpu_quota
   - linux.resources.pids.limit = config.limits.max_pids
   - linux.seccomp = allowlist of safe syscalls
   - mounts = [/proc, /dev/null, /dev/urandom, /workspace (tmpfs)]
5. Write config.json to bundle directory
6. Call libcontainer::ContainerBuilder::new(sandbox_id, bundle_path)
       .as_init()
       .build()
7. Return sandbox_id
```

#### `start(sandbox_id)`

```
1. Lookup container from containers map
2. container.start()
   → libcontainer applies namespaces, cgroups, seccomp from config.json
   → pivot_root into rootfs
   → exec(executor) — executor starts waiting on stdin
3. Store stdin/stdout pipes in ContainerHandle
```

#### `execute(sandbox_id, request) → ExecResult`

```
1. Lookup ContainerHandle
2. Write to stdin pipe:
   {"action":"exec","language":"python","code":"...","timeout_ms":30000}
3. Read from stdout pipe (with tokio::time::timeout):
   {"stdout":"...","stderr":"...","exit_code":0,"execution_time_ms":12}
4. If timeout expires:
   a. Kill the user's subprocess (executor handles this internally)
   b. Return ExecResult { timed_out: true, ... }
5. Parse and return ExecResult
```

#### `upload_file(sandbox_id, host_path, sandbox_path) → bytes copied`

```
1. Lookup ContainerHandle → get container PID
2. Target path: /proc/{pid}/root/workspace/{sandbox_path}
   (/proc/{pid}/root/ is the container's root filesystem as seen from host)
3. std::fs::copy(host_path, target_path)
4. Return bytes copied
```

#### `download_file(sandbox_id, sandbox_path, host_path) → bytes copied`

```
1. Lookup ContainerHandle → get container PID
2. Source path: /proc/{pid}/root/workspace/{sandbox_path}
3. std::fs::copy(source_path, host_path)
4. Return bytes copied
```

#### `destroy(sandbox_id)`

```
1. Lookup and remove ContainerHandle from map
2. container.kill(Signal::SIGKILL)
3. container.delete()
   → libcontainer removes cgroup, cleans up namespaces
4. Unmount and remove bundle directory: /tmp/sandcastle/{sandbox_id}/
```

### OCI Config Generation

Using the `oci-spec` crate, we build config.json programmatically:

```rust
use oci_spec::runtime::{Spec, ProcessBuilder, LinuxBuilder, LinuxResourcesBuilder, ...};

fn build_oci_config(config: &SandboxConfig) -> Spec {
    let process = ProcessBuilder::default()
        .args(vec!["/sandcastle/executor".into()])
        .cwd("/workspace".into())
        .user(LinuxUserBuilder::default().uid(1000u32).gid(1000u32).build()?)
        .env(build_env_vars(&config.env_vars))
        .build()?;

    let linux = LinuxBuilder::default()
        .namespaces(vec![
            LinuxNamespace { typ: PID, path: None },
            LinuxNamespace { typ: Mount, path: None },
            LinuxNamespace { typ: Network, path: None },
            LinuxNamespace { typ: User, path: None },
            LinuxNamespace { typ: Uts, path: None },
        ])
        .resources(
            LinuxResourcesBuilder::default()
                .memory(LinuxMemory { limit: Some(config.limits.memory_mb as i64 * 1_048_576) })
                .cpu(LinuxCpu { quota: Some(config.limits.cpu_quota as i64 * 1000), period: Some(100_000) })
                .pids(LinuxPids { limit: config.limits.max_pids as i64 })
                .build()?
        )
        .seccomp(build_seccomp_profile())
        .build()?;

    SpecBuilder::default()
        .version("1.0.2")
        .process(process)
        .root(Root { path: "rootfs".into(), readonly: Some(false) })
        .mounts(build_mounts())
        .linux(linux)
        .build()?
}
```

### Seccomp Profile

Allowlist approach — deny everything by default, allow only safe syscalls:

```
Allowed syscalls (~60):
  read, write, close, fstat, lseek, mmap, mprotect, munmap, brk,
  rt_sigaction, rt_sigprocmask, ioctl (limited), access, pipe, pipe2,
  dup, dup2, dup3, socket (AF_UNIX only), clone (no CLONE_NEWUSER),
  execve, wait4, kill (own PID only), getpid, getppid,
  fcntl, flock, fsync, fdatasync, truncate, ftruncate,
  getcwd, chdir, mkdir, rmdir, unlink, rename, link, symlink,
  readlink, chmod, chown, stat, lstat, openat, mkdirat,
  newfstatat, unlinkat, renameat, futex, clock_gettime,
  epoll_create, epoll_ctl, epoll_wait, eventfd2,
  getrandom, pread64, pwrite64, readv, writev, ...

Blocked syscalls (deny with EPERM):
  mount, umount, pivot_root, reboot, swapon, swapoff,
  kexec_load, init_module, delete_module, ptrace,
  personality, unshare, setns, bpf, userfaultfd,
  perf_event_open, acct, settimeofday, stime, ...
```

---

## 6. Rootfs Construction

### Strategy: Docker Export

Each language rootfs is built by exporting a Docker container. Run once during setup, reused for all sandboxes.

### Build Process

```
rootfs/build.sh:

For each language (python, node, bash):
  1. docker create --name sc-{lang} {base_image}
     (python:3.12-slim, node:20-slim, bash:5)
  2. docker export sc-{lang} → extract to /var/lib/sandcastle/rootfs/{lang}/
  3. docker rm sc-{lang}
  4. Copy sandcastle-executor binary into rootfs:
     cp target/release/sandcastle-executor /var/lib/sandcastle/rootfs/{lang}/sandcastle/executor
  5. Create /workspace directory inside rootfs
  6. Set permissions (executor owned by root, /workspace writable by uid 1000)
```

### Rootfs Layout (per language)

```
/var/lib/sandcastle/rootfs/python/
├── bin/                  # busybox, sh
├── usr/
│   ├── bin/python3       # Python runtime
│   └── lib/python3.12/   # Standard library
├── lib/                  # shared libraries (glibc, libssl, etc.)
├── sandcastle/
│   └── executor          # our static musl binary
├── workspace/            # user code directory (writable)
├── tmp/                  # temp dir (writable)
├── dev/                  # populated at runtime (null, urandom)
├── proc/                 # mounted at runtime
└── etc/
    └── passwd            # minimal (root + sandbox user)
```

### Size Estimates

| Language | Base Image | Rootfs Size |
|----------|-----------|-------------|
| Python | python:3.12-slim | ~120MB |
| Node.js | node:20-slim | ~180MB |
| Bash | bash:5 + coreutils | ~15MB |

These are one-time costs. Each sandbox bind-mounts the rootfs read-only and gets a writable overlay for /workspace.

---

## 7. Configuration

### `sandcastle.toml`

```toml
[server]
transport = "stdio"                      # "stdio" only in Phase 1

[defaults]
language = "python"
timeout_seconds = 30
memory_mb = 512
cpu_quota = 100                          # 100 = 1 core
max_pids = 64
max_disk_mb = 512
max_output_bytes = 1048576               # 1 MB

[limits]
max_sessions = 50
session_timeout_seconds = 300            # 5 min idle timeout
max_memory_mb = 4096                     # ceiling for any single sandbox
max_timeout_seconds = 600
max_file_size_bytes = 10485760           # 10 MB

[files]
allowed_input_dirs = ["/tmp/sandcastle/input"]
output_dir = "/tmp/sandcastle/output"

[rootfs]
python = "/var/lib/sandcastle/rootfs/python"
javascript = "/var/lib/sandcastle/rootfs/node"
bash = "/var/lib/sandcastle/rootfs/bash"
executor = "/var/lib/sandcastle/bin/sandcastle-executor"
```

---

## 8. Implementation Order

| Step | Crate | What | Why first |
|------|-------|------|-----------|
| 1 | sandcastle-runtime | Trait + types | Everything depends on these types |
| 2 | sandcastle-executor | In-sandbox binary | Can test independently inside a manual chroot |
| 3 | ProcessSandbox | libcontainer-based impl + OCI config gen | Core sandbox logic |
| 4 | sandcastle-manager | Session + file validation | Wires runtime to business logic |
| 5 | sandcastle-server | MCP protocol + CLI | Final layer, connects everything |
| 6 | rootfs/ | Docker export build scripts | Needed for real execution |
| 7 | tests/ | Integration + E2E | Validate the full stack |

---

## 9. Dependencies

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

# sandcastle-executor only (static musl, no async)
# minimal: serde + serde_json only

# ProcessSandbox
libcontainer = "0.4"        # youki's container management library
oci-spec = "0.7"            # OCI runtime spec types
nix = { version = "0.29", features = ["process", "mount", "signal"] }
```

### Key Rule

**No `Command::new()` or shell commands in Rust code.** Everything goes through Rust crates. If a crate doesn't exist for something, escalate for approval before using a shell command.
