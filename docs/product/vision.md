# Sandcastle — Product Vision

## One-liner
Sandcastle is the universal "execute code safely" tool for AI agents.

## Problem
Every AI agent that executes code faces the same problem: **how do you run untrusted code without compromising your infrastructure?** Today, agent developers either:

1. **Skip sandboxing** — run code directly on the host (dangerous)
2. **Use Docker** — basic isolation, but not designed for security (container escapes are common)
3. **Build custom sandboxing** — every team reinvents the wheel (months of engineering)
4. **Use E2B/Modal** — vendor lock-in, cloud-only, no MCP support

## Solution
Sandcastle provides sandboxed code execution as a **plug-and-play MCP tool**. Any AI agent that speaks MCP gets secure code execution with zero integration effort.

```
Agent → MCP call: execute_code("print(2+2)") → "4"
```

That's it. The agent doesn't know or care about VMs, containers, syscalls, or network rules. Sandcastle handles all of it.

## Target Users

### Primary: AI Agent Developers
- Building agents that need to execute code (coding assistants, data analysts, automation)
- Don't want to think about security, isolation, or infrastructure
- Want a simple API: send code, get results

### Secondary: AI Platform Companies
- Running multi-tenant agent platforms (like OneClick.ai)
- Need strong isolation between users' code execution
- Need usage-based billing granularity

### Tertiary: Enterprise Security Teams
- Evaluating code execution for AI tools
- Need compliance-friendly isolation (SOC2, ISO 27001)
- Need audit logs and network controls

## Key Differentiators

### 1. MCP-Native
No SDK to install, no language-specific client. Any MCP-compatible agent connects instantly. As MCP becomes the standard protocol for AI tools, Sandcastle becomes the default sandbox.

### 2. Tiered Isolation
No one-size-fits-all. Simple math? Use process isolation (5ms). Unknown code? Use gVisor (50ms). Potential malware? Use Firecracker (250ms). The agent chooses based on context.

### 3. Self-Hostable
Not cloud-only. Run Sandcastle on your own infrastructure — your code never leaves your network. Open-source core with optional managed service.

### 4. Network-Zero by Default
Every competitor defaults to open network. Sandcastle defaults to **no network**. You explicitly allowlist domains. This is the right default for security.

## Use Cases

### 1. Coding Assistant Code Execution
Agent writes code → needs to test it → `execute_code` in sandbox → sees output → iterates.
- **Isolation**: medium (gVisor)
- **Network**: none or pypi.org/npmjs.com for package installs
- **Timeout**: 30s

### 2. Data Analysis
Agent receives CSV → writes Python to analyze it → `upload_file` + `execute_code` → `download_file` for charts/results.
- **Isolation**: medium
- **Network**: none
- **Timeout**: 120s
- **Memory**: 2GB (large datasets)

### 3. User-Submitted Code Evaluation
Education platform → students submit code → agent evaluates it in sandbox.
- **Isolation**: high (Firecracker) — students might try to escape
- **Network**: none
- **Timeout**: 10s

### 4. Web Scraping / API Calls
Agent needs to fetch data from specific APIs → allowlisted domains.
- **Isolation**: medium
- **Network**: allowed_domains = ["api.github.com", "api.openai.com"]
- **Timeout**: 60s

### 5. CI/CD Pipeline Step
Automated pipeline → agent generates and runs tests.
- **Isolation**: low (trusted code from own repo)
- **Network**: full (or restricted to internal services)
- **Timeout**: 300s

## Business Model

### Open Source (Core)
- Sandcastle binary + all three isolation backends
- MCP server (stdio + HTTP)
- CLI tool
- Apache-2.0 license

### Managed Service (Revenue)
- **Sandcastle Cloud** — hosted sandboxes, no infrastructure to manage
- Usage-based pricing: per execution-second
- Pricing tiers by isolation level:
  - Low: $0.001 / execution-second
  - Medium: $0.005 / execution-second
  - High: $0.02 / execution-second
- Free tier: 1000 execution-seconds / month

### Enterprise (Revenue)
- Priority support
- Custom isolation policies
- SSO/SAML
- Audit log exports
- SLA guarantees
- On-prem deployment assistance

## Success Metrics

### Adoption
- GitHub stars
- MCP tool registry installs
- Monthly active sandboxes (managed service)

### Quality
- P99 sandbox creation latency (target: <500ms for all levels)
- Sandbox escape incidents (target: 0)
- Uptime (target: 99.9% for managed service)

### Revenue
- Monthly recurring revenue (managed service)
- Enterprise contracts

## Competitive Landscape

| | Sandcastle | E2B | Modal | CodeSandbox | Docker |
|---|---|---|---|---|---|
| Primary use | AI agent sandbox | AI agent sandbox | ML compute | Browser IDE | General containers |
| Protocol | MCP | REST SDK | Python SDK | REST API | Docker API |
| Isolation | 3 tiers | Firecracker | gVisor | Container | Namespace |
| Self-host | ✅ | ❌ | ❌ | ❌ | ✅ |
| Network default | Blocked | Open | Open | Open | Open |
| Open source | ✅ Full | Partial | ❌ | Partial | ✅ |
| Pricing | Usage-based | Usage-based | Usage-based | Subscription | Free (self-host) |

## Roadmap

### Phase 1 — Foundation
- MCP server (stdio transport)
- Low isolation backend (namespaces + seccomp)
- Single language (Python)
- One-shot execute_code tool
- CLI for local testing

### Phase 2 — Security
- Medium isolation (gVisor)
- Network allowlisting
- Session-based sandboxes (create/execute/destroy)
- File upload/download
- Multi-language (Python, JavaScript, Bash)

### Phase 3 — Performance
- High isolation (Firecracker)
- Pre-warmed sandbox pools
- Snapshot-based restore
- HTTP transport for remote agents

### Phase 4 — Scale
- Managed service (Sandcastle Cloud)
- Usage-based billing
- Dashboard + analytics
- More languages (Rust, Go, TypeScript)

### Phase 5 — Enterprise
- SSO/SAML
- Audit logs
- Custom policies
- On-prem deployment
- SLA
