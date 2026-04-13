#!/usr/bin/env bash
# Sandcastle — One-time setup script
#
# Installs all dependencies, builds the project, creates rootfs images,
# and configures MCP so you can connect immediately after running this.
#
# Usage: sudo ./scripts/setup.sh
#
# What this script does:
#   1. Installs system packages (Docker, musl tools, e2fsprogs)
#   2. Installs Rust toolchain + musl target (if not present)
#   3. Installs runsc (gVisor) for medium isolation
#   4. Installs Firecracker + kernel for high isolation
#   5. Builds the Rust workspace (server + executor)
#   6. Creates runtime directory structure
#   7. Builds container rootfs images (Docker export)
#   8. Builds ext4 rootfs images (Firecracker)
#   9. Configures MCP for Copilot CLI
#
# Prerequisites:
#   - Ubuntu/Debian-based Linux (x86_64)
#   - KVM support for Firecracker (optional — skipped if unavailable)
#   - Internet access for downloading packages
#
# After running: start a Copilot CLI session and use /mcp to verify.

set -euo pipefail

# --- Configuration ---
SANDCASTLE_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SANDCASTLE_DATA="/var/lib/sandcastle"
SANDCASTLE_RUN="/run/sandcastle"
FIRECRACKER_VERSION="1.12.0"
RUNSC_VERSION="release-20260406.0"
KERNEL_URL="https://s3.amazonaws.com/spec.ccfc.min/ci-artifacts/kernels/x86_64/vmlinux-6.1.102"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info()  { echo -e "${BLUE}[INFO]${NC}  $*"; }
log_ok()    { echo -e "${GREEN}[OK]${NC}    $*"; }
log_warn()  { echo -e "${YELLOW}[WARN]${NC}  $*"; }
log_error() { echo -e "${RED}[ERROR]${NC} $*"; }
log_step()  { echo -e "\n${GREEN}=== $* ===${NC}"; }

# --- Pre-flight checks ---
if [ "$(id -u)" -ne 0 ]; then
    log_error "This script must be run as root (sudo ./scripts/setup.sh)"
    exit 1
fi

REAL_USER="${SUDO_USER:-$(whoami)}"
REAL_HOME=$(getent passwd "$REAL_USER" | cut -d: -f6)

log_step "Sandcastle Setup"
log_info "Project root: $SANDCASTLE_ROOT"
log_info "Data directory: $SANDCASTLE_DATA"
log_info "User: $REAL_USER (home: $REAL_HOME)"

# Detect architecture
ARCH=$(uname -m)
if [ "$ARCH" != "x86_64" ]; then
    log_error "Only x86_64 is supported. Detected: $ARCH"
    exit 1
fi

# Check that the real user has passwordless sudo (needed for MCP stdio)
if ! su - "$REAL_USER" -c 'sudo -n true' 2>/dev/null; then
    log_warn "User '$REAL_USER' does not have passwordless sudo."
    log_warn "MCP stdio transport requires passwordless sudo to start the server."
    log_warn "Add to /etc/sudoers: $REAL_USER ALL=(ALL) NOPASSWD: ALL"
fi

# --- Step 1: System packages ---
log_step "Step 1/9: Installing system packages"

apt-get update -qq

PACKAGES=(
    curl
    git
    ca-certificates
    python3         # Used for MCP config merging
    e2fsprogs       # mkfs.ext4 for Firecracker rootfs
    build-essential # C compiler for Rust crate compilation
    pkg-config
    libssl-dev
    musl-tools      # musl-gcc for static linking
)

MISSING=()
for pkg in "${PACKAGES[@]}"; do
    if ! dpkg -l "$pkg" 2>/dev/null | grep -q '^ii'; then
        MISSING+=("$pkg")
    fi
done

