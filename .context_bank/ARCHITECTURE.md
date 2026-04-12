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
├───────────────────────────────────────────────────────────┤
│  sandcastle-manager                                       │
│  Session lifecycle, file transfer, input validation       │
│  Tracks active sessions, enforces limits, reaps expired   │
├───────────────────────────────────────────────────────────┤
│  sandcastle-runtime  (trait)                              │
│  SandboxRuntime trait — the interface all backends impl   │
│  Shared types: SandboxId, ExecRequest, ExecResult, etc.   │
├───────────────────────────────────────────────────────────┤
│  sandcastle-process  (implementation)                     │
│  Linux namespace containers via libcontainer (youki)      │
│  OCI spec generation, pipe-based I/O, container lifecycle │
└────────────────────────┬──────────────────────────────────┘
                         │ Pipes (stdin/stdout)
┌────────────────────────▼──────────────────────────────────┐
│  sandcastle-executor                                      │
│  Runs INSIDE the container as PID 1                       │
│  Reads JSON commands from stdin, spawns language runtime,  │
│  captures output, writes JSON response to stdout          │
└───────────────────────────────────────────────────────────┘
```

## Dependency Graph (Crates)

```
sandcastle-server
  ├── sandcastle-manager
  │     └── sandcastle-runtime (trait + types)
  ├── sandcastle-process
  │     └── sandcastle-runtime
  └── sandcastle-runtime

sandcastle-executor (standalone binary, no deps on other crates)
```

## Data Flow: execute_code (One-Shot)

```
1. Agent sends MCP tool call: execute_code(code="print(42)", language="python")
2. sandcastle-server receives via rmcp, routes to SandcastleTools::execute_code()
3. SandcastleTools delegates to SandboxManager::execute_oneshot()
4. Manager calls runtime.create() → runtime.start() → runtime.execute() → runtime.destroy()
5. ProcessSandbox::create():
   a. Generates unique ID: "sc-{uuid}"
   b. Creates OCI config.json (namespaces, mounts, resource limits)
   c. Creates stdin/stdout pipes
   d. Calls ContainerBuilder::new().as_init(bundle).build()
   e. libcontainer forks, sets up namespaces, blocks at notify socket
6. ProcessSandbox::start():
   a. Container::load(state_dir/id).start()
   b. Executor binary begins running inside container, reading stdin
7. ProcessSandbox::execute():
   a. Writes JSON to stdin pipe: {"action":"exec","language":"python","code":"print(42)","timeout_ms":30000}
   b. Reads JSON from stdout pipe: {"stdout":"42\n","stderr":"","exit_code":0,...}
8. ProcessSandbox::destroy():
   a. Container::kill(SIGTERM) → Container::delete(force=true)
   b. Cleans up bundle dir and workspace dir
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

## Container Isolation Model (ProcessSandbox)

Each container gets:

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

## Executor Protocol

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

## Future Backends (Not Yet Implemented)

| Backend | Crate | Status | Isolation Level |
|---------|-------|--------|-----------------|
| Linux namespaces (libcontainer) | sandcastle-process | ✅ Working | Low |
| gVisor (runsc) | sandcastle-gvisor | ⬜ Phase 2 | Medium |
| Firecracker (microVM) | sandcastle-firecracker | ⬜ Phase 3 | High |

All backends implement the same `SandboxRuntime` trait. The manager and server are backend-agnostic.
