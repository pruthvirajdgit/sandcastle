# Phase 1 — Product Goals

## Objective

Ship a working Sandcastle binary that an AI agent can use via MCP to execute code safely on a single machine. Phase 1 proves the core value proposition: **"plug in one MCP tool, get secure code execution."**

## Success Criteria

1. An MCP client can connect via stdio, send `execute_code`, and get correct output back.
2. Code runs in an isolated process — cannot access host filesystem, network, or other sandboxes.
3. Resource limits (memory, CPU, timeout, PIDs) are enforced and kill runaway processes.
4. File upload/download works via host paths with directory allowlisting.
5. Multi-language: Python, JavaScript, and Bash all work.
6. One-shot and session-based execution both work end-to-end.

## What Ships

### MCP Tools (6 total)

| Tool | Mode | Description |
|------|------|-------------|
| `execute_code` | One-shot | Create sandbox → run code → return result → destroy |
| `create_sandbox` | Session | Create a persistent sandbox, return `session_id` |
| `execute_in_session` | Session | Run code in existing sandbox (state preserved) |
| `upload_file` | Session | Copy file from host `allowed_input_dirs` into sandbox `/workspace` |
| `download_file` | Session | Copy file from sandbox `/workspace` to host `output_dir` |
| `destroy_sandbox` | Session | Kill sandbox and clean up all resources |

### Isolation (Low only)

- Linux namespaces: PID, mount, network, user, UTS
- Seccomp-BPF: whitelist of allowed syscalls
- Cgroups v2: memory, CPU, PIDs limits
- No network interfaces inside sandbox (network-zero)
- Separate rootfs per sandbox (pivot_root)

### Languages

| Language | Runtime | Extension |
|----------|---------|-----------|
| Python | `python3` | `.py` |
| JavaScript | `node` | `.js` |
| Bash | `bash` | `.sh` |

### File Security

- **Upload**: Only from directories listed in `allowed_input_dirs` config
- **Download**: Only to the configured `output_dir`
- **Path traversal**: Rejected (no `..` components)
- **File content**: Never passes through MCP messages — only host paths exchanged
- **Max file size**: Configurable (default 10MB)

### Configuration

Single `sandcastle.toml` file controls everything:
- Default resource limits
- Allowed input/output directories
- Rootfs paths per language
- Transport selection (stdio only in Phase 1)

### CLI

```bash
# Start as MCP server (stdio)
sandcastle serve --stdio

# Quick test (not via MCP, for debugging)
sandcastle run --language python --code "print('hello')"

# Show config
sandcastle config show
```

## What Does NOT Ship

| Feature | Why not Phase 1 |
|---------|----------------|
| gVisor (medium isolation) | Phase 2 — requires runsc installation and OCI integration |
| Firecracker (high isolation) | Phase 2 — requires KVM, kernel images, rootfs disk images |
| Pre-warmed pool | Phase 2 — optimization, not correctness |
| HTTP+SSE transport | Phase 2 — stdio is sufficient for local agents |
| Network allowlisting | Phase 2 — Phase 1 is network-zero only |
| Malware scanning | Phase 3 — security hardening |
| Rate limiting | Phase 3 — managed service feature |
| Audit logging | Phase 3 — compliance feature |
| Multi-tenancy | Phase 3+ — single-user in Phase 1 |

## User Experience

### One-shot (most common)

Agent sends one MCP call, gets result:

```
Agent: execute_code(code="import math; print(math.pi)", language="python")
Sandcastle: { stdout: "3.141592653589793\n", exit_code: 0 }
```

No session management. Fire and forget.

### Session-based (file workflows)

Agent needs state or files across multiple calls:

```
Agent: create_sandbox(language="python")
Sandcastle: { session_id: "sb-abc123" }

Agent: upload_file(session_id="sb-abc123", host_path="/data/input.csv", sandbox_path="input.csv")
Sandcastle: { sandbox_path: "/workspace/input.csv", size_bytes: 2048 }

Agent: execute_in_session(session_id="sb-abc123", code="import pandas as pd; df = pd.read_csv('input.csv'); df.to_json('output.json')")
Sandcastle: { stdout: "", exit_code: 0 }

Agent: download_file(session_id="sb-abc123", sandbox_path="output.json")
Sandcastle: { host_path: "/output/sb-abc123/output.json", size_bytes: 4096 }

Agent: destroy_sandbox(session_id="sb-abc123")
Sandcastle: { destroyed: true }
```

### Error cases

| Scenario | Behavior |
|----------|----------|
| Code times out | `timed_out: true`, process killed, partial stdout returned |
| Code exceeds memory | `oom_killed: true`, process killed |
| Fork bomb | Blocked by `pids.max` cgroup, error in stderr |
| Infinite output | Truncated at `max_output_bytes` |
| Upload from non-allowed dir | Error: "host path not in allowed_input_dirs" |
| Session expired | Error: "sandbox not found or expired" |
| Path traversal attempt | Error: "invalid path: traversal not allowed" |
