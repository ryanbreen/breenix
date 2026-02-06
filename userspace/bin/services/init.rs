//! Breenix init process (/sbin/init)
//!
//! PID 1 - spawns system services and the interactive shell, then reaps zombies.
//!
//! Spawns:
//!   - /sbin/telnetd (background service)
//!   - /bin/init_shell (foreground shell on serial console)
//!
//! Main loop reaps terminated children with waitpid(WNOHANG) and respawns
//! crashed services.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;

const TELNETD_PATH: &[u8] = b"/sbin/telnetd\0";
const SHELL_PATH: &[u8] = b"/bin/init_shell\0";

/// Fork and exec a binary. Returns the child PID on success, -1 on failure.
fn spawn(path: &[u8], name: &str) -> i64 {
    let pid = process::fork();
    if pid == 0 {
        // Child: exec the binary
        let argv: [*const u8; 2] = [path.as_ptr(), core::ptr::null()];
        let _ = process::execv(path, argv.as_ptr());
        // exec failed
        io::print("[init] ERROR: exec failed for ");
        io::print(name);
        io::print("\n");
        process::exit(127);
    }
    if pid < 0 {
        io::print("[init] ERROR: fork failed for ");
        io::print(name);
        io::print("\n");
        return -1;
    }
    pid
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let pid = process::getpid();
    io::print("[init] Breenix init starting (PID ");
    print_i32(pid as i32);
    io::print(")\n");

    // Start telnetd
    io::print("[init] Starting /sbin/telnetd...\n");
    let mut telnetd_pid = spawn(TELNETD_PATH, "telnetd");

    // Start interactive shell
    io::print("[init] Starting /bin/init_shell...\n");
    let mut shell_pid = spawn(SHELL_PATH, "init_shell");

    // Main loop: reap zombies and respawn crashed services
    let mut status: i32 = 0;
    loop {
        let reaped = process::waitpid(-1, &mut status as *mut i32, process::WNOHANG);
        if reaped > 0 {
            if reaped == shell_pid {
                // Shell exited — respawn it
                io::print("[init] Shell exited, respawning...\n");
                shell_pid = spawn(SHELL_PATH, "init_shell");
            } else if reaped == telnetd_pid {
                // Telnetd crashed — respawn it
                io::print("[init] telnetd exited, respawning...\n");
                telnetd_pid = spawn(TELNETD_PATH, "telnetd");
            }
        }
        process::yield_now();
    }
}

/// Print an i32 as decimal to serial output.
fn print_i32(mut n: i32) {
    if n < 0 {
        io::print("-");
        // Handle i32::MIN edge case
        if n == i32::MIN {
            io::print("2147483648");
            return;
        }
        n = -n;
    }
    if n == 0 {
        io::print("0");
        return;
    }
    let mut buf = [0u8; 10];
    let mut i = 0usize;
    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    // Reverse and print
    while i > 0 {
        i -= 1;
        let ch = [buf[i]];
        // SAFETY: buf[i] is always a valid ASCII digit
        let s = unsafe { core::str::from_utf8_unchecked(&ch) };
        io::print(s);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("[init] PANIC\n");
    process::exit(101);
}
