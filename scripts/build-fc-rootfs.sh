#!/usr/bin/env bash
# Build ext4 rootfs images for Sandcastle Firecracker backend.
#
# Converts existing rootfs directories (from build-rootfs.sh) into ext4 block device images.
# The vsock-enabled executor is injected into each image.
#
# Prerequisites:
#   - sudo ./scripts/build-rootfs.sh (rootfs directories must exist)
#   - cd service && cargo build -p sandcastle-executor --target x86_64-unknown-linux-musl --features vsock-mode
#
# Usage: sudo ./scripts/build-fc-rootfs.sh

set -euo pipefail

ROOTFS_DIR="${1:-/var/lib/sandcastle/rootfs}"
EXECUTOR_BIN="${2:-$(dirname "$0")/../service/target/x86_64-unknown-linux-musl/debug/sandcastle-executor}"

echo "=== Sandcastle Firecracker rootfs builder ==="
echo "Rootfs dir: $ROOTFS_DIR"
echo "Executor binary: $EXECUTOR_BIN"

if [ ! -f "$EXECUTOR_BIN" ]; then
    echo "ERROR: Executor binary not found at $EXECUTOR_BIN"
    echo "Build it first: cd service && cargo build -p sandcastle-executor --target x86_64-unknown-linux-musl --features vsock-mode"
    exit 1
fi

build_ext4() {
    local lang="$1"
    local src_dir="$ROOTFS_DIR/$lang"
    local dest_img="$ROOTFS_DIR/${lang}.ext4"

    echo ""
    echo "--- Building ext4 image for $lang ---"

    if [ ! -d "$src_dir" ]; then
        echo "  ERROR: Source rootfs directory $src_dir does not exist"
        echo "  Run: sudo ./scripts/build-rootfs.sh first"
        return 1
    fi

    if [ -f "$dest_img" ]; then
        echo "  Image already exists at $dest_img, skipping (delete to rebuild)"
        return
    fi

    # Calculate size: rootfs dir size + 128MB headroom
    local dir_size_mb
    dir_size_mb=$(du -sm "$src_dir" | cut -f1)
    local img_size_mb=$((dir_size_mb + 128))
    echo "  Source dir: ${dir_size_mb}MB, image size: ${img_size_mb}MB"

    # Create empty image file
    dd if=/dev/zero of="$dest_img" bs=1M count="$img_size_mb" status=none
    echo "  Created ${img_size_mb}MB image file"

    # Format as ext4
    mkfs.ext4 -F -q "$dest_img"
    echo "  Formatted as ext4"

    # Mount and copy rootfs contents
    local mount_point
    mount_point=$(mktemp -d)
    mount -o loop "$dest_img" "$mount_point"

    # Copy rootfs directory contents
    cp -a "$src_dir"/. "$mount_point"/
    echo "  Copied rootfs contents"

    # Replace the stdio executor with the vsock-enabled executor
    mkdir -p "$mount_point/sandbox"
    cp "$EXECUTOR_BIN" "$mount_point/sandbox/executor"
    chmod 755 "$mount_point/sandbox/executor"
    echo "  Installed vsock executor binary"

    # Ensure workspace directory exists
    mkdir -p "$mount_point/workspace"
    chmod 777 "$mount_point/workspace"

    # Unmount
    umount "$mount_point"
    rmdir "$mount_point"

    echo "  Done: $(du -sh "$dest_img" | cut -f1) image"
}

# Build ext4 images for each language
build_ext4 "python"
build_ext4 "bash"
build_ext4 "javascript"

echo ""
echo "=== Firecracker ext4 rootfs images built ==="
echo "Images:"
ls -lh "$ROOTFS_DIR"/*.ext4 2>/dev/null || echo "  No ext4 images found"
