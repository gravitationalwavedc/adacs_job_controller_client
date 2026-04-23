#!/bin/bash
#
# Build release binary with static linking against glibc 2.17 (RHEL 7 compatible)
#
# This script uses cargo-zigbuild with Zig as the linker to produce a binary
# that only depends on core glibc libraries (libc, libm, libpthread, libdl)
# and is compatible with glibc 2.17+ (RHEL 7, CentOS 7, and newer).
#
# Prerequisites:
#   - Zig (https://ziglang.org/download/)
#   - cargo-zigbuild (cargo install cargo-zigbuild)
#
# Usage:
#   ./build_release.sh
#
# Output:
#   target/x86_64-unknown-linux-gnu/release/adacs_job_client
#

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# Check for zig
if ! command -v zig &> /dev/null; then
    echo "Error: zig is not installed"
    echo ""
    echo "Install zig using one of these methods:"
    echo "  1. webinstall (recommended): curl -sSf https://webinstall.dev/zig | bash"
    echo "  2. Official download: https://ziglang.org/download/"
    echo "  3. PyPI: pip3 install ziglang"
    echo ""
    echo "After installation, make sure zig is in your PATH"
    exit 1
fi

# Check for cargo-zigbuild
if ! command -v cargo-zigbuild &> /dev/null; then
    echo "Installing cargo-zigbuild..."
    cargo install cargo-zigbuild
fi

echo "Building release binary with glibc 2.17 compatibility..."
echo "Zig version: $(zig version)"
echo ""

cargo zigbuild --target x86_64-unknown-linux-gnu.2.17 --release

BINARY_PATH="target/x86_64-unknown-linux-gnu/release/adacs_job_client"

echo ""
echo "Build complete: $BINARY_PATH"
echo ""
echo "Verifying dependencies..."
if command -v ldd &> /dev/null; then
    ldd "$BINARY_PATH"
fi

echo ""
echo "Checking glibc version requirement..."
if command -v objdump &> /dev/null; then
    MAX_VERSION=$(objdump -T "$BINARY_PATH" 2>/dev/null | grep GLIBC_ | cut -d@ -f2 | sed 's/.*GLIBC_\([0-9.]*\).*/\1/' | sort -V | tail -1)
    echo "Maximum glibc version required: GLIBC_$MAX_VERSION"
fi

echo ""
echo "Binary is compatible with RHEL 7 / glibc 2.17+"