if [ ${#MISSING[@]} -gt 0 ]; then
    log_info "Installing: ${MISSING[*]}"
    apt-get install -y -qq "${MISSING[@]}"
    log_ok "System packages installed"
else
    log_ok "All system packages already installed"
fi

# Docker
if ! command -v docker &>/dev/null; then
    log_info "Installing Docker..."
    curl -fsSL https://get.docker.com | sh
    usermod -aG docker "$REAL_USER"
    log_ok "Docker installed"
else
    log_ok "Docker already installed ($(docker --version | head -c 30))"
fi

# --- Step 2: Rust toolchain ---
log_step "Step 2/9: Rust toolchain"

if ! su - "$REAL_USER" -c 'command -v cargo' &>/dev/null; then
    log_info "Installing Rust toolchain..."
    su - "$REAL_USER" -c 'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y'
    log_ok "Rust installed"
else
    log_ok "Rust already installed ($(su - "$REAL_USER" -c 'rustc --version' 2>/dev/null))"
fi

# Ensure musl target is installed
if ! su - "$REAL_USER" -c 'rustup target list --installed 2>/dev/null' | grep -q 'x86_64-unknown-linux-musl'; then
    log_info "Adding musl target..."
    su - "$REAL_USER" -c 'rustup target add x86_64-unknown-linux-musl'
    log_ok "musl target added"
else
    log_ok "musl target already installed"
fi

# --- Step 3: runsc (gVisor) ---
log_step "Step 3/9: runsc (gVisor) for medium isolation"

if command -v runsc &>/dev/null; then
    log_ok "runsc already installed ($(runsc --version 2>&1 | head -1))"
else
    log_info "Installing runsc ${RUNSC_VERSION}..."
    curl -fsSL "https://storage.googleapis.com/gvisor/releases/${RUNSC_VERSION}/x86_64/runsc" -o /usr/local/bin/runsc
    chmod 755 /usr/local/bin/runsc
    log_ok "runsc installed"
fi

# --- Step 4: Firecracker + kernel ---
log_step "Step 4/9: Firecracker for high isolation"

# Check KVM availability
HAS_KVM=false
if [ -e /dev/kvm ] && [ -r /dev/kvm ] && [ -w /dev/kvm ]; then
    HAS_KVM=true
    log_ok "KVM available (/dev/kvm exists and is accessible)"
else
    log_warn "KVM not available — Firecracker (high isolation) will be skipped"
    log_warn "To enable: use a VM with nested virtualization support"
fi

if [ "$HAS_KVM" = true ]; then
    # Firecracker binary
    if command -v firecracker &>/dev/null; then
        log_ok "Firecracker already installed ($(firecracker --version 2>&1 | head -1))"
    else
        log_info "Installing Firecracker v${FIRECRACKER_VERSION}..."
        fc_url="https://github.com/firecracker-microvm/firecracker/releases/download/v${FIRECRACKER_VERSION}/firecracker-v${FIRECRACKER_VERSION}-x86_64.tgz"
        fc_tmp=$(mktemp -d)
        curl -fsSL "$fc_url" -o "$fc_tmp/fc.tgz"
        tar -xzf "$fc_tmp/fc.tgz" -C "$fc_tmp"
        cp "$fc_tmp/release-v${FIRECRACKER_VERSION}-x86_64/firecracker-v${FIRECRACKER_VERSION}-x86_64" /usr/local/bin/firecracker
        cp "$fc_tmp/release-v${FIRECRACKER_VERSION}-x86_64/jailer-v${FIRECRACKER_VERSION}-x86_64" /usr/local/bin/jailer
        chmod 755 /usr/local/bin/firecracker /usr/local/bin/jailer
        rm -rf "$fc_tmp"
        log_ok "Firecracker installed"
    fi

    # Kernel
    mkdir -p "$SANDCASTLE_DATA/kernel"
    if [ -f "$SANDCASTLE_DATA/kernel/vmlinux" ]; then
        log_ok "Kernel already at $SANDCASTLE_DATA/kernel/vmlinux"
    else
        log_info "Downloading Linux kernel for Firecracker..."
        # Try common locations first
        if [ -f /opt/firecracker/vmlinux-6.1 ]; then
            cp /opt/firecracker/vmlinux-6.1 "$SANDCASTLE_DATA/kernel/vmlinux"
            log_ok "Kernel copied from /opt/firecracker/vmlinux-6.1"
        else
            curl -fsSL "$KERNEL_URL" -o "$SANDCASTLE_DATA/kernel/vmlinux"
            log_ok "Kernel downloaded"
        fi
    fi
fi

# --- Step 5: Build Rust workspace ---
log_step "Step 5/9: Building Rust workspace"

CARGO_BIN=$(su - "$REAL_USER" -c 'which cargo')

log_info "Building server and all crates..."
su - "$REAL_USER" -c "cd '$SANDCASTLE_ROOT/service' && cargo build 2>&1" | tail -5
log_ok "Workspace built"

log_info "Building executor (stdio mode, static musl)..."
su - "$REAL_USER" -c "cd '$SANDCASTLE_ROOT/service' && cargo build -p sandcastle-executor --target x86_64-unknown-linux-musl 2>&1" | tail -3
log_ok "Executor (stdio) built"

if [ "$HAS_KVM" = true ]; then
    log_info "Building executor (vsock mode, static musl)..."
    su - "$REAL_USER" -c "cd '$SANDCASTLE_ROOT/service' && cargo build -p sandcastle-executor --target x86_64-unknown-linux-musl --features vsock-mode 2>&1" | tail -3
    log_ok "Executor (vsock) built"
fi

# --- Step 6: Create directory structure ---
log_step "Step 6/9: Creating runtime directories"

mkdir -p "$SANDCASTLE_DATA"/{rootfs,bundles,workspaces,bin}
mkdir -p "$SANDCASTLE_DATA"/gvisor/{bundles,workspaces}
mkdir -p "$SANDCASTLE_RUN"
mkdir -p "$SANDCASTLE_RUN/gvisor"

if [ "$HAS_KVM" = true ]; then
    mkdir -p "$SANDCASTLE_DATA"/firecracker/workspaces
fi

# Copy executor to canonical location
cp "$SANDCASTLE_ROOT/service/target/x86_64-unknown-linux-musl/debug/sandcastle-executor" \
   "$SANDCASTLE_DATA/bin/executor"
chmod 755 "$SANDCASTLE_DATA/bin/executor"

log_ok "Directory structure created"

# --- Step 7: Build container rootfs images ---
log_step "Step 7/9: Building container rootfs images"

# Ensure Docker daemon is running
if ! docker info &>/dev/null; then
    log_info "Starting Docker daemon..."
    systemctl start docker
    sleep 2
    if ! docker info &>/dev/null; then
        log_error "Docker daemon failed to start. Cannot build rootfs images."
        exit 1
    fi
fi

"$SANDCASTLE_ROOT/scripts/build-rootfs.sh" "$SANDCASTLE_DATA/rootfs" \
    "$SANDCASTLE_ROOT/service/target/x86_64-unknown-linux-musl/debug/sandcastle-executor"

log_ok "Container rootfs images ready"

# --- Step 8: Build Firecracker ext4 rootfs images ---
log_step "Step 8/9: Building Firecracker ext4 rootfs images"

if [ "$HAS_KVM" = true ]; then
    # For ext4 images, we need the vsock-enabled executor
    VSOCK_EXECUTOR="$SANDCASTLE_ROOT/service/target/x86_64-unknown-linux-musl/debug/sandcastle-executor"

    # Rebuild with vsock features for ext4 images
    su - "$REAL_USER" -c "cd '$SANDCASTLE_ROOT/service' && cargo build -p sandcastle-executor --target x86_64-unknown-linux-musl --features vsock-mode 2>&1" | tail -3

    "$SANDCASTLE_ROOT/scripts/build-fc-rootfs.sh" "$SANDCASTLE_DATA/rootfs" "$VSOCK_EXECUTOR"

    # Rebuild without vsock features so the stdio executor is back for containers
    su - "$REAL_USER" -c "cd '$SANDCASTLE_ROOT/service' && cargo build -p sandcastle-executor --target x86_64-unknown-linux-musl 2>&1" | tail -3
    # Re-copy stdio executor to rootfs dirs
    for lang in python bash javascript; do
        if [ -d "$SANDCASTLE_DATA/rootfs/$lang/sandbox" ]; then
            cp "$SANDCASTLE_ROOT/service/target/x86_64-unknown-linux-musl/debug/sandcastle-executor" \
               "$SANDCASTLE_DATA/rootfs/$lang/sandbox/executor"
        fi
    done

    log_ok "Firecracker ext4 rootfs images ready"
else
    log_warn "Skipping Firecracker rootfs (no KVM)"
fi

# --- Step 9: Configure MCP ---
log_step "Step 9/9: Configuring MCP for Copilot CLI"

SERVER_BIN="$SANDCASTLE_ROOT/service/target/debug/sandcastle"
MCP_CONFIG_DIR="$REAL_HOME/.copilot"
MCP_CONFIG="$MCP_CONFIG_DIR/mcp-config.json"

mkdir -p "$MCP_CONFIG_DIR"

if [ -f "$MCP_CONFIG" ]; then
    # Check if sandcastle is already configured by parsing JSON properly
    if python3 -c "
import json, sys
with open('$MCP_CONFIG') as f:
    config = json.load(f)
if 'sandcastle' in config.get('mcpServers', {}):
    sys.exit(0)
sys.exit(1)
" 2>/dev/null; then
        log_ok "MCP already configured in $MCP_CONFIG"
    else
        log_info "Adding sandcastle to existing MCP config..."
        cp "$MCP_CONFIG" "${MCP_CONFIG}.bak"
        python3 -c "
import json, sys, tempfile, os
with open('$MCP_CONFIG') as f:
    config = json.load(f)
config.setdefault('mcpServers', {})['sandcastle'] = {
    'type': 'stdio',
    'command': 'sudo',
    'args': ['$SERVER_BIN', 'serve', '--transport', 'stdio']
}
tmp_fd, tmp_path = tempfile.mkstemp(dir='$MCP_CONFIG_DIR')
with os.fdopen(tmp_fd, 'w') as f:
    json.dump(config, f, indent=2)
os.rename(tmp_path, '$MCP_CONFIG')
"
        log_ok "MCP config updated (backup at ${MCP_CONFIG}.bak)"
    fi
else
    cat > "$MCP_CONFIG" << MCPJSON
{
  "mcpServers": {
    "sandcastle": {
      "type": "stdio",
      "command": "sudo",
      "args": [
        "$SERVER_BIN",
        "serve",
        "--transport",
        "stdio"
      ]
    }
  }
}
MCPJSON
    log_ok "MCP config created at $MCP_CONFIG"
fi

chown -R "$REAL_USER:$REAL_USER" "$MCP_CONFIG_DIR"

# --- Summary ---
log_step "Setup Complete!"

echo ""
echo "  Sandcastle is ready to use."
echo ""
echo "  Installed backends:"
echo "    ✅ Low isolation    — Linux namespaces (libcontainer)"
echo "    ✅ Medium isolation — gVisor (runsc)"
if [ "$HAS_KVM" = true ]; then
echo "    ✅ High isolation   — Firecracker microVM (KVM)"
else
echo "    ⚠️  High isolation   — Skipped (no KVM)"
fi
echo ""
echo "  Server binary: $SERVER_BIN"
echo "  MCP config:    $MCP_CONFIG"
echo ""
echo "  Next steps:"
echo "    1. Start a Copilot CLI session"
echo "    2. Run /mcp to verify sandcastle is connected"
echo "    3. Try: execute_code with isolation='low', 'medium', or 'high'"
echo ""
echo "  Useful commands:"
echo "    sudo ./tests/integration.sh    # Run integration tests"
echo "    cd service && cargo test       # Run unit tests"
echo ""
