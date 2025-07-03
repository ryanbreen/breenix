#!/bin/bash
# Restart MCP server in tmux

echo "🔄 Restarting MCP server..."

# Send Ctrl+C to stop current MCP
tmux send-keys -t breenix-mcp:mcp.0 C-c

# Wait a moment
sleep 1

# Clear the pane
tmux send-keys -t breenix-mcp:mcp.0 C-l

# Rebuild and restart HTTP server
tmux send-keys -t breenix-mcp:mcp.0 "cargo build --bin breenix-http-server && BREENIX_MCP_PORT=8080 cargo run --bin breenix-http-server" Enter

echo "✅ MCP restart command sent"