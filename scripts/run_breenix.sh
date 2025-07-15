#!/bin/bash

# Run Breenix with logging to timestamped log files

# Get the directory of this script
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
PROJECT_ROOT="$SCRIPT_DIR/.."

# Create logs directory if it doesn't exist
mkdir -p "$PROJECT_ROOT/logs"

# Create a timestamp for the log file
TIMESTAMP=$(date +"%Y%m%d_%H%M%S")
LOG_FILE="$PROJECT_ROOT/logs/breenix_${TIMESTAMP}.log"

# Default to UEFI mode
MODE="${1:-uefi}"

# Shift to remove mode from args if provided
if [ "$1" = "uefi" ] || [ "$1" = "bios" ]; then
    shift
fi

echo "Starting Breenix in $MODE mode..."
echo "Logging to: $LOG_FILE"
echo ""

# Build and run using xtask
cargo run -p xtask -- build-and-run --features testing --timeout 60 2>&1 | tee "$LOG_FILE"

echo ""
echo "Log saved to: $LOG_FILE"