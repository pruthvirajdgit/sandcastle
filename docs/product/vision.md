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

### 1. MCP-Native — Sandboxing by Architecture, Not Discipline
With SDK-based tools (E2B, Modal), the agent is a Python/Node process running on a host machine. It *can* run arbitrary code on the host — the sandbox is opt-in. A compromised agent, a prompt injection, or a developer mistake can bypass the sandbox entirely and execute code directly on the host.

With MCP, the agent communicates via structured messages only. It has no runtime, no interpreter, no filesystem access on the host. Its **only** way to execute code is through the `execute_code` tool — which always runs in a sandbox. 100% of code execution is sandboxed by design, not by developer discipline.

No SDK to install, no language-specific client. Any MCP-compatible agent connects instantly. As MCP becomes the standard protocol for AI tools, Sandcastle becomes the default sandbox.

### 2. Tiered Isolation
No one-size-fits-all. Simple math? Use process isolation (5ms). Unknown code? Use gVisor (50ms). Potential malware? Use Firecracker (250ms). The agent chooses based on context.

### 3. Self-Hostable — Your Data Never Leaves Your Network
Cloud-only sandboxes (E2B, Modal) require uploading your data to third-party infrastructure. For a data analysis use case, your CSV travels over the internet to their servers, gets processed, and results come back. This is a non-starter for privacy-sensitive data, regulated industries (healthcare, finance, government), or large files.

Sandcastle runs on your own infrastructure. File injection is a local pipe/vsock write, not an HTTP upload. Your data never leaves your network. Open-source core with optional managed service for those who prefer hosted.

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
| Protocol | MCP-first | REST SDK + MCP | Python SDK | REST API | Docker API |
| Sandbox by architecture | ✅ (MCP = only path) | ❌ (SDK = agent can bypass) | ❌ | ❌ | ❌ |
| Isolation | 3 tiers | Firecracker only | gVisor only | Container | Namespace |
| Self-host | ✅ | ❌ | ❌ | ❌ | ✅ |
| Network default | Blocked | Open | Open | Open | Open |
| Data stays local | ✅ | ❌ (cloud only) | ❌ (cloud only) | ❌ | ✅ |
| Open source | ✅ Full | Partial | ❌ | Partial | ✅ |
| Pricing | Usage-based | Usage-based | Usage-based | Subscription | Free (self-host) |

## Roadmap

### Phase 1 — Foundation ✅ Complete
- ✅ MCP server (stdio transport) via rmcp
- ✅ Low isolation backend (Linux namespaces + cgroups v2 via libcontainer/youki)
- ✅ Core tools: `execute_code`, `create_sandbox`, `execute_in_session`, `destroy_sandbox`
- ✅ File tools: `upload_file`, `download_file` (host path based)
- ✅ Multi-language support (Python 3.12, JavaScript/Node 20, Bash 5)
- ✅ Resource limits (CPU, memory, timeout, PIDs, disk)
- ✅ Network-zero by default (PID + mount namespace isolation)
- ✅ Configurable `allowed_input_dirs` and `output_dir`
- ✅ 15 unit tests + e2e integration test
- ✅ Static musl executor binary for container compatibility
- ✅ Docker-export rootfs build pipeline

### Phase 2 — MCP Integration & Validation ✅ Complete
- ✅ MCP server registered as Copilot CLI tool provider (~/.copilot/mcp-config.json)
- ✅ Live code execution verified via Copilot → MCP → container pipeline
- ✅ File transfer pipeline validated end-to-end (upload → execute → download)
- ✅ Persistent session lifecycle verified (create → multi-exec → destroy)
- ✅ Comprehensive context_bank documentation for AI agent onboarding

### Phase 3 — Tiered Isolation (gVisor + Firecracker)
- Medium isolation backend (gVisor / runsc)
- High isolation backend (Firecracker microVM via KVM)
- Pre-warmed sandbox pools per isolation level
- Snapshot-based restore for Firecracker (fast wake)
- Network allowlisting with DNS proxy
- HTTP+SSE transport for remote agents
- Pool management (target size, replenishment, idle timeout)

### Phase 4 — Security Hardening
- Malware scanning on file downloads (YARA rules + ClamAV)
- File quarantine on malware detection
- IP pinning for allowlisted domains (anti domain-fronting)
- Audit logging (creation, execution, network blocks, resource usage)
- Content type restrictions on downloads
- Seccomp profile tuning per language runtime
- Security benchmarks and penetration testing

### Phase 5 — Scale & Monetization
- Managed service (Sandcastle Cloud)
- Usage-based billing per execution-second
- Dashboard + analytics
- More languages (Rust, Go, TypeScript)
- API key management

### Phase 6 — Enterprise
- SSO/SAML
- Custom isolation policies
- On-prem deployment assistance
- SLA guarantees
- Compliance documentation (SOC2, ISO 27001)
