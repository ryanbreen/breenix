//! Truthful results from process termination and supervision.

use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Exit(i32),
    Signal(i32, &'static str),
    ExecFailed(String),
    TimedOut,
    Cancelled,
}

impl Verdict {
    pub fn from_status(status: ExitStatus) -> Self {
        if let Some(code) = status.code() {
            if code == 0 { Self::Pass } else { Self::Exit(code) }
        } else if let Some(signal) = status.signal() {
            Self::Signal(signal, signal_name(signal))
        } else {
            // A nonterminal wait status is not a successful test.
            Self::ExecFailed("child did not terminate".to_owned())
        }
    }

    pub fn from_spawn_error(error: &std::io::Error) -> Self {
        Self::ExecFailed(error.to_string())
    }

    pub fn failed(&self) -> bool { !matches!(self, Self::Pass) }
}

fn signal_name(signal: i32) -> &'static str {
    match signal {
        1 => "SIGHUP", 2 => "SIGINT", 3 => "SIGQUIT", 4 => "SIGILL",
        5 => "SIGTRAP", 6 => "SIGABRT", 7 => "SIGBUS", 8 => "SIGFPE",
        9 => "SIGKILL", 10 => "SIGUSR1", 11 => "SIGSEGV", 12 => "SIGUSR2",
        13 => "SIGPIPE", 14 => "SIGALRM", 15 => "SIGTERM", 16 => "SIGSTKFLT",
        17 => "SIGCHLD", 18 => "SIGCONT", 19 => "SIGSTOP", 20 => "SIGTSTP",
        21 => "SIGTTIN", 22 => "SIGTTOU", 23 => "SIGURG", 24 => "SIGXCPU",
        25 => "SIGXFSZ", 26 => "SIGVTALRM", 27 => "SIGPROF", 28 => "SIGWINCH",
        29 => "SIGIO", 30 => "SIGPWR", 31 => "SIGSYS", _ => "unknown signal",
    }
}
