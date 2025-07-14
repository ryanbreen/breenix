#!/bin/bash

# Test exec functionality

echo "Building kernel with testing features..."
cargo run --bin xtask -- build --features testing

echo "Running kernel and capturing output..."
LOGFILE="logs/exec_test_$(date +%Y%m%d_%H%M%S).log"

# Run with timeout and capture all output
timeout 30s cargo run --bin xtask -- build-and-run --features testing 2>&1 | tee "$LOGFILE"

echo ""
echo "=== Checking for exec test results ==="
echo ""

# Check for key markers
echo "Checking for EXEC_OK marker (indicates successful exec):"
grep -a "EXEC_OK" "$LOGFILE" || echo "  ❌ EXEC_OK not found"

echo ""
echo "Checking for exec_replace calls:"
grep -a "exec_replace" "$LOGFILE" | head -5 || echo "  ❌ No exec_replace calls found"

echo ""
echo "Checking for syscall 11 (exec):"
grep -a "RAX=0xb" "$LOGFILE" | head -5 || echo "  ❌ No syscall 11 found"

echo ""
echo "Log saved to: $LOGFILE"