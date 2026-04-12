# Sandcastle Security Model

## Threat Model

### What we protect against
1. **Data exfiltration** — Code trying to send data to external servers
2. **Host compromise** — Code trying to escape the sandbox and access host resources
3. **Resource abuse** — Code consuming unlimited CPU, memory, or disk
4. **Lateral movement** — Code trying to access other sandboxes
5. **Persistence** — Code trying to survive sandbox destruction
6. **Kernel exploits** — Code trying to exploit Linux kernel vulnerabilities

### What we do NOT protect against
1. **Cryptographic side-channel attacks** (Spectre/Meltdown) — mitigated by host kernel patches, not sandbox
2. **Physical access** — assumed attacker has no physical access to host
3. **Supply chain attacks on our own binary** — out of scope

## Isolation Level Details

### Low — Linux Namespaces + Seccomp + Cgroups

**Mechanism**: The sandbox is a child process created with `clone()` using multiple namespace flags. A seccomp-BPF filter restricts available syscalls. Cgroups v2 enforce resource limits.

**What's isolated:**
| Resource | Mechanism | Details |
|----------|-----------|---------|
| Processes | PID namespace | Sandbox sees only its own processes, PID 1 = executor |
| Filesystem | Mount namespace + pivot_root | Sandbox has its own root, cannot see host FS |
| Network | Network namespace | Empty netns = no network interfaces |
| Users | User namespace | UID 0 in sandbox maps to unprivileged UID on host |
| IPC | IPC namespace | No shared memory/semaphores with host |
| Hostname | UTS namespace | Isolated hostname |
| Syscalls | seccomp-BPF | ~100 allowed syscalls, all others return EPERM |
| CPU | cgroup cpu.max | Quota-based CPU limiting |
| Memory | cgroup memory.max | Hard limit, OOM kill on exceed |
| PIDs | cgroup pids.max | Prevents fork bombs |

**Attack surface:**
- All ~100 allowed syscalls are attack surface
- Kernel vulnerabilities in those syscalls could allow escape
- This is the same isolation level as Docker (without additional hardening)

**When to use:**
- Trusted code from known sources
- Simple computations, text processing, data formatting
- When speed is critical (~5ms startup)

### Medium — gVisor

**Mechanism**: gVisor's Sentry process intercepts all guest syscalls via ptrace. The Sentry reimplements Linux syscall semantics in a userspace Go application. Only ~70 host syscalls are made by the Sentry itself.

**What's additionally isolated (beyond namespaces):**
| Resource | Mechanism | Details |
|----------|-----------|---------|
| Syscalls | Sentry intercept | Guest never makes real syscalls — Sentry re-implements them |
| Network | Netstack | gVisor's own TCP/IP stack, not the host kernel's |
| Filesystem | Gofer process | Filesystem access proxied through a separate process |

**Attack surface:**
- The Sentry's ~70 host syscalls (much smaller than raw namespace's ~100)
- gVisor itself could have bugs, but it's heavily tested (used in Google Cloud Run, Cloud Functions)
- Kernel is never exposed to guest code — eliminates entire class of kernel exploits

**When to use:**
- General untrusted code execution
- Code from unknown sources that doesn't need raw performance
- Default for most use cases

### High — Firecracker MicroVM

**Mechanism**: Each sandbox runs in its own Firecracker microVM backed by KVM. The guest runs its own Linux kernel in hardware-isolated VMX non-root mode. The host kernel's KVM module manages CPU and memory isolation at the hardware level.

**What's additionally isolated (beyond gVisor):**
| Resource | Mechanism | Details |
|----------|-----------|---------|
| CPU | KVM VMX non-root mode | Hardware-enforced isolation, guest can't execute privileged instructions |
| Memory | KVM EPT (Extended Page Tables) | Hardware-enforced memory mapping, guest can't access host RAM |
| Devices | Firecracker emulation | Only 5 virtio devices, minimal attack surface |
| Kernel | Separate guest kernel | Guest has its own kernel — host kernel never exposed |

**Attack surface:**
- KVM module in host kernel (very well audited)
- Firecracker's 5 virtio device emulators (~50K lines Rust)
- VM escape requires exploiting KVM or Firecracker — extremely difficult

