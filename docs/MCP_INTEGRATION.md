# Breenix MCP Integration

## Overview

Breenix includes a Model Context Protocol (MCP) server that provides programmatic access to kernel testing and interaction. The MCP server runs over HTTP and provides tools for starting/stopping Breenix, sending commands, and monitoring logs.

## Architecture

```
┌─────────────────┐    HTTP    ┌─────────────────┐    Process    ┌─────────────────┐
│  Claude Code    │ ◄────────► │  MCP HTTP       │ ◄───────────► │  QEMU/Breenix   │
│  (Client)       │            │  Server         │               │  Kernel         │
└─────────────────┘            └─────────────────┘               └─────────────────┘
                                        │                                 │
                                        ▼                                 ▼
                               ┌─────────────────┐               ┌─────────────────┐
                               │  Log Storage    │               │  Serial I/O     │
                               │  /tmp/breenix-  │               │  Commands       │
                               │  mcp/kernel.log │               │                 │
                               └─────────────────┘               └─────────────────┘
```

## Components

### 1. MCP HTTP Server
- **Binary**: `breenix-http-server`
- **Port**: 8080 (configurable via `BREENIX_MCP_PORT`)
- **Protocol**: HTTP with JSON-RPC for MCP methods
- **Location**: `mcp/src/http_server.rs`

### 2. Session Management
- Manages QEMU process lifecycle
- Captures stdout/stderr from kernel
- Provides command injection via serial interface
- Tracks process state and logs

### 3. Log Integration
- Real-time log capture from Breenix kernel
- Writes to `/tmp/breenix-mcp/kernel.log`
- Integrated with tmuxinator for live viewing
- Supports both stdout and stderr capture

## Available Tools

The MCP server provides the following tools for interacting with Breenix:

### Core Process Management

#### `mcp__breenix__start`
Starts Breenix in QEMU.
- **Parameters**: 
  - `display` (boolean, optional): Show QEMU display window (default: false)
- **Returns**: Success/failure message

#### `mcp__breenix__stop`
Stops the running Breenix session.
- **Parameters**: None
- **Returns**: Success/failure message

#### `mcp__breenix__running`
Checks if Breenix is currently running.
- **Parameters**: None
- **Returns**: Status information (running state, QEMU process count, MCP management status)

#### `mcp__breenix__kill`
Forcefully kills all QEMU processes.
- **Parameters**: None
- **Returns**: Number of processes killed

### Communication

#### `mcp__breenix__send`
Sends a command to Breenix via serial interface.
- **Parameters**:
  - `command` (string, required): Command to send
- **Returns**: Confirmation message

#### `mcp__breenix__wait_prompt`
Waits for Breenix to be ready for input.
- **Parameters**:
  - `timeout` (number, optional): Timeout in seconds (default: 5.0)
- **Returns**: Success when prompt is ready, or timeout error

#### `mcp__breenix__run_command`
Runs a command and waits for completion.
- **Parameters**:
  - `command` (string, required): Command to run
  - `wait_pattern` (string, optional): Regex pattern indicating completion
  - `timeout` (number, optional): Timeout in seconds (default: 5.0)
- **Returns**: Command output

### Logging

#### `mcp__breenix__logs`
Retrieves recent Breenix logs.
- **Parameters**:
  - `lines` (integer, optional): Number of recent lines to return (default: 50)
- **Returns**: Log entries

## HTTP API Endpoints

In addition to MCP protocol, the server provides REST endpoints:

### Health Check
- **GET** `/health` - Server health status

### Process Management
- **POST** `/start` - Start Breenix (JSON body: `{"display": false}`)
- **POST** `/stop` - Stop Breenix
- **GET** `/status` - Get status information
- **POST** `/kill-all` - Kill all QEMU processes

### Communication
- **POST** `/send` - Send command (JSON body: `{"command": "test"}`)
- **GET** `/logs?lines=50` - Get logs
- **POST** `/wait-prompt` - Wait for prompt (JSON body: `{"timeout": 5.0}`)
- **POST** `/run-command` - Run command and wait

