// Run with rustc --test: this exercises only this lane's source on the host.
extern "C" {
    #[cfg_attr(target_os = "macos", link_name = "__error")]
    fn __errno_location() -> *mut i32;
    fn malloc(n: usize) -> *mut u8;
    fn free(p: *mut u8);
    fn getenv(name: *const u8) -> *mut u8;
    fn clock_gettime(id: i32, out: *mut u8) -> i32;
}
mod pthread {
    use core::cell::UnsafeCell;
    use core::ffi::c_void;
    std::thread_local! { static LOCALE: UnsafeCell<*mut c_void> = const { UnsafeCell::new(core::ptr::null_mut()) }; }
    pub(crate) unsafe fn locale_slot() -> *mut *mut c_void {
        LOCALE.with(|slot| slot.get())
    }
    pub(crate) fn set_errno(e: i32) {
        unsafe {
            *crate::__errno_location() = e;
        }
    }
}
mod env {
    pub(crate) fn getenv_bytes(name: &[u8]) -> Option<&'static [u8]> {
        let mut key = name.to_vec();
        key.push(0);
        unsafe {
            let value = crate::getenv(key.as_ptr());
            if value.is_null() {
                return None;
            }
            let mut n = 0;
            while *value.add(n) != 0 {
                n += 1;
            }
            Some(core::slice::from_raw_parts(value, n))
        }
    }
}
#[path = "../libs/libbreenix-libc/src/locale/mod.rs"]
mod locale;