**When to use:**
- Fully untrusted code (user-submitted, internet-sourced)
- Code that might attempt kernel exploits
- Regulatory requirements for strong isolation
- When you want maximum security and can accept ~250ms startup

## Network Security

### Default: No Network

All isolation levels start with **no network access**. The sandbox cannot make any outbound connections or receive any inbound connections.

- **Low**: Empty network namespace (no interfaces)
- **Medium**: No network interface in gVisor config
- **High**: No TAP device created for the VM

### Allowlisted Domains

When `allowed_domains` is specified, Sandcastle creates a restricted network path:

```
Sandbox → Virtual Interface → Host iptables/nftables → Internet
                                     │
                                     ├── ALLOW: resolved IPs of allowed domains, ports 80/443
                                     └── DROP: everything else
```

**DNS resolution:**
- Sandcastle runs a DNS proxy on the host
- Sandbox's `/etc/resolv.conf` points to the host DNS proxy
- DNS proxy only resolves domains in the allowlist — all others return NXDOMAIN
- This prevents DNS-based exfiltration (encoding data in DNS queries to arbitrary domains)

**IP pinning:**
- Allowed domains are resolved to IPs at sandbox creation time
- Re-resolved periodically (every 60s) to handle DNS changes
- Only resolved IPs are allowed through the firewall
- Prevents domain-fronting attacks (where an allowed domain's IP serves a different domain)

### Communication Channel (Host ↔ Sandbox)

No TCP/IP is used for host-sandbox communication:
- **Low**: Unix pipe (stdin/stdout of the child process)
- **Medium**: Unix socket via gVisor's filesystem
- **High**: vsock (virtio socket — direct host↔VM channel, no network stack)

This means even if the sandbox somehow creates a network interface, it can't intercept the control channel.

## Filesystem Security

### Ephemeral by Default
- Sandbox filesystem is a temporary copy/overlay of the base image
- Destroyed completely when sandbox is destroyed
- No persistent state survives sandbox destruction

### No Host Access
- No host directories are mounted into the sandbox
- The sandbox root filesystem is a self-contained image
- Even `/proc` and `/sys` are either empty, restricted, or gVisor-managed

### Working Directory
- All user code executes in `/workspace`
- File uploads are copied from host `allowed_input_dirs` into `/workspace`
- File downloads are copied from `/workspace` to host `output_dir`
- Host paths are validated: uploads only from allowlisted dirs, downloads only to designated output dir
- Cannot write outside `/workspace` (enforced by executor, not just permissions)
- File content never passes through MCP messages — only host paths are exchanged

## Resource Limits Enforcement

| Resource | Low | Medium | High |
|----------|-----|--------|------|
| Memory | cgroup memory.max | cgroup memory.max | VM mem_size_mib |
| CPU | cgroup cpu.max | cgroup cpu.max | VM vcpu_count |
| Timeout | Host-side kill | Host-side `runsc kill` | Host-side Firecracker kill |
| Disk | cgroup io.max + tmpfs size | Same + gVisor fs limits | VM rootfs size |
| PIDs | cgroup pids.max | cgroup pids.max | VM-internal limits |

### Timeout Enforcement
- Host-side timer runs independently of the sandbox
- When timeout fires, the sandbox is killed immediately (SIGKILL / process termination)
- No guest cooperation required — the host is always in control
- Partial output collected before kill is still returned

### OOM Handling
- If sandbox exceeds memory limit, the kernel OOM-kills the process
- Sandcastle detects this and returns a specific error: `"oom_killed": true`
- The agent can retry with a higher memory limit

## Audit & Logging

### What we log
- Sandbox creation: isolation level, language, resource limits, allowed domains
- Execution start/end: duration, exit code, timeout status
- Network activity: outbound connection attempts (allowed and blocked)
- Resource usage: peak memory, CPU time consumed
- Sandbox destruction: reason (completed, timeout, manual, idle)

### What we don't log
- The code itself (privacy — the agent's code is not our business)
- stdout/stderr content (returned to agent only)
- File contents (uploaded/downloaded)

### Metrics
- Sandbox creation rate (per isolation level)
- Execution duration distribution
- Timeout rate
- OOM rate
- Pool utilization
- Network block rate
