//! Clear the display and scrollback on the terminal connected to stdout.

use std::io::{self, Write};

fn main() -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(b"\x1b[H\x1b[2J\x1b[3J")?;
    stdout.flush()
}
