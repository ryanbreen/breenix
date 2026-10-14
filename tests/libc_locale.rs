// Run with rustc --test: this exercises only this lane's source on the host.
extern "C" {
    #[cfg_attr(target_os = "macos", link_name = "__error")]
    fn __errno_location() -> *mut i32;
    fn malloc(n: usize) -> *mut u8;
    fn free(p: *mut u8);
    fn getenv(name: *const u8) -> *mut u8;
}
mod pthread {
    use core::cell::UnsafeCell;
    use core::ffi::c_void;
    std::thread_local! { static LOCALE: UnsafeCell<*mut c_void> = const { UnsafeCell::new(core::ptr::null_mut()) }; }
    pub(crate) unsafe fn locale_slot() -> *mut *mut c_void { LOCALE.with(|slot| slot.get()) }
    pub(crate) fn set_errno(e: i32) { unsafe { *crate::__errno_location() = e; } }
}
#[path = "../libs/libbreenix-libc/src/locale/mod.rs"]
mod locale;
