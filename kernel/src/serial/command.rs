extern crate alloc;

use super::SerialInputStream;
use crate::serial_println;
use crate::serial_writer::console;
use alloc::string::String;
use alloc::vec::Vec;
use futures_util::stream::StreamExt;
use spin::Once;
use core::sync::atomic::{AtomicBool, Ordering};

static TTY_INPUT: AtomicBool = AtomicBool::new(false);

/// Select the userspace console before starting the serial input task.
pub fn use_tty_input() {
    TTY_INPUT.store(true, Ordering::Release);
}

// Command handler type
type CommandHandler = fn();

// Registry for custom command handlers
static COMMAND_REGISTRY: Once<CommandRegistry> = Once::new();

#[allow(dead_code)] // Used by serial_command_task (conditionally compiled)
struct CommandRegistry {
    ps_handler: Option<CommandHandler>,
    mem_handler: Option<CommandHandler>,
    test_handler: Option<CommandHandler>,
    fork_test_handler: Option<CommandHandler>,
    exec_test_handler: Option<CommandHandler>,
}

impl CommandRegistry {}

/// Register command handlers from the kernel binary
pub fn register_handlers(
    ps: CommandHandler,
    mem: CommandHandler,
    test: CommandHandler,
    fork_test: CommandHandler,
    exec_test: CommandHandler,
) {
    COMMAND_REGISTRY.call_once(|| CommandRegistry {
        ps_handler: Some(ps),
        mem_handler: Some(mem),
        test_handler: Some(test),
        fork_test_handler: Some(fork_test),
        exec_test_handler: Some(exec_test),
    });
}

/// Handle serial input with line editing and command processing
#[allow(dead_code)] // Used in kernel_main_continue (conditionally compiled)
pub async fn serial_command_task() {
    log::info!("Serial command task started");

    let mut input = SerialInputStream::new();
    let mut line_buffer = String::new();
    let tty_input = TTY_INPUT.load(Ordering::Acquire);

    // Send a prompt
    if !tty_input {
        print_prompt();
    }

    let mut host_keys = crate::drivers::xhci::host_keys::Lines::new();
    while let Some(byte) = input.next().await {
        // A suite boot that declared serial host keys: serial lines are host-key requests.
        if crate::drivers::xhci::host_keys_armed() {
            if let Some(line) = host_keys.feed(byte) {
                crate::drivers::xhci::host_key_line(line);
            }
            continue;
        }
        if tty_input {
            crate::tty::push_char_from(byte, crate::tty::InputSource::Serial);
            continue;
        }
        match byte {
            // Enter/newline - process the command
            b'\r' | b'\n' => {
                // Echo newline
                console(b"\r\n");

                if !line_buffer.is_empty() {
                    process_command(&line_buffer);
                    line_buffer.clear();
                }

                print_prompt();
            }

            // Backspace (ASCII 0x08 or 0x7F)
            0x08 | 0x7F => {
                if !line_buffer.is_empty() {
                    line_buffer.pop();
                    // Send backspace sequence: backspace, space, backspace
                    // This erases the character on most terminals
                    console(&[0x08, b' ', 0x08]);
                }
            }

            // Ctrl+C (ASCII 0x03) - cancel current line
            0x03 => {
                line_buffer.clear();
                console(b"^C\r\n");
                print_prompt();
            }

            // Regular printable characters
            0x20..=0x7E => {
                // Echo the character
                console(&[byte]);
                line_buffer.push(byte as char);
            }

            // Ignore other control characters
            _ => {}
        }
    }
}

/// The prompt goes out as one unit, so no other writer's bytes land inside it,
/// never between the chunks of a TTY write, and on a fresh line if another
/// writer's line is open (`serial_writer`).
#[allow(dead_code)] // Used by serial_command_task (conditionally compiled)
fn print_prompt() {
    console(b"breenix> ");
}

#[allow(dead_code)] // Used by serial_command_task (conditionally compiled)
fn process_command(command: &str) {
    let trimmed = command.trim();

    // Split command and arguments
    let mut parts = trimmed.split_whitespace();
    let cmd = match parts.next() {
        Some(c) => c,
        None => return,
    };

    match cmd {
        "help" => {
            serial_println!("Available commands:");
            serial_println!("  help        - Show this help message");
            serial_println!("  hello       - Test command");
            serial_println!("  ps          - List processes");
            serial_println!("  mem         - Show memory statistics");
            serial_println!("  test        - Run test processes");
            serial_println!("  forktest    - Test fork system call");
            serial_println!("  exectest    - Test exec system call");
            serial_println!("  echo <msg>  - Echo a message");
        }

        "hello" => {
            serial_println!("Hello from Breenix serial console!");
        }

        "ps" => {
            if let Some(registry) = COMMAND_REGISTRY.get() {
                if let Some(handler) = registry.ps_handler {
                    handler();
                } else {
                    serial_println!("Process listing not available");
                }
            } else {
                serial_println!("Command handlers not registered");
            }
        }

        "mem" => {
            if let Some(registry) = COMMAND_REGISTRY.get() {
                if let Some(handler) = registry.mem_handler {
                    handler();
                } else {
                    serial_println!("Memory statistics not available");
                }
            } else {
                serial_println!("Command handlers not registered");
            }
        }

        "test" | "t" => {
            if let Some(registry) = COMMAND_REGISTRY.get() {
                if let Some(handler) = registry.test_handler {
                    handler();
                } else {
                    serial_println!("Test command not available");
                }
            } else {
                serial_println!("Command handlers not registered");
            }
        }

        "forktest" | "f" => {
            if let Some(registry) = COMMAND_REGISTRY.get() {
                if let Some(handler) = registry.fork_test_handler {
                    handler();
                } else {
                    serial_println!("Fork test command not available");
                }
            } else {
                serial_println!("Command handlers not registered");
            }
        }

        "exectest" | "e" => {
            if let Some(registry) = COMMAND_REGISTRY.get() {
                if let Some(handler) = registry.exec_test_handler {
                    handler();
                } else {
                    serial_println!("Exec test command not available");
                }
            } else {
                serial_println!("Command handlers not registered");
            }
        }

        "forkexec" | "fe" => {
            serial_println!(
                "Fork+exec command not implemented in serial - use Ctrl+X in keyboard mode"
            );
        }

        "echo" => {
            let rest: Vec<&str> = parts.collect();
            if rest.is_empty() {
                serial_println!("Usage: echo <message>");
            } else {
                let message = rest.join(" ");
                serial_println!("{}", message);
            }
        }

        _ => {
            serial_println!(
                "Unknown command: '{}'. Type 'help' for available commands.",
                cmd
            );
        }
    }
}
