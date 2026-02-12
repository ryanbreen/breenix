//! Signal exec check program (std version)
//!
//! This program is exec'd by signal_exec_test to verify that signal
//! handlers are reset to SIG_DFL after exec().
//!
//! POSIX requires that signals with custom handlers be reset to SIG_DFL
//! after exec, while ignored signals (SIG_IGN) may remain ignored.

use libbreenix::signal::{SIGUSR1, SIG_DFL, SIG_IGN};
use libbreenix::{sigaction, Sigaction};

fn main() {
    println!("=== Signal Exec Check (after exec) ===");
    println!("This program was exec'd - checking if signal handlers are reset to SIG_DFL\n");

    // Query the current handler for SIGUSR1
    // If exec reset handlers properly, this should return SIG_DFL (0)
    println!("Querying SIGUSR1 handler state...");

    let mut old_action = Sigaction::default();

    // sigaction with act=None queries current handler without changing it
    let ret = sigaction(SIGUSR1, None, Some(&mut old_action));

    if ret.is_ok() {
        println!("  sigaction query succeeded");
        println!("  Handler value: {}", old_action.handler);

        if old_action.handler == SIG_DFL {
            println!("  PASS: Handler is SIG_DFL (correctly reset after exec)");
            println!("\nSIGNAL_EXEC_RESET_VERIFIED");
            std::process::exit(0);
        } else if old_action.handler == SIG_IGN {
            println!("  INFO: Handler is SIG_IGN (may be acceptable per POSIX)");
            // This is technically acceptable for POSIX but we want SIG_DFL
            println!("\nSIGNAL_EXEC_RESET_PARTIAL");
            std::process::exit(1);
        } else {
            println!("  FAIL: Handler is NOT SIG_DFL - it was inherited from pre-exec!");
            println!("\nSIGNAL_EXEC_RESET_FAILED");
            std::process::exit(2);
        }
    } else {
        println!("  FAIL: sigaction query returned error");
        println!("\nSIGNAL_EXEC_RESET_FAILED");
        std::process::exit(3);
    }
}
