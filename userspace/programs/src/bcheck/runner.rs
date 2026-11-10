//! Nonblocking supervision of one self-check process group.

use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use super::verdict::Verdict;

unsafe extern "C" { fn kill(pid: i32, signal: i32) -> i32; }
const SIGKILL: i32 = 9;

pub struct Runner {
    child: Child,
    deadline: Instant,
    stopped: Option<Verdict>,
}

impl Runner {
    pub fn start(argv: &[String], timeout: Duration) -> Result<Self, Verdict> {
        let (program, args) = argv.split_first().ok_or_else(|| Verdict::ExecFailed("empty command".into()))?;
        let child = Command::new(program).args(args)
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .process_group(0).spawn().map_err(|error| {
                if error.raw_os_error() == Some(2) { Verdict::ExecFailed("ENOENT".into()) }
                else { Verdict::from_spawn_error(&error) }
            })?;
        Ok(Self { child, deadline: Instant::now() + timeout, stopped: None })
    }

    pub fn pid(&self) -> u32 { self.child.id() }

    pub fn poll(&mut self) -> Option<Verdict> {
        match self.child.try_wait() {
            Ok(Some(status)) => return Some(self.stopped.take().unwrap_or_else(|| Verdict::from_status(status))),
            Err(error) => return Some(Verdict::ExecFailed(error.to_string())),
            Ok(None) => {}
        }
        if self.stopped.is_none() && Instant::now() >= self.deadline {
            self.stop(Verdict::TimedOut);
        }
        None
    }

    pub fn cancel(&mut self) {
        if self.stopped.is_none() { self.stop(Verdict::Cancelled); }
    }

    fn stop(&mut self, verdict: Verdict) {
        unsafe { kill(-(self.child.id() as i32), SIGKILL); }
        self.stopped = Some(verdict);
    }
}
