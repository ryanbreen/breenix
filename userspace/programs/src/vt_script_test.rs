#[path = "vt_script_test/scripts.rs"]
mod scripts;
use std::io::{Read, Write};

fn main() {
    let name = std::env::args().nth(1).unwrap_or_default();
    if name == "--keys" {
        let fd = libbreenix::types::Fd::STDIN;
        let mut term = libbreenix::termios::Termios::default();
        libbreenix::termios::tcgetattr(fd, &mut term).expect("tcgetattr");
        libbreenix::termios::cfmakeraw(&mut term);
        libbreenix::termios::tcsetattr(fd, libbreenix::termios::TCSANOW, &term).expect("tcsetattr");
        let mut out = std::io::stdout().lock();
        write!(out, "KEYS READY\r\n").unwrap();
        out.flush().unwrap();
        let mut buf = [0u8; 64];
        let mut count = 0;
        loop {
            let n = std::io::stdin().read(&mut buf).unwrap();
            if n == 0 { break; }
            for byte in &buf[..n] { write!(out, "{byte:02x}").unwrap(); }
            write!(out, "\r\n").unwrap();
            count += 1;
            if count == 15 { write!(out, "\x1b[?1hAPPLICATION READY\r\n").unwrap(); }
            out.flush().unwrap();
        }
        return;
    }
    if name == "--session" {
        let session = libbreenix::process::getsid(0).expect("getsid");
        println!("SESSION {}", session.raw());
        return;
    }
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
