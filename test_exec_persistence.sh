#!/bin/bash

echo "Testing exec with page table persistence fix..."

# Run the test with QEMU and capture output
echo "Running test with 25 second timeout..."
timeout 25s cargo run --bin xtask run-qemu kernel/target/x86_64-breenix/release/kernel > /tmp/exec_test_output.log 2>&1

# Check for key success markers
echo "Checking for exec success markers..."

echo "Looking for EXEC_OK marker:"
grep -a "EXEC_OK" /tmp/exec_test_output.log || echo "  ❌ EXEC_OK not found"

echo "Looking for exec_replace completion:"
grep -a "exec_replace: Returning 0" /tmp/exec_test_output.log || echo "  ❌ exec_replace completion not found"

echo "Looking for page table persistence:"
grep -a "Process.*: Replacing page table" /tmp/exec_test_output.log || echo "  ❌ Page table replacement not found"

echo "Looking for CR3 switches:"
grep -a "Scheduled page table switch" /tmp/exec_test_output.log || echo "  ❌ CR3 switches not found"

echo "Looking for page table restoration after timer:"
grep -a "User-space context restore" /tmp/exec_test_output.log || echo "  ❌ Context restore not found"

echo ""
echo "Last 20 lines of output:"
tail -20 /tmp/exec_test_output.log

echo ""
echo "Full log saved to /tmp/exec_test_output.log"