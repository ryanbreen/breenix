#!/bin/bash
# CI test script - runs Breenix exactly as in local testing

echo "Starting Breenix CI test..."
echo "Current directory: $(pwd)"
echo "QEMU version:"
qemu-system-x86_64 --version || echo "QEMU not found"

# Run the test
timeout 30 ./scripts/run_breenix.sh uefi -display none > ci_test.log 2>&1
EXIT_CODE=$?

echo "Test completed with exit code: $EXIT_CODE"

# Check for success
if grep -q "USERSPACE OUTPUT: Hello from userspace" ci_test.log; then
    echo "✅ SUCCESS: Found userspace execution!"
    grep "USERSPACE OUTPUT" ci_test.log | head -5
    exit 0
else
    echo "❌ FAILED: No userspace execution found"
    echo "Last 30 lines of output:"
    tail -30 ci_test.log
    exit 1
fi