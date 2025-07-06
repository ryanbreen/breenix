#!/bin/bash
# Simple script to test exectest command

# Change to project root
cd "$(dirname "$0")/.."

# Run with stdio serial and send commands
(
    sleep 7  # Wait for kernel to boot
    echo "hello"
    sleep 1
    echo "exectest"
    sleep 5
) | cargo run --release --bin qemu-uefi -- -serial stdio -display none