#!/bin/bash

# Run exectest with automatic logging

# Get the directory of this script
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"

echo "Running exectest..."
"$SCRIPT_DIR/breenix_runner.py" --commands "exectest"