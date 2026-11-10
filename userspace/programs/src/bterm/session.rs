//! Terminal-owned process-group cleanup through the POSIX C interfaces.
use std::{io, thread, time::{Duration, Instant}};

unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn close(fd: i32) -> i32;
}

/// Hang up the setsid group, close its PTY, and collect the unreaped direct child.
/// A child that ignores HUP gets a bounded grace period before forced cleanup.
pub fn hangup_and_reap(group: i32, child: Option<i32>, master: i32) -> io::Result<Option<i32>> {
    if group <= 1 || child.is_some_and(|pid| pid <= 1) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid terminal session"));
    }
    let hup = if unsafe { kill(-group, 1) } == 0 { Ok(()) } else {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(3) {
            // Close can race the fork child's setsid. The child is still ours.
            match child {
                Some(pid) if unsafe { kill(pid, 1) } != 0 => Err(io::Error::last_os_error()),
                _ => Ok(()),
            }
        } else { Err(error) }
    };
    let closed = if unsafe { close(master) } == 0 { Ok(()) } else { Err(io::Error::last_os_error()) };
    let Some(pid) = child else { hup?; closed?; return Ok(None); };
    let mut deadline = Instant::now() + Duration::from_secs(10);
    let mut forced = false;
    let status = loop {
        let mut status = 0;
        let result = unsafe { waitpid(pid, &mut status, 1) };
        if result == pid { break status; }
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted { return Err(error); }
        }
        if Instant::now() >= deadline {
            if forced { return Err(io::Error::new(io::ErrorKind::TimedOut, "terminal child did not exit after SIGKILL")); }
            // Keep the cleanup limited to this tab, including the pre-setsid race.
            unsafe { kill(-group, 9); kill(pid, 9); }
            forced = true;
            deadline = Instant::now() + Duration::from_secs(10);
        }
        thread::sleep(Duration::from_millis(1));
    };
    hup?;
    closed?;
    Ok(Some(status))
}
