# Serial Input Implementation

## Overview

Serial input has been successfully implemented for Breenix, providing a remote command interface that will be invaluable for testing process control features in Phase 8.

## Implementation Details

### 1. Hardware Configuration
- Enabled UART receive interrupts on COM1 (0x3F8)
- Set bit 0 of Interrupt Enable Register to enable "data available" interrupts
- Configured PIC to unmask IRQ4 (serial port interrupt)

### 2. Interrupt Handling
- Added `Serial` variant to `InterruptIndex` enum (IRQ4)
- Implemented `serial_interrupt_handler` that reads all available bytes
- Handler checks Line Status Register and reads while data available
- Properly sends EOI to PIC after handling

### 3. Async Stream
- Created `SerialInputStream` implementing the `Stream` trait
- Uses `ArrayQueue<u8>` for lockless byte queue (256 bytes)
- Uses `AtomicWaker` for async task notification
- Follows same pattern as keyboard stream

### 4. Command Processing
- Line buffering with 256-character limit
- Backspace support (0x08 and 0x7F)
- Ctrl+C to cancel current line
- Echo characters back for visual feedback
- Command parsing with space-separated arguments

### 5. Available Commands
- `help` - Show available commands
- `hello` - Test command
- `echo <message>` - Echo a message
- `ps` - List processes (placeholder)
- `mem` - Memory statistics (placeholder)
- `test` or `t` - Run tests (placeholder)

## Usage

### Interactive Testing
```bash
./scripts/test_serial_input.sh
# Or directly:
cargo run --bin qemu-uefi -- -serial stdio -display none
```

### Sending Commands
Once the kernel boots, press Enter to see the prompt:
```
> help
Available commands:
  help        - Show this help message
  hello       - Test command
  ps          - List processes
  mem         - Show memory statistics
  test        - Run test processes
  echo <msg>  - Echo a message
> echo Serial input works!
Serial input works!
> _
```

### Automation
The serial interface can be automated using tools like:
- `socat` - For scripting: `echo "help" | socat - UNIX-CONNECT:/tmp/qemu-serial`
- `screen` - For interactive sessions: `screen /dev/ttyUSB0 115200`
- Python scripts with pyserial

## Future Enhancements

1. **Add Real Commands** - Connect ps, mem, test to actual implementations
2. **Command History** - Add up/down arrow support
3. **Tab Completion** - For command names
4. **Raw Mode** - For binary protocols
5. **Flow Control** - XON/XOFF or hardware flow control

## Benefits for Phase 8

This serial input implementation provides:
- Remote testing of fork/exec/wait without keyboard
- Scriptable test scenarios
- Better debugging when processes hang
- Foundation for automated CI/CD testing