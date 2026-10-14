// Run with rustc --test: this exercises only this lane's source on the host.
extern "C" { fn __errno_location() -> *mut i32; }
#[path = "../libs/libbreenix-libc/src/locale/mod.rs"]
mod locale;
