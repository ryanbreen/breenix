//! ENOSYS syscall test - verifies that undefined syscalls return -ENOSYS
//!
//! Tests that syscall 999 (guaranteed unimplemented) returns -38 (ENOSYS).

use libbreenix::raw;

fn main() {
    let rv = unsafe { raw::syscall0(999) } as i64;
    if rv == -38 {
        println!("ENOSYS OK");
    } else {
        println!("ENOSYS FAIL (got {})", rv);
    }
}
