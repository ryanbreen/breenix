# Serial Command Interface

## Overview
Breenix now supports sending commands through the serial port, which enables automated testing and control without keyboard input.

## Implementation Details

### Command Registry (`kernel/src/serial/command.rs`)
- Commands are registered during kernel initialization
- Available commands:
  - `help` - Show available commands
  - `hello` - Test command
  - `ps` - List processes
  - `mem` - Show memory statistics
  - `test` or `t` - Run test processes
  - `forktest` or `f` - Test fork system call
  - `echo <msg>` - Echo a message

### MCP Integration
The MCP server (`mcp/src/main.rs`) sends commands through the serial port:
- Uses `breenix_send` tool to write commands to stdin
- QEMU is launched with `-serial stdio` to connect serial to stdin/stdout
- Commands are sent as plain text followed by newline

### Test Scripts
- `scripts/test_fork_mcp.sh` - Uses MCP to send "forktest" command
- The script waits for kernel logs and analyzes fork test results

## Usage Examples

### Manual Testing
```bash
# Send commands via echo
echo "forktest" | cargo run --bin qemu-uefi -- -serial stdio

# Interactive session
cargo run --bin qemu-uefi -- -serial stdio
> help
> forktest
```

### Automated Testing with MCP
```bash
# Start MCP server (in one terminal)
cd mcp && cargo run

# Run fork test (in another terminal)
./scripts/test_fork_mcp.sh 5
```

## Fork Test Flow
1. MCP sends "forktest" command to serial port
2. Serial command handler calls `test_fork_debug()`
3. Without testing feature: Calls `sys_fork()` directly from kernel
4. With testing feature: Loads fork_test.elf userspace program
5. Logs show thread ID tracking and fork system call invocation