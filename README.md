# 🏰 Sandcastle

**Tiered sandbox execution for AI agents via MCP.**

Sandcastle is an MCP (Model Context Protocol) tool that gives any AI agent secure, sandboxed code execution. Agents call `execute_code` — Sandcastle handles isolation, resource limits, network restrictions, and cleanup.

## Why Sandcastle?

AI agents need to execute code. But executing untrusted code on your infrastructure is dangerous:
- Code could exfiltrate data over the network
- Code could consume unlimited CPU/memory
- Code could access the host filesystem
- Code could run indefinitely

Sandcastle solves this by providing **sandboxed execution as an MCP tool**. Any AI agent that speaks MCP can safely execute code without the agent developer thinking about isolation.

## Key Features

- **🔒 Three isolation levels** — Choose the right security/performance tradeoff
- **🔌 MCP-native** — Works with any MCP-compatible agent (Claude, GPT, Copilot, custom)
- **⚡ Fast** — Pre-warmed sandbox pools, snapshot-based restore
- **🌐 Network control** — No network by default, allowlist specific domains
- **📦 Multi-language** — Python, JavaScript, Bash, Rust, Go, and more
- **⏱️ Time-bound** — Automatic timeout and cleanup
- **💰 Usage-based** — Pay for what you use (execution seconds)

## Isolation Levels

| Level | Backend | Boot Time | CPU Overhead | Security | Default |
|-------|---------|-----------|-------------|----------|---------|
| `low` | Linux namespaces + seccomp + cgroups | ~5ms | ~0% | Process isolation | |
| `medium` | gVisor (runsc) | ~50ms | ~20% | Syscall interception | ✅ |
| `high` | Firecracker microVM | ~250ms | ~1% | Hardware virtualization (KVM) | |

- **Low**: For trusted code — math calculations, text formatting, data parsing. Fastest, lightest.
- **Medium** (default): For general untrusted code. gVisor intercepts every syscall — no direct kernel access. Good balance of speed and security.
- **High**: For fully untrusted code — user submissions, internet-sourced code, potential malware. Each execution runs in its own Firecracker microVM with hardware-level isolation via KVM.

## MCP Tools

### `create_sandbox`
Create a persistent sandbox session for multi-step execution.

```json
{
  "name": "create_sandbox",
  "arguments": {
    "language": "python",
    "isolation": "medium",
    "timeout_seconds": 300,
    "memory_mb": 512,
    "cpu_cores": 1,
    "allowed_domains": ["pypi.org", "files.pythonhosted.org"],
    "env_vars": {"API_KEY": "..."}
  }
}
```

Returns: `{ "sandbox_id": "sb-a1b2c3d4" }`

### `execute_code`
Execute code in a new ephemeral sandbox (one-shot) or existing sandbox.

```json
{
  "name": "execute_code",
  "arguments": {
    "code": "print('hello world')",
    "language": "python",
    "isolation": "medium",
    "timeout_seconds": 30
  }
}
```

Returns:
```json
{
  "stdout": "hello world\n",
  "stderr": "",
  "exit_code": 0,
  "execution_time_ms": 45,
  "timed_out": false
}
```

### `execute_in_sandbox`
Execute code in an existing sandbox session (preserves state between calls).

```json
{
  "name": "execute_in_sandbox",
  "arguments": {
    "sandbox_id": "sb-a1b2c3d4",
    "code": "x = 42",
    "timeout_seconds": 10
  }
}
```

### `upload_file`
Inject a file into a sandbox.

```json
{
  "name": "upload_file",
  "arguments": {
    "sandbox_id": "sb-a1b2c3d4",
    "path": "/workspace/data.csv",
    "content": "name,age\nAlice,30\nBob,25"
  }
}
```

### `download_file`
Pull a file out of a sandbox.

```json
{
  "name": "download_file",
  "arguments": {
    "sandbox_id": "sb-a1b2c3d4",
    "path": "/workspace/output.json"
  }
}
```

### `destroy_sandbox`
Destroy a sandbox and all its data.

