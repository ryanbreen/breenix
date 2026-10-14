
use std::cell::{Cell, RefCell};
use std::ffi::{CStr, CString};

mod pthread {
    use super::Cell;
    thread_local!(static ERRNO: Cell<i32> = const { Cell::new(0) });
    pub fn errno() -> i32 { ERRNO.with(|e| e.get()) }
    pub fn set_errno(e: i32) { ERRNO.with(|v| v.set(e)); }
}
#[no_mangle]
pub static mut environ: *const *const u8 = core::ptr::null();
thread_local! {
    static PATH: RefCell<CString> = RefCell::new(CString::new("/one:/two").unwrap());
    static RESULTS: RefCell<Vec<i32>> = const { RefCell::new(Vec::new()) };
    static CALLS: RefCell<Vec<Vec<u8>>> = const { RefCell::new(Vec::new()) };
}
unsafe fn getenv(_: *const u8) -> *const u8 { PATH.with(|p| p.borrow().as_ptr() as *const u8) }
unsafe fn execve(path: *const u8, _: *const *const u8, _: *const *const u8) -> i32 {
    CALLS.with(|c| c.borrow_mut().push(CStr::from_ptr(path.cast()).to_bytes().to_vec()));
    pthread::set_errno(RESULTS.with(|r| r.borrow_mut().remove(0)));
    -1
}
unsafe fn malloc(n: usize) -> *mut u8 {
    let layout = std::alloc::Layout::from_size_align(n + 16, 16).unwrap();
    let p = std::alloc::alloc(layout);
    p.cast::<usize>().write(n);
    p.add(16)
}
unsafe fn free(p: *mut u8) {
    let base = p.sub(16);
    let n = base.cast::<usize>().read();
    std::alloc::dealloc(base, std::alloc::Layout::from_size_align(n + 16, 16).unwrap());
}
fn search(errors: &[i32]) -> i32 {
    RESULTS.with(|r| *r.borrow_mut() = errors.to_vec());
    CALLS.with(|c| c.borrow_mut().clear());
    let name = c"tool";
    let argv = [name.as_ptr() as *const u8, core::ptr::null()];
    unsafe { assert_eq!(execvp(name.as_ptr().cast(), argv.as_ptr()), -1); }
    pthread::errno()
}
#[test]
fn fatal_exec_errors_stop_at_first_candidate() {
    for error in [7, 12, 26, 40] {
        assert_eq!(search(&[error]), error);
        assert_eq!(CALLS.with(|c| c.borrow().len()), 1);
    }
}
#[test]
fn search_remembers_permission_denial_and_skips_missing_directories() {
    assert_eq!(search(&[13, 2]), 13);
    assert_eq!(search(&[20, 2]), 2);
}
#[test]
fn overlong_candidate_reports_name_too_long() {
    PATH.with(|p| *p.borrow_mut() = CString::new("x".repeat(4096)).unwrap());
    assert_eq!(search(&[]), 36);
    assert_eq!(CALLS.with(|c| c.borrow().len()), 0);
    PATH.with(|p| *p.borrow_mut() = CString::new("/one:/two").unwrap());
}
#[test]
fn executable_format_error_runs_shell_and_preserves_shell_error() {
    assert_eq!(search(&[8, 12]), 12);
    assert_eq!(CALLS.with(|c| c.borrow().clone()), [b"/one/tool".to_vec(), b"/bin/sh".to_vec()]);
}
#[test]
fn unsupported_prctl_reads_no_optional_arguments() {
    unsafe { assert_eq!(prctl(3), -1); }
    assert_eq!(pthread::errno(), 38);
    // Also type-check every exported exec wrapper without executing it.
    let _ = (execv, execvpe, execl, execle, execlp, c_len, with_arg_list);
    assert_eq!((EINTR, EINVAL, ENOTTY, ERANGE, AT_FDCWD, AT_SYMLINK_NOFOLLOW, CLOCK_REALTIME, S_IFMT, S_IFCHR),
               (4, 22, 25, 34, -100, 256, 0, 0o170000, 0o020000));
}
