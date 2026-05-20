use libbreenix::fs::{self, O_RDONLY};
use libbreenix::io;

fn main() {
    let fd = match fs::open("/proc/xhci/counters", O_RDONLY) {
        Ok(fd) => fd,
        Err(e) => {
            print!("[xhci-counters] open failed: {}\n", e);
            std::process::exit(1);
        }
    };

    let mut buf = [0u8; 512];
    let n = match io::read(fd, &mut buf) {
        Ok(n) => n,
        Err(e) => {
            let _ = io::close(fd);
            print!("[xhci-counters] read failed: {}\n", e);
            std::process::exit(1);
        }
    };
    let _ = io::close(fd);

    let text = core::str::from_utf8(&buf[..n]).unwrap_or("");
    for line in text.lines() {
        print!("[xhci-counters] {}\n", line);
    }
}
