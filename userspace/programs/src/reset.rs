use libbreenix::termios::{cc, iflag, lflag, oflag, tcgetattr, tcsetattr, Termios, TCSANOW};
use libbreenix::types::Fd;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::fd::AsRawFd;

const RIS: &[u8] = b"\x1bc";

fn sane_termios(mut attrs: Termios) -> Termios {
    // Restore the Breenix line discipline defaults, preserving serial settings.
    attrs.c_iflag = iflag::ICRNL;
    attrs.c_oflag = oflag::OPOST | oflag::ONLCR;
    attrs.c_lflag =
        lflag::ISIG | lflag::ICANON | lflag::ECHO | lflag::ECHOE | lflag::ECHOK | lflag::IEXTEN;
    attrs.c_cc = [0; cc::NCCS];
    attrs.c_cc[cc::VINTR] = b'\x03'; // Ctrl-C
    attrs.c_cc[cc::VQUIT] = b'\x1c'; // Ctrl-backslash
    attrs.c_cc[cc::VERASE] = b'\x7f'; // DEL
    attrs.c_cc[cc::VKILL] = b'\x15'; // Ctrl-U
    attrs.c_cc[cc::VEOF] = b'\x04'; // Ctrl-D
    attrs.c_cc[cc::VMIN] = 1;
    attrs.c_cc[cc::VSTART] = b'\x11'; // Ctrl-Q
    attrs.c_cc[cc::VSTOP] = b'\x13'; // Ctrl-S
    attrs.c_cc[cc::VSUSP] = b'\x1a'; // Ctrl-Z
    attrs.c_cc[cc::VWERASE] = b'\x17'; // Ctrl-W
    attrs.c_cc[cc::VLNEXT] = b'\x16'; // Ctrl-V
    attrs
}

fn run() -> Result<(), String> {
    let mut tty = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|error| format!("/dev/tty: {error}"))?;
    let fd = Fd::from_raw(tty.as_raw_fd() as u64);
    let mut attrs = Termios::default();
    tcgetattr(fd, &mut attrs).map_err(|error| format!("tcgetattr: {error}"))?;
    tcsetattr(fd, TCSANOW, &sane_termios(attrs))
        .map_err(|error| format!("tcsetattr: {error}"))?;
    tty.write_all(RIS).map_err(|error| format!("write: {error}"))
}

fn main() {
    if let Err(error) = run() {
        eprintln!("reset: {error}");
        std::process::exit(1);
    }
}
