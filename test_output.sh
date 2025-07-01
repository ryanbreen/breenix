#!/bin/bash

echo "Running kernel test and capturing output..."
timeout 10 cargo run --features testing --bin qemu-uefi -- -serial stdio -display none > test_output.log 2>&1

echo "Test completed. Checking for timer preemption messages..."
if grep -q "Timer preemption:" test_output.log; then
    echo "✓ Found timer preemption messages!"
    grep "Timer preemption:" test_output.log | head -5
else
    echo "✗ No timer preemption messages found"
fi

echo ""
echo "Checking for process creation..."
if grep -q "Created process" test_output.log; then
    echo "✓ Found process creation messages!"
    grep "Created process" test_output.log
else
    echo "✗ No process creation messages found"
fi

echo ""
echo "Last 20 lines of output:"
tail -20 test_output.log