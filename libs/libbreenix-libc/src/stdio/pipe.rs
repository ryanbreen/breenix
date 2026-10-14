//! POSIX popen/pclose, using /bin/sh and the FILE registry for inherited pipe cleanup.
use super::file::{self, File};
use core::ptr;
#[no_mangle]
pub unsafe extern "C" fn popen(command: *const u8, mode: *const u8) -> *mut File {
    if command.is_null()
        || mode.is_null()
        || !(*mode == b'r' || *mode == b'w')
        || !(*mode.add(1) == 0 || (*mode.add(1) == b'e' && *mode.add(2) == 0))
    {
        crate::pthread::set_errno(22);
        return ptr::null_mut();
    }
    let read = *mode == b'r';
    let cloexec = *mode.add(1) == b'e';
    let mut fds = [0; 2];
    if crate::pipe2(fds.as_mut_ptr(), 0x80000) < 0 {
        return ptr::null_mut();
    }
    let parent = fds[if read { 0 } else { 1 }];
    let child = fds[if read { 1 } else { 0 }];
    let stream = file::fdopen(parent, if read { c"r" } else { c"w" }.as_ptr() as *const u8);
    if stream.is_null() {
        let e = crate::pthread::errno();
        crate::close(parent);
        crate::close(child);
        crate::pthread::set_errno(e);
        return ptr::null_mut();
    }
    // Both ends stay close-on-exec until fork_pipe marks the stream as a popen stream.
    let pid = file::fork_pipe(stream, cloexec);
    if pid == 0 {
        file::close_inherited_pipes(stream);
        crate::close(parent);
        let target = if read { 1 } else { 0 };
        if child == target {
            if crate::fcntl(target, 2, 0) < 0 {
                crate::_exit(127)
            }
        } else {
            if crate::dup2(child, target) < 0 {
                crate::_exit(127)
            }
            crate::close(child);
        }
        let args = [
            c"sh".as_ptr() as *const u8,
            c"-c".as_ptr() as *const u8,
            c"--".as_ptr() as *const u8,
            command,
            ptr::null(),
        ];
        crate::execve(
            c"/bin/sh".as_ptr() as *const u8,
            args.as_ptr(),
            crate::env::environ as *const *const u8,
        );
        crate::_exit(127)
    }
    let saved = crate::pthread::errno();
    crate::close(child);
    if pid < 0 {
        file::fclose(stream);
        crate::pthread::set_errno(saved);
        ptr::null_mut()
    } else {
        stream
    }
}
#[no_mangle]
pub unsafe extern "C" fn pclose(stream: *mut File) -> i32 {
    let pid = file::take_pipe_pid(stream);
    if pid <= 0 {
        crate::pthread::set_errno(22);
        return -1;
    }
    let closed = file::fclose(stream);
    let saved = crate::pthread::errno();
    let mut status = 0;
    loop {
        let r = crate::waitpid(pid, &mut status, 0);
        if r >= 0 {
            break;
        }
        if crate::pthread::errno() != 4 {
            return -1;
        }
    }
    if closed < 0 {
        crate::pthread::set_errno(saved);
        -1
    } else {
        status
    }
}
