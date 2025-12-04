#!/bin/bash
# Demonstration of working breenix-gdb-chat interface
set -e

echo "=== Breenix GDB Chat Interface Demo ==="
echo ""

# Clean up any existing processes
pkill -9 -f "qemu" 2>/dev/null || true
pkill -9 -f "gdb" 2>/dev/null || true
sleep 2

echo "Test 1: Start QEMU with debug kernel and examine initial state"
echo "----------------------------------------------------------------"
printf "info registers rip rsp rbp\nx/i \$rip\nquit\n" | python3 breenix-gdb-chat/scripts/gdb_chat.py 2>&1 | jq -c 'select(.command) | {command, success, output: (.output.rip // .output)}'

sleep 2
pkill -9 -f "qemu" 2>/dev/null || true
pkill -9 -f "gdb" 2>/dev/null || true
sleep 2

echo ""
echo "Test 2: Set breakpoint, continue execution (auto-interrupt after 30s), examine state"
echo "-------------------------------------------------------------------------------------"
printf "break _start\ncontinue\ninfo registers rip rsp\nbt\nquit\n" | python3 breenix-gdb-chat/scripts/gdb_chat.py 2>&1 | jq -c 'select(.command) | {command, success, output: (if .output.rip then {rip: .output.rip, rsp: .output.rsp} else .output end)}'

sleep 2
pkill -9 -f "qemu" 2>/dev/null || true
pkill -9 -f "gdb" 2>/dev/null || true

echo ""
echo "=== Demo Complete ==="
echo ""
echo "Key features demonstrated:"
echo "  1. Automatic QEMU startup with GDB server"
echo "  2. Setting breakpoints"
echo "  3. Continuing execution with auto-interrupt after 30s"
echo "  4. Examining registers and backtrace"
echo "  5. JSON-formatted output for easy parsing"
