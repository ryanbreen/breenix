//! Bounded rendezvous for the interrupted-save case.
use std::{io, path::Path, time::{Duration, Instant}};

pub fn wait(control: &Path, name: &str) -> io::Result<()> {
    let start = Instant::now();
    loop {
        if control.join(name).try_exists()? { return Ok(()); }
        if start.elapsed() >= Duration::from_secs(10) {
            return Err(io::Error::new(io::ErrorKind::TimedOut, format!("save barrier {name}")));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

