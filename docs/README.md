# Sandcastle Documentation

## Project Status

- **Phase 1** ✅ Complete — Core MCP server + ProcessSandbox (Linux namespaces)
- **Phase 2** ✅ Complete — MCP integration with Copilot CLI, file transfer, e2e validation
- **Phase 3** ⬜ Planned — gVisor backend (medium isolation)
- **Phase 4** ⬜ Planned — Firecracker backend (high isolation)

## Documentation Structure

### For AI Agents (Context Bank)
Start with `/.context_bank/OVERVIEW.md` — structured for AI agent onboarding:
- `OVERVIEW.md` — Repo layout, status, quick commands
- `ARCHITECTURE.md` — System design, data flows, isolation model
- `CRATE_REFERENCE.md` — Per-crate API reference
- `CONVENTIONS.md` — Coding rules, build commands, workflow
- `KNOWN_ISSUES.md` — Gotchas and lessons learned

### Product Documentation
- [Product Vision](product/vision.md) — Problem, solution, roadmap, competitive landscape
- [Use Cases](product/use-cases.md) — Target scenarios and user stories
- [MCP Tools Spec](product/mcp-tools-spec.md) — Tool reference for agent developers
- [Vulnerability Map](product/vulnerability-map.md) — Security threat model
- [Phase 1 Goals](product/phase_1/goals.md) — Phase 1 success criteria (✅ complete)

### Technical Documentation
- [Architecture](technical/architecture.md) — System design overview
- [Security](technical/security.md) — Isolation model and threat mitigation
- [Phase 1 Spec](technical/phase_1/phase1-spec.md) — Technical specification (✅ complete)

## Quick Start

```bash
# Build
cd service && cargo build

# Build static executor (required for containers)
cd service && cargo build -p sandcastle-executor --target x86_64-unknown-linux-musl

# Build rootfs images
sudo ./scripts/build-rootfs.sh

# Run tests
cd service && cargo test
cd service && sudo $(which cargo) test -p sandcastle-process --test e2e -- --nocapture

# Start MCP server
sudo ./service/target/debug/sandcastle serve --transport stdio

# Configure for Copilot CLI
# Add to ~/.copilot/mcp-config.json, then /restart in Copilot
```
