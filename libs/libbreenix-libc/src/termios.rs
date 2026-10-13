//! Clean-room POSIX terminal functions using Breenix's Linux-compatible ioctl ABI.
use libbreenix::termios::{Termios, request};

fn invalid() -> i32 {
    unsafe { *crate::__errno_location() = 22; }
    -1
}

#[no_mangle]
pub unsafe extern "C" fn tcgetattr(fd: i32, attrs: *mut Termios) -> i32 {
    crate::ioctl(fd, request::TCGETS, attrs as u64)
}

#[no_mangle]
pub unsafe extern "C" fn tcsetattr(fd: i32, action: i32, attrs: *const Termios) -> i32 {
    let req = match action { 0 => request::TCSETS, 1 => request::TCSETSW,
        2 => request::TCSETSF, _ => return invalid() };
    crate::ioctl(fd, req, attrs as u64)
}

const CBAUD: u32 = 0x100f;
fn valid_speed(speed: u32) -> bool { speed <= 15 || (0x1001..=0x100f).contains(&speed) }

#[no_mangle]
pub unsafe extern "C" fn cfgetospeed(attrs: *const Termios) -> u32 { (*attrs).c_cflag & CBAUD }
#[no_mangle]
pub unsafe extern "C" fn cfgetispeed(attrs: *const Termios) -> u32 { (*attrs).c_cflag & CBAUD }
#[no_mangle]
pub unsafe extern "C" fn cfsetospeed(attrs: *mut Termios, speed: u32) -> i32 {
    if !valid_speed(speed) { return invalid(); }
    (*attrs).c_cflag = ((*attrs).c_cflag & !CBAUD) | speed;
    (*attrs).c_ospeed = speed;
    0
}
#[no_mangle]
pub unsafe extern "C" fn cfsetispeed(attrs: *mut Termios, speed: u32) -> i32 {
    if !valid_speed(speed) { return invalid(); }
    let speed = if speed == 0 { cfgetospeed(attrs) } else { speed };
    (*attrs).c_cflag = ((*attrs).c_cflag & !CBAUD) | speed;
    (*attrs).c_ispeed = speed;
    0
}
#[no_mangle]
pub unsafe extern "C" fn tcflush(fd: i32, selector: i32) -> i32 {
    crate::ioctl(fd, request::TCFLSH, selector as u64)
}
#[no_mangle]
pub unsafe extern "C" fn tcdrain(fd: i32) -> i32 { crate::ioctl(fd, request::TCSBRK, 1) }
#[no_mangle]
pub unsafe extern "C" fn tcflow(fd: i32, action: i32) -> i32 {
    crate::ioctl(fd, request::TCXONC, action as u64)
}
#[no_mangle]
pub unsafe extern "C" fn tcsendbreak(fd: i32, duration: i32) -> i32 {
    crate::ioctl(fd, request::TCSBRK, duration as u64)
}
#[no_mangle]
pub unsafe extern "C" fn tcgetpgrp(fd: i32) -> i32 {
    let mut pgid = 0i32;
    if crate::ioctl(fd, request::TIOCGPGRP, &mut pgid as *mut i32 as u64) < 0 { -1 } else { pgid }
}
#[no_mangle]
pub unsafe extern "C" fn tcsetpgrp(fd: i32, pgid: i32) -> i32 {
    crate::ioctl(fd, request::TIOCSPGRP, &pgid as *const i32 as u64)
}
#[no_mangle]
pub unsafe extern "C" fn tcgetsid(fd: i32) -> i32 {
    let mut sid = 0i32;
    if crate::ioctl(fd, request::TIOCGSID, &mut sid as *mut i32 as u64) < 0 { -1 } else { sid }
}
