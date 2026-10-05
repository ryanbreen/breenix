//! Exec target for the directories suite's fork/exec credential check.
use libbreenix::{
    process,
    syscall::{nr, raw},
};

fn main() {
    let mut groups = [0u32; 2];
    // SAFETY: the output array is writable for the entire syscall.
    let valid = unsafe {
        raw::syscall1(nr::UMASK, 0) == 0o027
            && raw::syscall2(nr::GETGROUPS, 2, groups.as_mut_ptr() as u64) == 2
            && groups == [1001, 2345]
    };
    process::exit(if valid { 0 } else { 1 });
}
