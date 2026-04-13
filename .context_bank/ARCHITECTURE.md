# Architecture

## System Design

Sandcastle follows a **layered architecture** with clean separation between protocol handling, session management, and sandbox execution:

```
┌───────────────────────────────────────────────────────────┐
│                    AI Agent (Claude, GPT, etc.)            │
└────────────────────────┬──────────────────────────────────┘
                         │ MCP Protocol (JSON-RPC over stdio)
┌────────────────────────▼──────────────────────────────────┐
│  sandcastle-server                                        │
│  Entry point. Parses MCP tool calls, delegates to manager │
│  Uses: rmcp crate for MCP protocol                        │
│  Parses `isolation` param → passes to manager             │
├───────────────────────────────────────────────────────────┤
│  sandcastle-manager                                       │
│  Session lifecycle, file transfer, input validation       │
│  Routes requests to correct backend by IsolationLevel     │
│  Tracks active sessions, enforces limits, reaps expired   │
├───────────────────────────────────────────────────────────┤
│  sandcastle-runtime  (trait)                              │
│  SandboxRuntime trait — the interface all backends impl   │
│  IsolationLevel enum: Low, Medium, High                   │
│  Shared types: SandboxId, ExecRequest, ExecResult, etc.   │
├───────────────────────────────────────────────────────────┤
│  sandcastle-process  (Low isolation)                      │   sandcastle-gvisor  (Medium isolation)    │
│  Linux namespace containers via libcontainer (youki)      │   gVisor containers via runsc CLI           │
│  OCI spec: PID + Mount namespaces                         │   OCI spec: PID + Mount + IPC + UTS + Net  │
│  Pipe-based I/O, container lifecycle                      │   Pipe-based I/O via `runsc run`           │
└────────────────────────┬──────────────────────────────────┘
                         │ Pipes (stdin/stdout)
┌────────────────────────▼──────────────────────────────────┐
│  sandcastle-executor                                      │
│  Runs INSIDE the container as PID 1                       │
│  Reads JSON commands from stdin, spawns language runtime,  │
│  captures output, writes JSON response to stdout          │
│  Same binary used by ALL backends                         │
└───────────────────────────────────────────────────────────┘
```

## Dependency Graph (Crates)

```
sandcastle-server
  ├── sandcastle-manager
  │     └── sandcastle-runtime (trait + types)
  ├── sandcastle-process
  │     └── sandcastle-runtime
  ├── sandcastle-gvisor
  │     └── sandcastle-runtime
  └── sandcastle-runtime

sandcastle-executor (standalone binary, no deps on other crates)
```

## Data Flow: execute_code (One-Shot)

```
1. Agent sends MCP tool call: execute_code(code="print(42)", language="python", isolation="medium")
2. sandcastle-server receives via rmcp, routes to SandcastleTools::execute_code()
3. Server parses isolation param → IsolationLevel::Medium
4. SandcastleTools delegates to SandboxManager::execute_oneshot(code, language, timeout, isolation)
5. Manager looks up runtime for IsolationLevel::Medium → GvisorSandbox
6. Manager calls runtime.create() → runtime.start() → runtime.execute() → runtime.destroy()
7. For GvisorSandbox:
   a. create(): Generates ID "gv-{uuid}", creates OCI bundle (5 namespaces), symlinks rootfs
   b. start(): Spawns `runsc run --bundle <path> <id>`, waits for executor readiness
   c. execute(): Writes JSON to stdin pipe, reads JSON response from stdout pipe
   d. destroy(): `runsc kill` + `runsc delete --force` + cleanup dirs
8. For ProcessSandbox:
   a. create(): Generates ID "sc-{uuid}", creates OCI config, builds container via libcontainer
   b. start(): Container::load().start() resumes blocked init
   c. execute(): Same JSON pipe protocol
   d. destroy(): Container::kill + delete + cleanup dirs
9. Result propagates back up → serialized as MCP response → sent to agent
```