```json
{
  "name": "destroy_sandbox",
  "arguments": {
    "sandbox_id": "sb-a1b2c3d4"
  }
}
```

## Quick Start

### As an MCP tool (stdio)
```json
{
  "mcpServers": {
    "sandcastle": {
      "command": "sandcastle",
      "args": ["serve", "--stdio"]
    }
  }
}
```

### As an HTTP server
```bash
sandcastle serve --http --port 8090
```

### Self-hosted
```bash
# Install
curl -fsSL https://sandcastle.dev/install.sh | sh

# Start the sandbox pool
sandcastle start --pool-size 10 --default-isolation medium

# The MCP server is now ready for connections
```

## Architecture

```
┌─────────────────────────────────┐
│         AI Agent (Any)          │
│   Claude / GPT / Copilot / ... │
└──────────┬──────────────────────┘
           │ MCP Protocol (stdio or HTTP)
┌──────────▼──────────────────────┐
│      Sandcastle MCP Server      │
│         (Rust binary)           │
├─────────────────────────────────┤
│       Sandbox Pool Manager      │
│  ┌─────┐  ┌───────┐  ┌──────┐  │
│  │ Low │  │Medium │  │ High │  │
│  │Pool │  │ Pool  │  │ Pool │  │
│  └──┬──┘  └───┬───┘  └──┬───┘  │
│     │         │          │      │
│  ns+sec   gVisor     Firecracker│
│  +cgroup  (runsc)    (KVM)      │
└─────────────────────────────────┘
           │
    ┌──────▼──────┐
    │  Executor   │
    │ (inside     │
    │  sandbox)   │
    │             │
    │ Run code    │
    │ Capture I/O │
    │ Return      │
    └─────────────┘
```

## Security Model

### Network
- **Default: no network access** — sandbox cannot make any outbound connections
- **Allowlist**: specify exact domains the sandbox can reach (e.g., package registries)
- **No inbound**: nothing from outside can reach into the sandbox
- Communication between host and sandbox uses vsock (Firecracker) or Unix pipes (gVisor/namespaces) — not TCP/IP

### Filesystem
- **Ephemeral**: sandbox filesystem is destroyed after use
- **No host mounts**: sandbox cannot see the host filesystem
- **Working directory**: `/workspace` — all file operations scoped here

### Resources
- **CPU**: configurable core count, enforced via cgroups
- **Memory**: configurable limit, OOM-killed if exceeded
- **Time**: hard timeout, process killed if exceeded
- **Disk**: configurable disk quota

### Isolation Boundaries
| Threat | Low | Medium | High |
|--------|-----|--------|------|
| Read host files | ✅ Blocked | ✅ Blocked | ✅ Blocked |
| Network exfiltration | ✅ Blocked | ✅ Blocked | ✅ Blocked |
| Kernel exploit | ❌ Possible | ✅ Blocked (gVisor) | ✅ Blocked (KVM) |
| Container escape | ❌ Possible | ⚠️ Hard | ✅ Blocked |
| CPU/mem abuse | ✅ Cgroups | ✅ Cgroups | ✅ VM limits |

## Supported Languages

| Language | Runtime | Image Size |
|----------|---------|------------|
| Python 3.12 | CPython | ~80MB |
| JavaScript/Node 22 | Node.js | ~60MB |
| Bash | GNU Bash | ~10MB |
| Rust | rustc + cargo | ~200MB |
| Go | go compiler | ~150MB |
| TypeScript | Node + tsx | ~70MB |

## Comparison

| | Sandcastle | E2B | Modal | Docker |
|---|---|---|---|---|
| MCP-native | ✅ | ❌ (SDK only) | ❌ (Python SDK) | ❌ |
| Isolation tiers | 3 levels | Firecracker only | gVisor only | Namespace only |
| Default network | Blocked | Open | Open | Open |
| Self-hostable | ✅ | ❌ (cloud only) | ❌ (cloud only) | ✅ |
| Boot time | 5-250ms | ~250ms | ~50ms | ~500ms |
| Open source | ✅ | Partial | ❌ | ✅ |

## License

Apache-2.0
