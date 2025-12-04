#!/bin/bash
set -e

# Kill any existing QEMU/GDB processes
pkill -9 -f "qemu" 2>/dev/null || true
pkill -9 -f "gdb" 2>/dev/null || true
sleep 2

# Test 1: Basic connection and simple commands
echo "=== Test 1: Connection and basic commands ==="
printf "info registers rip\nx/i \$rip\nquit\n" | python3 breenix-gdb-chat/scripts/gdb_chat.py | jq -c '.command, .output.rip, .output' | head -20

sleep 2
pkill -9 -f "qemu" 2>/dev/null || true
pkill -9 -f "gdb" 2>/dev/null || true
sleep 2

# Test 2: Set breakpoint and try to continue (with 30 second timeout to see if it hits)
echo ""
echo "=== Test 2: Set breakpoint at _start and continue (30s timeout) ==="
timeout 35 bash -c 'printf "hbreak *0x50200\ncontinue\nquit\n" | python3 breenix-gdb-chat/scripts/gdb_chat.py' | jq -c '.command, .success, .error // "no error"' || echo "Test timed out after 35 seconds"

pkill -9 -f "qemu" 2>/dev/null || true
pkill -9 -f "gdb" 2>/dev/null || true

echo ""
echo "=== All tests complete ==="
