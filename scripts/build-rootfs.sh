#!/usr/bin/env bash
# Build per-language rootfs directories for Sandcastle ProcessSandbox.
#
# Uses Docker to export minimal rootfs trees:
#   rootfs/python/   — Python 3.12 slim
#   rootfs/bash/     — Alpine with bash
#   rootfs/javascript/ — Node 20 slim
#
# Usage: sudo ./scripts/build-rootfs.sh [--rootfs-dir /path/to/rootfs] [--executor /path/to/executor]

set -euo pipefail

ROOTFS_DIR="${1:-/var/lib/sandcastle/rootfs}"
EXECUTOR_BIN="${2:-$(dirname "$0")/../target/debug/sandcastle-executor}"

echo "=== Sandcastle rootfs builder ==="
echo "Rootfs dir: $ROOTFS_DIR"
echo "Executor binary: $EXECUTOR_BIN"

if [ ! -f "$EXECUTOR_BIN" ]; then
    echo "ERROR: Executor binary not found at $EXECUTOR_BIN"
    echo "Build it first: cargo build -p sandcastle-executor"
    exit 1
fi

build_rootfs() {
    local lang="$1"
    local image="$2"
    local dest="$ROOTFS_DIR/$lang"

    echo ""
    echo "--- Building rootfs for $lang from $image ---"

    if [ -d "$dest" ]; then
        echo "  Rootfs already exists at $dest, skipping (delete to rebuild)"
        return
    fi

    mkdir -p "$dest"

    # Create a temporary container and export its filesystem
    local container_id
    container_id=$(docker create "$image" /bin/true 2>/dev/null)
    echo "  Created container: $container_id"

    docker export "$container_id" | tar -xf - -C "$dest" 2>/dev/null
    docker rm "$container_id" >/dev/null 2>&1
    echo "  Exported filesystem to $dest"

    # Create /sandbox dir and copy executor
    mkdir -p "$dest/sandbox"
    cp "$EXECUTOR_BIN" "$dest/sandbox/executor"
    chmod 755 "$dest/sandbox/executor"
    echo "  Installed executor binary"

    # Create /workspace dir
    mkdir -p "$dest/workspace"

    # Ensure /dev, /proc, /tmp exist (they may not in exported images)
    mkdir -p "$dest/dev" "$dest/proc" "$dest/tmp"

    echo "  Done: $(du -sh "$dest" | cut -f1) total"
}

# Build for each language
build_rootfs "python" "python:3.12-slim"
build_rootfs "bash" "bash:5"
build_rootfs "javascript" "node:20-slim"

echo ""
echo "=== All rootfs images built ==="
echo "Contents:"
ls -la "$ROOTFS_DIR/"
