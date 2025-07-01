#!/bin/bash

echo "Starting interactive kernel test..."
echo "After kernel boots:"
echo "  - Press Ctrl+P to test multiple processes with timer preemption"
echo "  - Press Ctrl+U to test single userspace process"
echo "  - Press Ctrl+T for time debug info"
echo "  - Press Ctrl+M for memory debug info"
echo ""

# Run QEMU with serial output
cargo run --features testing --bin qemu-uefi -- -serial stdio