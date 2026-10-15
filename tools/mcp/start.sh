#!/bin/bash

# Start the Breenix MCP server
# This script changes to the working directory and runs the MCP

# Get the directory where this script is located
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Change to the repository root (tools/mcp is two levels down)
cd "$SCRIPT_DIR/../.." || exit 1

# Run the MCP server
cargo run --quiet --manifest-path "$SCRIPT_DIR/Cargo.toml" --bin breenix-mcp