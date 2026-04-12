# MCP Tool Specification

## Overview

Sandcastle exposes 6 MCP tools for sandboxed code execution. All tools follow the MCP protocol and can be used by any MCP-compatible AI agent.

## Transport

Sandcastle supports two MCP transports:

| Transport | Use Case | Connection |
|-----------|----------|------------|
| **stdio** | Local agent (same machine) | stdin/stdout pipes |
| **HTTP + SSE** | Remote agent (over network) | HTTP POST for requests, SSE for responses |

## Tools

---

### 1. `execute_code`

Execute code in a new ephemeral sandbox. The sandbox is created, code runs, result is returned, and sandbox is destroyed — all in one call.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "code": {
      "type": "string",
      "description": "The code to execute"
    },
    "language": {
      "type": "string",
      "enum": ["python", "javascript", "bash", "typescript", "rust", "go"],
      "description": "Programming language. Default: python"
    },
    "isolation": {
      "type": "string",
      "enum": ["low", "medium", "high"],
      "description": "Isolation level. Default: medium"
    },
    "timeout_seconds": {
      "type": "integer",
      "minimum": 1,
      "maximum": 600,
      "description": "Max execution time. Default: 30"
    },
    "memory_mb": {
      "type": "integer",
      "minimum": 64,
      "maximum": 8192,
      "description": "Memory limit in MB. Default: 512"
    },
    "allowed_domains": {
      "type": "array",
      "items": { "type": "string" },
      "description": "Domains the sandbox can access. Default: none (no network)"
    }
  },
  "required": ["code"]
}
```

**Output Schema:**
```json
{
  "type": "object",
  "properties": {
    "stdout": { "type": "string" },
    "stderr": { "type": "string" },
    "exit_code": { "type": "integer" },
    "execution_time_ms": { "type": "integer" },
    "timed_out": { "type": "boolean" },
    "oom_killed": { "type": "boolean" }
  }
}
```

**Example:**
```json
// Request
{
  "name": "execute_code",
  "arguments": {
    "code": "import math\nprint(math.factorial(20))",
    "language": "python",
    "isolation": "low",
    "timeout_seconds": 5
  }
}

// Response
{
  "stdout": "2432902008176640000\n",
  "stderr": "",
  "exit_code": 0,
  "execution_time_ms": 12,
  "timed_out": false,
  "oom_killed": false
}
```

---

### 2. `create_sandbox`

Create a persistent sandbox session. The sandbox stays alive until explicitly destroyed or timeout expires. Useful for multi-step execution where state needs to persist between calls.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "language": {
      "type": "string",
      "enum": ["python", "javascript", "bash", "typescript", "rust", "go"],
      "description": "Programming language. Default: python"
    },
    "isolation": {
      "type": "string",
      "enum": ["low", "medium", "high"],
      "description": "Isolation level. Default: medium"
    },
    "timeout_seconds": {
      "type": "integer",
      "minimum": 1,
      "maximum": 3600,
      "description": "Session lifetime. Default: 300"
    },
    "memory_mb": {
      "type": "integer",
      "minimum": 64,
      "maximum": 8192,
      "description": "Memory limit in MB. Default: 512"
    },
    "cpu_cores": {
      "type": "integer",
      "minimum": 1,
      "maximum": 4,
      "description": "CPU core count. Default: 1"
    },
    "allowed_domains": {
      "type": "array",
      "items": { "type": "string" },
      "description": "Domains the sandbox can access. Default: none"
    },
    "env_vars": {
      "type": "object",
      "additionalProperties": { "type": "string" },
      "description": "Environment variables to set in the sandbox"
    }
  }
}
```

**Output Schema:**
```json
{
  "type": "object",
  "properties": {
    "sandbox_id": { "type": "string" },
    "language": { "type": "string" },
    "isolation": { "type": "string" },
    "expires_at": { "type": "string", "format": "date-time" }
  }
}
```

---

### 3. `execute_in_sandbox`

Execute code in an existing sandbox. State from previous executions is preserved (variables, files, installed packages).

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "sandbox_id": {
      "type": "string",
      "description": "ID returned by create_sandbox"
    },
    "code": {
      "type": "string",
      "description": "The code to execute"
    },
    "timeout_seconds": {
      "type": "integer",
      "minimum": 1,
      "maximum": 600,
      "description": "Max execution time for this call. Default: 30"
    }
  },
  "required": ["sandbox_id", "code"]
}
```

**Output Schema:** Same as `execute_code`.

**Example (multi-step):**
```json
// Step 1: Define a function
{ "name": "execute_in_sandbox", "arguments": {
    "sandbox_id": "sb-a1b2c3d4",
    "code": "def greet(name): return f'Hello, {name}!'"
}}
// → exit_code: 0

