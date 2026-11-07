
use std::cell::{Cell, RefCell};
use std::ffi::{CStr, CString};

extern crate self as libbreenix;
pub mod process {
    pub const PR_SET_CHILD_SUBREAPER: i32 = 0x42520007;
    pub const PR_GET_CHILD_SUBREAPER: i32 = 0x42520008;
}
pub mod nr {
    pub const PRCTL: u64 = 167;
}
pub mod raw {
    pub unsafe fn syscall2(number: u64, option: u64, arg: u64) -> u64 {
        assert_eq!(number, super::nr::PRCTL);
        super::PRCTL_CALL.with(|call| call.set(Some((option, arg))));
        super::PRCTL_RESULT.with(|result| result.get() as u64)
    }
}
pub fn syscall_result_to_c_int(result: i64) -> i32 {
    if result < 0 { pthread::set_errno(-result as i32); -1 } else { result as i32 }
}
thread_local! {
    static PRCTL_CALL: Cell<Option<(u64, u64)>> = const { Cell::new(None) };
    static PRCTL_RESULT: Cell<i64> = const { Cell::new(0) };
}

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
fn overlong_candidate_is_skipped_for_later_path_entry() {
    PATH.with(|p| *p.borrow_mut() = CString::new(format!("{}:/two", "x".repeat(4096))).unwrap());
    assert_eq!(search(&[13]), 13);
    assert_eq!(CALLS.with(|c| c.borrow().clone()), [b"/two/tool".to_vec()]);
    PATH.with(|p| *p.borrow_mut() = CString::new("/one:/two").unwrap());
}
#[test]
fn executable_format_error_runs_shell_and_preserves_shell_error() {
    assert_eq!(search(&[8, 12]), 12);
    assert_eq!(CALLS.with(|c| c.borrow().clone()), [b"/one/tool".to_vec(), b"/bin/sh".to_vec()]);
}
#[test]
fn unsupported_prctl_reads_no_optional_arguments() {
    PRCTL_CALL.with(|call| call.set(None));
    unsafe { assert_eq!(prctl(3), -1); }
    assert_eq!(pthread::errno(), 38);
    assert_eq!(PRCTL_CALL.with(Cell::get), None);
    // Also type-check every exported exec wrapper without executing it.
    let _ = (execv, execvpe, execl, execle, execlp, c_len, with_arg_list);
}

#[test]
fn subreaper_prctl_routes_only_its_typed_argument_and_translates_errno() {
    PRCTL_RESULT.with(|result| result.set(0));
    unsafe { assert_eq!(prctl(process::PR_SET_CHILD_SUBREAPER, 1i32), 0); }
    assert_eq!(PRCTL_CALL.with(Cell::get), Some((0x42520007, 1)));
    let mut flag = 0i32;
    unsafe { assert_eq!(prctl(process::PR_GET_CHILD_SUBREAPER, &mut flag as *mut i32), 0); }
    assert_eq!(PRCTL_CALL.with(Cell::get), Some((0x42520008, &mut flag as *mut i32 as u64)));
    PRCTL_RESULT.with(|result| result.set(-14));
    unsafe { assert_eq!(prctl(process::PR_GET_CHILD_SUBREAPER, core::ptr::null_mut::<i32>()), -1); }
    assert_eq!(pthread::errno(), 14);
}
