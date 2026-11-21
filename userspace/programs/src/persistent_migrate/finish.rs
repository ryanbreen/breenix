use std::{
    io::{self, Write},
    process::ExitCode,
};

pub fn power_off_after(
    result: io::Result<()>,
    request_poweroff: impl FnOnce() -> io::Result<()>,
) -> ExitCode {
    let status = match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("[migrate] failed: {}", error.to_string().escape_debug());
            1
        }
    };
    let receipt = {
        let mut serial = io::stdout().lock();
        writeln!(serial, "\n[migrate] exit code: {status}").and_then(|()| serial.flush())
    };
    if let Err(error) = receipt {
        eprintln!("[migrate] could not write exit receipt: {error}");
    }
    if let Err(error) = request_poweroff() {
        eprintln!("[migrate] poweroff failed: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::from(status)
}
