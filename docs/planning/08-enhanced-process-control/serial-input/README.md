# Serial Input Implementation (Phase 8)

## Why Serial Input in Phase 8?

Serial input is being added to Phase 8 specifically to help with testing process control features:
- **Test fork()**: Trigger process creation remotely
- **Test exec()**: Load different programs on demand  
- **Process monitoring**: Check process states without keyboard
- **Automated testing**: Script complex fork/exec/wait scenarios
- **Debugging**: Inspect process trees when keyboard is busy

## Motivation

Serial input would be a game-changer for Breenix development and testing:
- **Remote Control**: Send commands without needing keyboard in QEMU
- **Automated Testing**: Script complex test scenarios
- **Debugging**: Interact with kernel even when graphics/keyboard fail
- **CI/CD**: Enable automated testing in headless environments

## Design Overview

### Current State
- Serial output works great (uart_16550)
- Only keyboard input currently supported
- Serial port configured for output only

### Implementation Plan

1. **UART Receive Interrupt**
   ```rust
   // Enable receive interrupts in UART setup
   serial_port.enable_receive_interrupt();
   
   // Handle interrupt when data available
   fn serial_interrupt_handler() {
       while serial_port.data_available() {
           let byte = serial_port.read_byte();
           SERIAL_INPUT_QUEUE.push(byte);
           SERIAL_WAKER.wake();
       }
   }
   ```

2. **Input Buffer & Stream**
   ```rust
   // Similar to keyboard stream
   pub struct SerialInputStream {
       _private: (),
   }
   
   impl Stream for SerialInputStream {
       type Item = u8;
       // Poll from SERIAL_INPUT_QUEUE
   }
   ```

3. **Line Discipline**
   - Buffer until newline
   - Handle backspace
   - Echo characters back
   - Support for raw mode

4. **Command Processor**
   ```rust
   // Process commands from serial
   async fn serial_command_task() {
       let mut input = SerialInputStream::new();
       let mut line_buffer = String::new();
       
       while let Some(byte) = input.next().await {
           match byte {
               b'\n' => {
                   process_command(&line_buffer);
                   line_buffer.clear();
               }
               b'\x08' => { // Backspace
                   line_buffer.pop();
               }
               _ => {
                   line_buffer.push(byte as char);
               }
           }
       }
   }
   ```

## Testing Strategy

1. **Echo Test**: Type should echo back
2. **Command Test**: Send "help\n" should show commands
3. **Control Chars**: Test Ctrl+C, Ctrl+D behavior
4. **Automation**: Script that sends commands and verifies output

## Integration Points

- Add to async executor alongside keyboard task
- Share command processing with keyboard handler
- Future: multiplex with shell when available

## Benefits Once Implemented

```bash
# Automated testing example
echo "test_processes" | socat - UNIX-CONNECT:/tmp/qemu-serial
expect "All processes completed successfully"

# Remote debugging
screen /dev/ttyUSB0 115200
> help
Available commands:
  ps - list processes
  mem - show memory stats
  test - run test suite
```

## Priority

**HIGH** - This would significantly improve development velocity and enable better testing