// Step 2: Call the function (state preserved)
{ "name": "execute_in_sandbox", "arguments": {
    "sandbox_id": "sb-a1b2c3d4",
    "code": "print(greet('World'))"
}}
// → stdout: "Hello, World!\n"
```

---

### 4. `upload_file`

Upload a file from the host into an existing sandbox's `/workspace` directory. The file is read from the host filesystem and injected into the sandbox — the agent provides a host path, not file content.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "sandbox_id": {
      "type": "string",
      "description": "ID returned by create_sandbox"
    },
    "host_path": {
      "type": "string",
      "description": "Absolute path to file on host machine. Must be within allowed input directories (configured in sandcastle.toml)"
    },
    "sandbox_path": {
      "type": "string",
      "description": "Destination path inside sandbox (relative to /workspace). E.g., 'data.csv' or 'src/main.py'"
    }
  },
  "required": ["sandbox_id", "host_path", "sandbox_path"]
}
```

**Output Schema:**
```json
{
  "type": "object",
  "properties": {
    "sandbox_path": { "type": "string", "description": "Full path inside sandbox" },
    "size_bytes": { "type": "integer" }
  }
}
```

**Security:** Only files within configured `allowed_input_dirs` can be uploaded. Sandcastle rejects paths outside these directories (prevents reading arbitrary host files).

---

### 5. `download_file`

Extract a file from the sandbox to the host filesystem. Sandcastle copies the file from the sandbox to a designated output directory on the host — the agent receives the host path, not the file content.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "sandbox_id": {
      "type": "string",
      "description": "ID returned by create_sandbox"
    },
    "sandbox_path": {
      "type": "string",
      "description": "Path inside sandbox (relative to /workspace)"
    },
    "host_path": {
      "type": "string",
      "description": "Optional. Destination path on host. If omitted, Sandcastle writes to {output_dir}/{sandbox_id}/{filename}"
    }
  },
  "required": ["sandbox_id", "sandbox_path"]
}
```

**Output Schema:**
```json
{
  "type": "object",
  "properties": {
    "host_path": { "type": "string", "description": "Absolute path where file was written on host" },
    "size_bytes": { "type": "integer" },
    "scanned": { "type": "boolean", "description": "Whether malware scan was performed (Phase 3)" }
  }
}
```

**Security:**
- Files are written to a designated `output_dir` (configured in sandcastle.toml). Agent cannot write to arbitrary host paths.
- Max file size enforced (default 10MB, configurable).
- **Phase 3**: Optional malware scanning (YARA rules + ClamAV) before writing to host. If malware detected, file is quarantined and agent receives an error.
- File content never passes through the MCP response — only the host path is returned. This avoids bloating MCP messages with large binary data.

---

### 6. `destroy_sandbox`

Destroy a sandbox and all its data immediately.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "sandbox_id": {
      "type": "string",
      "description": "ID returned by create_sandbox"
    }
  },
  "required": ["sandbox_id"]
}
```

**Output Schema:**
```json
{
  "type": "object",
  "properties": {
    "destroyed": { "type": "boolean" }
  }
}
```

## Error Handling

All tools return MCP-standard errors:

| Code | Meaning | Example |
|------|---------|---------|
| `InvalidParams` | Bad input | Missing code, invalid language |
| `InternalError` | Sandbox creation/execution failed | Failed to spawn sandbox |
| `-1` (custom) | Sandbox not found | Invalid sandbox_id |
| `-2` (custom) | Resource limit exceeded | No free sandboxes in pool |
| `-3` (custom) | Sandbox expired | Session timed out |

Error response format:
```json
{
  "error": {
    "code": -1,
    "message": "Sandbox sb-a1b2c3d4 not found or expired"
  }
}
```

## Rate Limits (Managed Service)

| Tier | Concurrent Sandboxes | Executions/min | Max Session Duration |
|------|---------------------|---------------|---------------------|
| Free | 2 | 30 | 5 min |
| Pro | 20 | 300 | 60 min |
| Enterprise | Unlimited | Unlimited | 24 hours |

Self-hosted: no rate limits (you control the infrastructure).
