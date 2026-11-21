use std::{
    io::{self, Write},
    process::ExitCode,
};

pub fn migrate_and_power_off(
    prepare_shutdown_nonce: impl FnOnce() -> io::Result<String>,
    migrate: impl FnOnce() -> io::Result<()>,
    request_poweroff: impl FnOnce(&str) -> io::Result<()>,
) -> ExitCode {
    let (result, nonce) = match prepare_shutdown_nonce() {
        Ok(nonce) => (migrate(), Some(nonce)),
        Err(error) => (
            Err(io::Error::other(format!("prepare shutdown nonce: {error}"))),
            None,
        ),
    };
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
    if let Some(nonce) = nonce {
        if let Err(error) = request_poweroff(&nonce) {
            eprintln!("[migrate] poweroff failed: {error}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::from(status)
}
