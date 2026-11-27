//! Process and I/O support for Breenix userland commands.

use std::io::{self, Write};
use std::process::ExitCode;

const SIGPIPE: i32 = 13;
const SIG_DFL: usize = 0;
const SIG_ERR: usize = usize::MAX;
const STDOUT_FILENO: i32 = 1;

extern "C" {
    fn signal(number: i32, handler: usize) -> usize;
    fn write(fd: i32, buffer: *const u8, count: usize) -> isize;
}

/// Use the default SIGPIPE action so an output pipeline can stop its producer.
pub fn default_sigpipe() -> io::Result<()> {
    // SAFETY: SIGPIPE and SIG_DFL are valid for the supported Unix ABIs,
    // which pass the disposition and return value as pointer-sized values.
    if unsafe { signal(SIGPIPE, SIG_DFL) } == SIG_ERR {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Write to fd 1 and propagate descriptor errors to the command.
pub struct Stdout;

impl Write for Stdout {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        // SAFETY: bytes is readable for count bytes for the duration of write.
        let count = unsafe { write(STDOUT_FILENO, bytes.as_ptr(), bytes.len()) };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(count as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Finish a command, reporting an I/O error on stderr if possible.
pub fn finish(name: &str, result: io::Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr().lock(), "{name}: {error}");
            ExitCode::FAILURE
        }
    }
}
