#[path = "vt_script_test/scripts.rs"]
mod scripts;
use std::io::{Read, Write};

fn main() {
    let name = std::env::args().nth(1).unwrap_or_default();
    if name == "--size" {
        match libbreenix::termios::get_winsize(libbreenix::types::Fd::STDIN) {
            Ok(size) => println!("{} {}", size.ws_row, size.ws_col),
            Err(error) => { eprintln!("TIOCGWINSZ: {error}"); std::process::exit(1); }
        }
        return;
    }
    let Some(bytes) = scripts::script(&name) else { eprintln!("unknown script: {name}"); std::process::exit(1); };
    let mut out = std::io::stdout().lock();
    for byte in bytes { out.write_all(&[*byte]).unwrap(); out.flush().unwrap(); }
    let _ = std::io::stdin().read(&mut [0]);
}
