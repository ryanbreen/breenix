#!/bin/bash

# Run Breenix test commands with logging

# Get the directory of this script
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
PROJECT_ROOT="$SCRIPT_DIR/.."

# Create logs directory if it doesn't exist
mkdir -p "$PROJECT_ROOT/logs"

# Create a timestamp for the log file
TIMESTAMP=$(date +"%Y%m%d_%H%M%S")
LOG_FILE="$PROJECT_ROOT/logs/breenix_test_${TIMESTAMP}.log"

echo "Starting Breenix test run..."
echo "Logging to: $LOG_FILE"
echo ""

# Start Breenix in background
echo "Starting Breenix..."
cargo run --release --bin qemu-uefi -- -serial stdio -display none 2>&1 | tee "$LOG_FILE" &
QEMU_PID=$!

# Wait for kernel to be ready
echo "Waiting for kernel to initialize..."
sleep 5

# Send test command if provided
if [ -n "$1" ]; then
    echo "Sending test command: $1"
    # Use expect or similar to send command to serial console
    # For now, we'll just document this needs to be implemented
    echo "Note: Interactive command sending not yet implemented"
    echo "You can manually type commands in the QEMU console"
fi

# Wait for QEMU to finish
wait $QEMU_PID

echo ""
echo "Log saved to: $LOG_FILE"