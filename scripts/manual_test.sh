#!/bin/bash
# Manual test - just run and let us type

cd "$(dirname "$0")/.."
cargo run --release --bin qemu-uefi -- -serial stdio