## Data Flow: Persistent Session

```
1. Agent: create_sandbox(language="python") → returns session_id "sb-{uuid}"
   - Manager stores mapping: session_id → sandbox_id
   - Container stays running (executor waiting on stdin)

2. Agent: execute_in_session(session_id, code="x = 42")
   - Manager looks up sandbox_id from session
   - Sends JSON command to existing container's stdin pipe
   - Reads response from stdout pipe
   - State persists across calls (files in /workspace)

3. Agent: execute_in_session(session_id, code="print(x)")
   - Same container, same stdin/stdout pipes
   - NOTE: Python state does NOT persist between exec calls (each is a new process)
   - Files written to /workspace DO persist

4. Agent: destroy_sandbox(session_id)
   - Manager removes session, calls stop + destroy on runtime
```

## Container Isolation Model

### Multi-Backend Routing

The manager routes requests to the correct backend based on `IsolationLevel`:

| Level | Backend | ID Prefix | Namespaces | Container Runtime |
|-------|---------|-----------|------------|-------------------|
| Low | ProcessSandbox | `sc-` | PID + Mount | libcontainer (youki) |
| Medium | GvisorSandbox | `gv-` | PID + Mount + IPC + UTS + Net | runsc (gVisor) |
| High | (Phase 4) | — | Full VM | Firecracker |

Default isolation is **Low**. Agents choose isolation per-request via the `isolation` parameter.

### ProcessSandbox Isolation (Low)

| Resource       | Isolation Mechanism          | Details                           |
|----------------|------------------------------|-----------------------------------|
| PID            | PID namespace                | Container sees only its own PIDs  |
| Filesystem     | Mount namespace + bind mount | Read-only rootfs + /workspace RW  |
| Network        | (not yet enabled)            | Will use network namespace        |
| CPU/Memory     | cgroups v2                   | Configured via ResourceLimits     |
| PIDs           | cgroups v2 pids controller   | Default: max 64 processes         |

### OCI Spec Highlights

```json
{
  "ociVersion": "1.0.2",
  "process": {
    "terminal": false,
    "args": ["/sandbox/executor"],
    "env": ["PATH=...", "SANDBOX_LANGUAGE=python", "HOME=/workspace"],
    "cwd": "/workspace"
  },
  "root": {
    "path": "/var/lib/sandcastle/rootfs/python",
    "readonly": false
  },
  "mounts": [
    {"destination": "/proc", "type": "proc"},
    {"destination": "/dev",  "type": "tmpfs"},
    {"destination": "/workspace", "type": "bind", "source": "<host-workspace-dir>"}
  ],
  "linux": {
    "namespaces": [{"type": "pid"}, {"type": "mount"}],
    "resources": { "pids": { "limit": 64 } }
  }
}
```

### GvisorSandbox Isolation (Medium)

gVisor intercepts all syscalls at the kernel boundary, providing stronger isolation than namespace-only containers:

| Resource       | Isolation Mechanism          | Details                           |
|----------------|------------------------------|-----------------------------------|
| Syscalls       | gVisor Sentry (ptrace)       | All syscalls intercepted + filtered |
| PID            | PID namespace                | Container sees only its own PIDs  |
| Filesystem     | Mount namespace + bind mount | Read-only rootfs + /workspace RW  |
| Network        | Network namespace            | Fully isolated (no connectivity)  |
| IPC            | IPC namespace                | Isolated shared memory            |
| Hostname       | UTS namespace                | Isolated hostname                 |

