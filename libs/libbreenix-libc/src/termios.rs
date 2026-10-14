//! Clean-room POSIX terminal functions using Breenix's Linux-compatible ioctl ABI.
use libbreenix::termios::{Termios, IoctlTermios, Winsize, request};

fn invalid() -> i32 {
    unsafe { *crate::__errno_location() = 22; }
    -1
}

#[no_mangle]
pub unsafe extern "C" fn tcgetattr(fd: i32, attrs: *mut Termios) -> i32 {
    if attrs.is_null() { return crate::ioctl(fd, request::TCGETS, 0); }
    let mut wire = IoctlTermios::default();
    let ret = crate::ioctl(fd, request::TCGETS, &mut wire as *mut _ as u64);
    if ret == 0 { attrs.write(wire.into()); }
    ret
}

#[no_mangle]
pub unsafe extern "C" fn tcsetattr(fd: i32, action: i32, attrs: *const Termios) -> i32 {
    let req = match action { 0 => request::TCSETS, 1 => request::TCSETSW,
        2 => request::TCSETSF, _ => return invalid() };
    if attrs.is_null() { return crate::ioctl(fd, req, 0); }
    let wire = IoctlTermios::from(*attrs);
    crate::ioctl(fd, req, &wire as *const _ as u64)
}

const CBAUD: u32 = 0x100f;
const CIBAUD: u32 = CBAUD << 16;
fn valid_speed(speed: u32) -> bool { speed <= 15 || (0x1001..=0x100f).contains(&speed) }

#[no_mangle]
pub unsafe extern "C" fn cfgetospeed(attrs: *const Termios) -> u32 { (*attrs).c_cflag & CBAUD }
#[no_mangle]
pub unsafe extern "C" fn cfgetispeed(attrs: *const Termios) -> u32 {
    if (*attrs).c_ispeed == 0 { cfgetospeed(attrs) } else { (*attrs).c_ispeed }
}
#[no_mangle]
pub unsafe extern "C" fn cfsetospeed(attrs: *mut Termios, speed: u32) -> i32 {
    if !valid_speed(speed) { return invalid(); }
    (*attrs).c_cflag = ((*attrs).c_cflag & !CBAUD) | speed;
    (*attrs).c_ospeed = speed;
    if (*attrs).c_cflag & CIBAUD == 0 { (*attrs).c_ispeed = speed; }
    0
}
#[no_mangle]
pub unsafe extern "C" fn cfsetispeed(attrs: *mut Termios, speed: u32) -> i32 {
    if !valid_speed(speed) { return invalid(); }
    (*attrs).c_cflag = ((*attrs).c_cflag & !CIBAUD) | (speed << 16);
    (*attrs).c_ispeed = if speed == 0 { cfgetospeed(attrs) } else { speed };
    0
}
#[no_mangle]
pub unsafe extern "C" fn tcflush(fd: i32, selector: i32) -> i32 {
    crate::ioctl(fd, request::TCFLSH, selector as u64)
}
/// A cancellation point: it waits for output to drain.
#[no_mangle]
pub unsafe extern "C" fn tcdrain(fd: i32) -> i32 {
    crate::syscall_result_to_c_int(crate::pthread::cancel_syscall(libbreenix::syscall::nr::IOCTL,
        [fd as u64, request::TCSBRK as u64, 1, 0, 0, 0]))
}
#[no_mangle]
pub unsafe extern "C" fn tcflow(fd: i32, action: i32) -> i32 {
    crate::ioctl(fd, request::TCXONC, action as u64)
}
#[no_mangle]
pub unsafe extern "C" fn tcsendbreak(fd: i32, duration: i32) -> i32 {
    if duration <= 0 { crate::ioctl(fd, request::TCSBRK, 0) }
    else { crate::ioctl(fd, request::TCSBRKP, duration as u64) }
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

/// POSIX.1-2024 window size functions.
#[no_mangle]
pub unsafe extern "C" fn tcgetwinsize(fd: i32, size: *mut Winsize) -> i32 {
    crate::ioctl(fd, request::TIOCGWINSZ, size as u64)
}
#[no_mangle]
pub unsafe extern "C" fn tcsetwinsize(fd: i32, size: *const Winsize) -> i32 {
    crate::ioctl(fd, request::TIOCSWINSZ, size as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speeds_preserve_control_bits_and_invalid_values_leave_attributes_unchanged() {
        let mut attrs = Termios::default();
        attrs.c_cflag = 0x80 | 0x30;
        unsafe {
            assert_eq!(cfsetospeed(&mut attrs, 13), 0);
            assert_eq!(cfgetospeed(&attrs), 13);
            assert_eq!(attrs.c_cflag & !CBAUD, 0xb0);
            assert_eq!(cfsetispeed(&mut attrs, 14), 0);
            assert_eq!(cfgetispeed(&attrs), 14);
            assert_eq!(cfgetospeed(&attrs), 13);
            assert_eq!((attrs.c_cflag & CIBAUD) >> 16, 14);
            let before = attrs.c_cflag;
            assert_eq!(cfsetospeed(&mut attrs, 12345), -1);
            assert_eq!(*crate::__errno_location(), 22);
            assert_eq!(attrs.c_cflag, before);
            assert_eq!(cfsetispeed(&mut attrs, 0), 0);
            assert_eq!(cfgetispeed(&attrs), cfgetospeed(&attrs));
        }
    }

    #[test]
    fn linux_baud_encodings_have_no_gaps_accepted_as_rates() {
        for speed in 0..=15 { assert!(valid_speed(speed)); }
        for speed in 0x1001..=0x100f { assert!(valid_speed(speed)); }
        for speed in [16, 4096, 4112, 9600, 38400, u32::MAX] { assert!(!valid_speed(speed)); }
    }
    #[test]
    fn unknown_tcsetattr_actions_fail_before_an_ioctl() {
        for action in [-1, 3, 99, i32::MIN, i32::MAX] {
            unsafe {
                assert_eq!(tcsetattr(-1, action, core::ptr::null()), -1);
                assert_eq!(*crate::__errno_location(), 22);
            }
        }
    }

}
