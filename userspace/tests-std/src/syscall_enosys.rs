//! ENOSYS syscall test - verifies that undefined syscalls return -ENOSYS
//!
//! Tests that syscall 999 (guaranteed unimplemented) returns -38 (ENOSYS).

extern "C" {
    fn syscall(num: i64, a1: i64, a2: i64, a3: i64, a4: i64, a5: i64, a6: i64) -> i64;
}

fn main() {
    let rv = unsafe { syscall(999, 0, 0, 0, 0, 0, 0) };
    if rv == -38 {
        println!("ENOSYS OK");
    } else {
        println!("ENOSYS FAIL (got {})", rv);
    }
}
