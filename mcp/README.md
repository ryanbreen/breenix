# Breenix MCP Server

This MCP (Model Context Protocol) server provides intelligent interaction with Breenix QEMU sessions. Written in Rust for consistency with the Breenix codebase.

## Usage

Start the MCP server directly:
```bash
cd mcp && cargo run --bin breenix-mcp
```

Or use the tmuxinator config which automatically starts it:
```bash
tmuxinator start breenix
```

## Features

- **Session Management**: Start/stop Breenix in QEMU
- **Process Control**: Check running QEMU processes and kill them
- **Serial Communication**: Send commands and receive output
- **Log Monitoring**: View recent kernel logs with timestamps
- **Smart Waiting**: Wait for prompts or specific patterns in output
- **Command Execution**: Run commands and wait for completion

## Available Tools

### `breenix_start`
Start Breenix in QEMU
- `display` (optional): Show QEMU display window (default: false)

### `breenix_stop`
Stop the current Breenix session

### `breenix_running`
Check if Breenix is running and count QEMU processes

### `breenix_kill`
Kill all QEMU processes on the system

### `breenix_send`
Send a raw command via serial
- `command`: Command to send

### `breenix_logs`
Get recent kernel logs
- `lines` (optional): Number of lines to return (default: 50)

### `breenix_wait_prompt`
Wait for the serial prompt to be ready
- `timeout` (optional): Timeout in seconds (default: 5.0)

### `breenix_run_command`
Run a command and wait for completion
- `command`: Command to run
- `wait_pattern` (optional): Regex pattern indicating completion
- `timeout` (optional): Timeout in seconds (default: 5.0)

## Usage Examples

```python
# Check if any QEMU processes are running
breenix_running()

# Kill any existing QEMU processes
breenix_kill()

# Start Breenix
breenix_start()

# Wait for boot and prompt
breenix_wait_prompt(timeout=10)

# Run memory command and wait for completion
breenix_run_command(command="mem", wait_pattern="=== Memory Debug Information ===", timeout=5)

# Get recent logs
breenix_logs(lines=100)

# Stop session
breenix_stop()
```

## Smart Command Execution

The `breenix_run_command` tool is intelligent:
- It sends the command
- If a `wait_pattern` is provided, it waits for that pattern to appear
- Returns all logs since the command was sent
- Times out gracefully if pattern doesn't appear

This eliminates the need for arbitrary sleep delays between commands.