//! Shell commands using real POSIX infrastructure through WasmKernel.
//!
//! Each command is a function: fn(&mut WasmKernel, &[&str], &[u8]) -> (Vec<u8>, i32)
//! Arguments: (kernel, argv, stdin_data) -> (stdout_output, exit_code)

mod builtins;

use crate::kernel::WasmKernel;
use alloc::vec;
use alloc::vec::Vec;

/// Command function type
pub type CommandFn = fn(&mut WasmKernel, &[&str], &[u8]) -> (Vec<u8>, i32);

/// Look up a command by name
pub fn lookup(name: &str) -> Option<CommandFn> {
    match name {
        "ls" => Some(builtins::cmd_ls),
        "cat" => Some(builtins::cmd_cat),
        "echo" => Some(builtins::cmd_echo),
        "pwd" => Some(builtins::cmd_pwd),
        "cd" => Some(builtins::cmd_cd),
        "mkdir" => Some(builtins::cmd_mkdir),
        "rmdir" => Some(builtins::cmd_rmdir),
        "touch" => Some(builtins::cmd_touch),
        "rm" => Some(builtins::cmd_rm),
        "cp" => Some(builtins::cmd_cp),
        "mv" => Some(builtins::cmd_mv),
        "head" => Some(builtins::cmd_head),
        "tail" => Some(builtins::cmd_tail),
        "wc" => Some(builtins::cmd_wc),
        "uname" => Some(builtins::cmd_uname),
        "env" => Some(builtins::cmd_env),
        "export" => Some(builtins::cmd_export),
        "unset" => Some(builtins::cmd_unset),
        "help" => Some(builtins::cmd_help),
        "clear" => Some(builtins::cmd_clear),
        "stat" => Some(builtins::cmd_stat),
        "whoami" => Some(builtins::cmd_whoami),
        "hostname" => Some(builtins::cmd_hostname),
        "date" => Some(builtins::cmd_date),
        "true" => Some(builtins::cmd_true),
        "false" => Some(builtins::cmd_false),
        "tee" => Some(builtins::cmd_tee),
        "grep" => Some(builtins::cmd_grep),
        _ => None,
    }
}

/// Get list of available commands (for tab completion, help)
pub fn available_commands() -> Vec<&'static str> {
    let mut cmds = vec![
        "cat", "cd", "clear", "cp", "date", "echo", "env", "export",
        "false", "grep", "head", "help", "hostname", "ls", "mkdir",
        "mv", "pwd", "rm", "rmdir", "stat", "tail", "tee", "touch",
        "true", "uname", "unset", "wc", "whoami",
    ];
    cmds.sort();
    cmds
}
