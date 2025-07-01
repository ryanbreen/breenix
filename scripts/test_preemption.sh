#!/bin/bash

echo "Testing preemptive multitasking with timer interrupts..."
echo "This will run QEMU with serial output and wait for process scheduling"
echo "Press 'p' to start multi-process test"
echo ""

# Run QEMU with serial output
cargo run --features testing --bin qemu-uefi -- -serial stdio