#!/bin/bash
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/x86-qemu-policy.sh"
# Manual test - just run and let us type

cd "$(dirname "$0")/.."
cargo run --release --bin qemu-uefi -- -serial stdio