### MCP Protocol
- **POST** `/` - MCP JSON-RPC endpoint

## Development Setup

### Starting the MCP Server

#### Option 1: tmuxinator (Recommended)
```bash
# Start the complete development environment
tmuxinator start breenix-mcp

# This creates:
# - Top pane: MCP HTTP server on port 8080
# - Bottom pane: Live kernel logs display
```

#### Option 2: Manual
```bash
cd mcp
BREENIX_MCP_PORT=8080 cargo run --bin breenix-http-server
```

### Tmuxinator Configuration

The project includes a `.tmuxinator.yml` configuration that provides:

```yaml
windows:
  - mcp:
      layout: even-vertical
      panes:
        - mcp_server: cd mcp && BREENIX_MCP_PORT=8080 cargo run --bin breenix-http-server
        - kernel_logs: # Live tail of kernel logs
```

### Development Workflow

1. **Start MCP Environment**:
   ```bash
   tmuxinator start breenix-mcp
   ```

2. **Test HTTP API**:
   ```bash
   # Health check
   curl http://localhost:8080/health
   
   # Start Breenix
   curl -X POST http://localhost:8080/start -H "Content-Type: application/json" -d '{"display": false}'
   
   # Send a command
   curl -X POST http://localhost:8080/send -H "Content-Type: application/json" -d '{"command": "help"}'
   
   # Get logs
   curl http://localhost:8080/logs?lines=20
   ```

3. **Use with Claude Code**:
   Claude Code can automatically discover and use the MCP server tools for kernel testing and development.

### Restarting the Server

```bash
# Using the provided script
./scripts/restart_mcp.sh

# Or restart the entire tmuxinator session
tmuxinator restart breenix-mcp
```

## Log Management

### Log Storage
- **File**: `/tmp/breenix-mcp/kernel.log`
- **Format**: Plain text with timestamps
- **Rotation**: File is truncated on each Breenix start

### Log Viewing
- **tmuxinator**: Bottom pane shows live logs via `tail -F`
- **HTTP API**: `/logs` endpoint for programmatic access
- **MCP Tool**: `mcp__breenix__logs` for client integration

### Log Content
- Kernel boot messages
- Serial command output
- System call traces
- Error messages (prefixed with `[STDERR]`)

## Testing and Validation

### Unit Testing
The MCP tools enable comprehensive kernel testing:
```bash
# Example test sequence via MCP
1. Start Breenix: mcp__breenix__start
2. Wait for boot: mcp__breenix__wait_prompt
3. Run tests: mcp__breenix__run_command("test_fork")
4. Collect results: mcp__breenix__logs
5. Stop kernel: mcp__breenix__stop
```

### Integration with Test Suite
The MCP server integrates with Breenix's existing test infrastructure, allowing tests to be run programmatically through Claude Code or other MCP clients.

## Security Considerations

- **Local Only**: Server binds to `127.0.0.1` (localhost only)
- **No Authentication**: Intended for local development use
- **Process Isolation**: QEMU processes run with user permissions
- **Log Access**: Logs stored in `/tmp` with user access only

## Configuration

### Environment Variables
- `BREENIX_MCP_PORT`: HTTP server port (default: 8080)

### Build Configuration
The MCP server is built as a separate binary:
```toml
[[bin]]
name = "breenix-http-server"
path = "src/bin/http_server.rs"
```

## Troubleshooting

### Common Issues

1. **Port Already in Use**:
   ```bash
   # Check what's using port 8080
   lsof -i :8080
   # Use different port
   BREENIX_MCP_PORT=8081 cargo run --bin breenix-http-server
   ```

2. **QEMU Won't Start**:
   - Check that QEMU is installed: `which qemu-system-x86_64`
   - Verify kernel builds: `cargo build`
   - Check logs in MCP server output

3. **No Logs Appearing**:
   - Verify log file exists: `ls -la /tmp/breenix-mcp/kernel.log`
   - Check MCP server debug output for pipe errors
   - Restart tmuxinator session

### Debug Mode
Enable verbose logging in the MCP server by checking the terminal output for debug messages about pipe operations and process management.