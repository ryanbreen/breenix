//! The SDK suite's Rust std app: one program, one subcommand per guest case.
//!
//! Each subcommand prints fixed lines on stdout and exits 0 when the behaviour it
//! exercises is right; the guest suite compares the exact output and exit status. A
//! subcommand that sees something wrong prints what it saw and exits 1.

mod fs_cmds;
mod net_cmds;
mod proc_cmds;
mod thread_cmds;

use std::process::ExitCode;

type CmdResult = Result<(), String>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let Some(command) = args.get(1) else {
        println!("usage: rust-app COMMAND [ARG...]");
        return ExitCode::from(2);
    };
    let rest = &args[2..];
    let result = match command.as_str() {
        "hello" => {
            println!("hello from rust-app");
            Ok(())
        }
        "args" => proc_cmds::args(rest),
        "env" => proc_cmds::env(),
        "exit" => proc_cmds::exit(rest),
        "cat" => proc_cmds::cat(),
        "process-status" => proc_cmds::status(&args[0]),
        "process-output" => proc_cmds::output(&args[0]),
        "cwd" => proc_cmds::cwd(),
        "fs-rw" => fs_cmds::read_write(),
        "fs-dir" => fs_cmds::dirs(),
        "fs-metadata" => fs_cmds::metadata(),
        "threads" => thread_cmds::threads(),
        "sync" => thread_cmds::sync(),
        "net-tcp" => net_cmds::tcp(),
        "net-udp" => net_cmds::udp(),
        "net-unix" => net_cmds::unix(),
        other => Err(format!("unknown command {other}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            println!("{why}");
            ExitCode::from(1)
        }
    }
}
