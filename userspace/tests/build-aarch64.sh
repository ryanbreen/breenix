#!/bin/bash
set -e

# Legacy ARM64 build script - now delegates to std build
#
# All userspace binaries have been ported to use Rust std via tests-std/.
# This script exists for backward compatibility and chains to the std build.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STD_BUILD="$SCRIPT_DIR/../tests-std/build.sh"

if [ -f "$STD_BUILD" ]; then
    echo "=== Delegating to std ARM64 build ==="
    bash "$STD_BUILD" --arch aarch64
else
    echo "ERROR: tests-std/build.sh not found at $STD_BUILD"
    exit 1
fi
