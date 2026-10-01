//! Behavioral signal checks, run in the probe's supervised signal-check child.

use std::sync::atomic::{AtomicU32, Ordering};

use libbreenix::{
    errno::Errno,
    error::Error,
    process::{self, ForkResult},
    signal::{self, Sigaction},
    time,
};

static CAUGHT: AtomicU32 = AtomicU32::new(0);
static UNEXPECTED: AtomicU32 = AtomicU32::new(0);

extern "C" fn caught(_: i32) {
    CAUGHT.fetch_add(1, Ordering::SeqCst);
}
extern "C" fn unexpected(_: i32) {
    UNEXPECTED.fetch_add(1, Ordering::SeqCst);
}

fn pending() -> Result<u64, &'static str> {
    let mut set = 0;
    signal::sigpending(&mut set).map_err(|_| "sigpending failed")?;
    Ok(set)
}

fn reap(pid: i32, killed: bool) -> Result<(), &'static str> {
    let mut status = 0;
    loop {
        match process::waitpid(pid, &mut status, 0) {
            Ok(done) if done.raw() as i32 == pid => break,
            Err(Error::Os(Errno::EINTR)) => continue,
            _ => return Err("signal child wait failed"),
        }
    }
    if killed {
        if !process::wifsignaled(status) || process::wtermsig(status) != signal::SIGTERM {
            return Err("signal child did not die from SIGTERM");
        }
    } else if !process::wifexited(status) || process::wexitstatus(status) != 0 {
        return Err("signal sender failed");
    }
    Ok(())
}

fn wait_check(suspend: bool) -> Result<(), &'static str> {
    CAUGHT.store(0, Ordering::SeqCst);
    let parent = process::getpid().map_err(|_| "getpid failed")?.raw() as i32;
    // Ignored signals generated before the wait must leave no pending work.
    for sig in [signal::SIGCHLD, signal::SIGUSR2] {
        signal::kill(parent, sig).map_err(|_| "ignored self signal failed")?;
    }
    if pending()? != 0 {
        return Err("ignored signal became pending");
    }

    let victim = match process::fork().map_err(|_| "signal victim fork failed")? {
        ForkResult::Child => {
            let result = (|| {
                time::sleep_ms(20)?;
                signal::kill(parent, signal::SIGCHLD)?;
                signal::kill(parent, signal::SIGUSR2)?;
                if suspend {
                    signal::kill(parent, signal::SIGURG)?;
                }
                // Exercise deferred notification for a signal-terminated child.
                signal::kill(process::getpid()?.raw() as i32, signal::SIGTERM)?;
                Ok::<(), Error>(())
            })();
            process::exit(if result.is_ok() { 2 } else { 3 });
        }
        ForkResult::Parent(pid) => pid.raw() as i32,
    };
    let sender = match process::fork().map_err(|_| "signal sender fork failed")? {
        ForkResult::Child => {
            let result = time::sleep_ms(80).and_then(|_| signal::kill(parent, signal::SIGUSR1));
            process::exit(if result.is_ok() { 0 } else { 1 });
        }
        ForkResult::Parent(pid) => pid.raw() as i32,
    };
    let result = if suspend {
        signal::sigsuspend(&signal::sigmask(signal::SIGURG))
    } else {
        signal::pause()
    };
    if !matches!(result, Err(Error::Os(Errno::EINTR))) {
        return Err("signal wait did not return EINTR");
    }
    if CAUGHT.load(Ordering::SeqCst) != 1 || UNEXPECTED.load(Ordering::SeqCst) != 0 {
        return Err("signal wait returned without the caught signal");
    }
    if suspend {
        if pending()? & signal::sigmask(signal::SIGURG) == 0 {
            return Err("blocked signal was lost");
        }
        signal::sigaction(signal::SIGURG, Some(&Sigaction::ignore()), None)
            .map_err(|_| "discard blocked signal failed")?;
    }
    reap(victim, true)?;
    reap(sender, false)?;
    Ok(())
}

pub fn run() -> Result<(), &'static str> {
    signal::sigaction(signal::SIGUSR1, Some(&Sigaction::new(caught)), None)
        .map_err(|_| "install caught signal failed")?;
    signal::sigaction(signal::SIGUSR2, Some(&Sigaction::ignore()), None)
        .map_err(|_| "install ignored signal failed")?;
    signal::sigaction(signal::SIGCHLD, Some(&Sigaction::default_action()), None)
        .map_err(|_| "install default SIGCHLD failed")?;

    // A blocked caught signal is pending until installing ignore discards it.
    let bit = signal::sigmask(signal::SIGUSR1);
    signal::sigprocmask(signal::SIG_BLOCK, Some(&bit), None).map_err(|_| "block signal failed")?;
    let pid = process::getpid().map_err(|_| "getpid failed")?.raw() as i32;
    signal::kill(pid, signal::SIGUSR1).map_err(|_| "queue blocked signal failed")?;
    if pending()? & bit == 0 {
        return Err("caught blocked signal not pending");
    }
    signal::sigaction(signal::SIGUSR1, Some(&Sigaction::ignore()), None)
        .map_err(|_| "install ignore failed")?;
    if pending()? & bit != 0 {
        return Err("installing ignore did not discard pending signal");
    }
    signal::sigaction(signal::SIGUSR1, Some(&Sigaction::new(caught)), None)
        .map_err(|_| "reinstall caught signal failed")?;
    signal::sigprocmask(signal::SIG_UNBLOCK, Some(&bit), None)
        .map_err(|_| "unblock signal failed")?;
    if CAUGHT.load(Ordering::SeqCst) != 0 {
        return Err("discarded signal was delivered");
    }

    wait_check(false)?;
    // Keep SIGURG blocked in the original mask as well as the temporary mask.
    signal::sigaction(signal::SIGURG, Some(&Sigaction::new(unexpected)), None)
        .map_err(|_| "install blocked handler failed")?;
    signal::sigprocmask(
        signal::SIG_BLOCK,
        Some(&signal::sigmask(signal::SIGURG)),
        None,
    )
    .map_err(|_| "block SIGURG failed")?;
    wait_check(true)?;

    // A caught signal already pending when sigsuspend unmasks it runs once,
    // and sigreturn restores the original blocked mask.
    CAUGHT.store(0, Ordering::SeqCst);
    signal::sigprocmask(signal::SIG_BLOCK, Some(&bit), None)
        .map_err(|_| "block pending handler failed")?;
    signal::kill(pid, signal::SIGUSR1).map_err(|_| "queue pending handler failed")?;
    if !matches!(signal::sigsuspend(&0), Err(Error::Os(Errno::EINTR)))
        || CAUGHT.load(Ordering::SeqCst) != 1
    {
        return Err("pending caught signal did not interrupt sigsuspend");
    }
    let mut mask = 0;
    signal::sigprocmask(signal::SIG_SETMASK, None, Some(&mut mask))
        .map_err(|_| "read restored mask failed")?;
    if mask & bit == 0 {
        return Err("sigsuspend mask was not restored");
    }
    Ok(())
}