gVisor uses the `ptrace` platform (no KVM required — works on any VM). runsc state lives in `/run/sandcastle/gvisor` (separate from libcontainer's `/run/sandcastle`).

Key differences from ProcessSandbox:
- **5 namespaces** (pid, mount, ipc, uts, network) vs 2 (pid, mount)
- **runsc run** = create + start + wait in one command (direct stdin/stdout pipes)
- **runsc stderr** redirected to null to prevent JSON protocol corruption
- **Workspace dirs** need chmod 777 for container write access inside gVisor
- Container IDs prefixed `gv-` (vs `sc-` for ProcessSandbox)

The executor binary (`sandcastle-executor`) runs as PID 1 inside the container. It communicates with the host via JSON lines over stdin/stdout:

### Request (host → executor, via stdin pipe)
```json
{
  "action": "exec",
  "language": "python",
  "code": "print('hello')",
  "timeout_ms": 30000,
  "max_output_bytes": 1048576
}
```

### Response (executor → host, via stdout pipe)
```json
{
  "stdout": "hello\n",
  "stderr": "",
  "exit_code": 0,
  "execution_time_ms": 45,
  "timed_out": false,
  "oom_killed": false
}
```

### Executor Behavior
1. Reads one JSON line from stdin
2. Writes code to `/workspace/code.{ext}` (e.g., `code.py`)
3. Spawns the language runtime: `python3 /workspace/code.py`
4. Polls for completion with timeout
5. Captures stdout/stderr (up to max_output_bytes)
6. Detects OOM kill (exit code 137) and timeout
7. Deletes code file, writes JSON response to stdout
8. Loops back to step 1 (stays alive for multiple exec calls)

## Rootfs Strategy

Each language gets a pre-built rootfs directory created by Docker export:

```
/var/lib/sandcastle/rootfs/
├── python/      # python:3.12-slim exported (~131MB)
├── javascript/  # node:20-slim exported (~212MB)  
└── bash/        # bash:5 exported (~23MB)
```

The rootfs is **shared read-only** across all containers of the same language. Each container gets its own `/workspace` directory via bind mount.

The executor binary (`/sandbox/executor`) is copied into each rootfs. It MUST be statically linked (built with musl target) because the rootfs has a different glibc than the host.

## MCP Tools Exposed

| Tool | Description | Session Required |
|------|-------------|-----------------|
| `execute_code` | One-shot: create sandbox, run code, destroy | No |
| `create_sandbox` | Create persistent session | No |
| `execute_in_session` | Run code in existing session | Yes |
| `upload_file` | Copy file from host into sandbox /workspace | Yes |
| `download_file` | Copy file from sandbox /workspace to host | Yes |
| `destroy_sandbox` | Destroy session and cleanup | Yes |

## Backends

| Backend | Crate | Status | Isolation Level |
|---------|-------|--------|-----------------|
| Linux namespaces (libcontainer) | sandcastle-process | ✅ Working (Phase 1 & 2) | Low |
| gVisor (runsc) | sandcastle-gvisor | ✅ Working (Phase 3) | Medium |
| Firecracker (microVM) | sandcastle-firecracker | ⬜ Phase 4 | High |

All backends implement the same `SandboxRuntime` trait. The manager and server are backend-agnostic.

## Copilot MCP Integration (Phase 2 — Complete)

Sandcastle is registered as an MCP server for GitHub Copilot CLI via:

```
~/.copilot/mcp-config.json
```

```json
{
  "mcpServers": {
    "sandcastle": {
      "type": "stdio",
      "command": "sudo",
      "args": ["/home/azureuser/sandcastle/service/target/debug/sandcastle", "serve", "--transport", "stdio"]
    }
  }
}
```

Once configured, Copilot CLI automatically starts the Sandcastle server and exposes all 6 tools. The agent can then call `execute_code`, `create_sandbox`, etc. as native MCP tools — code executes inside namespace-isolated containers on the same machine.

### Verified Capabilities
- **One-shot execution**: `execute_code("print('hello')", language="python")` → runs in ephemeral container
- **Persistent sessions**: `create_sandbox` → multiple `execute_in_session` calls → `destroy_sandbox`
- **File transfer**: `upload_file` (host → sandbox) → execute code that reads it → `download_file` (sandbox → host)
- **Multi-language**: Python 3.12, JavaScript (Node 20), Bash 5
