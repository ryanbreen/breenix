//! Run with rustc --edition=2021 --test tests/libc_x2_io_host.rs.
//! Exercise the hosted wrappers at the raw syscall boundary without a guest boot.

extern crate self as libbreenix;

#[path = "../libs/libbreenix-libc/src/hosted_io.rs"]
mod hosted_io;
#[path = "../libs/libbreenix-libc/src/llvm_host.rs"]
mod llvm_host;

use std::cell::{Cell, RefCell};

type KernelCall = Box<dyn FnOnce(u64, &[u64]) -> i64>;

thread_local! {
    static KERNEL: RefCell<Option<KernelCall>> = const { RefCell::new(None) };
    static ERRNO: Cell<i32> = const { Cell::new(0) };
}

const EINVAL: i32 = 22;
const EACCES: i32 = 13;
const ENOENT: i32 = 2;
const ENOMEM: i32 = 12;

fn setup(number: u64, args: impl Into<Vec<u64>>, result: i64) {
    let args = args.into();
    setup_kernel(move |actual_number, actual_args| {
        assert_eq!((actual_number, actual_args), (number, args.as_slice()));
        result
    });
}

fn setup_kernel(call: impl FnOnce(u64, &[u64]) -> i64 + 'static) {
    ERRNO.set(71);
    KERNEL.with(|slot| assert!(slot.replace(Some(Box::new(call))).is_none()));
}

fn observed(errno: i32) {
    KERNEL.with(|slot| assert!(slot.borrow().is_none(), "syscall not issued"));
    assert_eq!(ERRNO.get(), errno);
}

fn syscall_result_to_c_int(result: i64) -> i32 {
    syscall_result_to_c_ssize(result) as i32
}

fn syscall_result_to_c_ssize(result: i64) -> isize {
    if result < 0 {
        ERRNO.set((-result) as i32);
        -1
    } else {
        result as isize
    }
}

mod raw {
    pub unsafe fn syscall0(number: u64) -> u64 {
        call(number, &[])
    }

    pub unsafe fn syscall2(number: u64, a: u64, b: u64) -> u64 {
        call(number, &[a, b])
    }

    pub unsafe fn syscall3(number: u64, a: u64, b: u64, c: u64) -> u64 {
        call(number, &[a, b, c])
    }

    pub unsafe fn syscall4(number: u64, a: u64, b: u64, c: u64, d: u64) -> u64 {
        call(number, &[a, b, c, d])
    }

    fn call(number: u64, args: &[u64]) -> u64 {
        let behavior = super::KERNEL.with(|slot| slot.take().expect("unexpected syscall"));
        behavior(number, args) as u64
    }
}

mod syscall {
    pub mod nr {
        pub const DUP3: u64 = if cfg!(target_arch = "x86_64") { 292 } else { 24 };
        pub const MADVISE: u64 = if cfg!(target_arch = "x86_64") { 28 } else { 233 };
        pub const MKDIRAT: u64 = if cfg!(target_arch = "x86_64") { 258 } else { 34 };
        pub const NEWFSTATAT: u64 = if cfg!(target_arch = "x86_64") { 262 } else { 79 };
        pub const SYMLINKAT: u64 = if cfg!(target_arch = "x86_64") { 266 } else { 36 };
        pub const READLINKAT: u64 = if cfg!(target_arch = "x86_64") { 267 } else { 78 };
        pub const SYNC: u64 = if cfg!(target_arch = "x86_64") { 162 } else { 81 };
    }
}

mod libc_host {
    use super::*;

    const PATH: *const u8 = c"entry".as_ptr().cast();
    const TARGET: *const u8 = c"destination".as_ptr().cast();
    const DIR: i32 = 17;

    #[test]
    fn dup3_returns_replaced_descriptor_with_cloexec() {
        setup(syscall::nr::DUP3, [5, 19, 0x80000], 19);
        assert_eq!(unsafe { hosted_io::dup3(5, 19, 0x80000) }, 19);
        observed(71);
        setup(syscall::nr::DUP3, [5, 5, 0], -i64::from(EINVAL));
        assert_eq!(unsafe { hosted_io::dup3(5, 5, 0) }, -1);
        observed(EINVAL);
    }

    #[test]
    fn faccessat_checks_relative_path_with_effective_ids() {
        setup(439, [DIR as u64, PATH as u64, 2, 0x200], -i64::from(EACCES));
        assert_eq!(unsafe { hosted_io::faccessat(DIR, PATH, 2, 0x200) }, -1);
        observed(EACCES);
    }

    #[test]
    fn fstatat_returns_status_without_following_final_link() {
        let mut stat = [0u64; 18];
        let status = stat.as_mut_ptr();
        let mode_offset = if cfg!(target_arch = "x86_64") { 24 } else { 16 };
        setup_kernel(move |number, args| {
            assert_eq!(number, syscall::nr::NEWFSTATAT);
            assert_eq!(args, [DIR as u64, PATH as u64, status as u64, 0x100]);
            unsafe { status.cast::<u8>().add(mode_offset).cast::<u32>().write(0o120777) };
            0
        });
        assert_eq!(unsafe { hosted_io::fstatat(DIR, PATH, status.cast(), 0x100) }, 0);
        assert_eq!(unsafe { status.cast::<u8>().add(mode_offset).cast::<u32>().read() }, 0o120777);
        observed(71);
    }

    #[test]
    fn getrusage_returns_child_cpu_times() {
        let mut usage = [0i64; 18];
        let status = usage.as_mut_ptr();
        setup_kernel(move |number, args| {
            assert_eq!(number, if cfg!(target_arch = "x86_64") { 98 } else { 165 });
            assert_eq!(args, [u64::MAX, status as u64]);
            unsafe { status.cast::<[i64; 4]>().write([7, 400, 2, 84]) };
            0
        });
        assert_eq!(unsafe { llvm_host::getrusage(-1, status.cast()) }, 0);
        assert_eq!(usage[..4], [7, 400, 2, 84]);
        observed(71);
    }

    #[test]
    fn mkdirat_creates_under_the_directory_descriptor() {
        setup(syscall::nr::MKDIRAT, [DIR as u64, PATH as u64, 0o750], 0);
        assert_eq!(unsafe { hosted_io::mkdirat(DIR, PATH, 0o750) }, 0);
        observed(71);
    }

    #[test]
    fn msync_requests_synchronous_writeback_and_propagates_failure() {
        let address = 0x2000 as *mut u8;
        setup(if cfg!(target_arch = "x86_64") { 26 } else { 227 },
              [address as u64, 8192, 4], -5);
        assert_eq!(unsafe { hosted_io::msync(address, 8192, 4) }, -1);
        observed(5);
    }

    #[test]
    fn posix_madvise_dontneed_preserves_contents_and_errno() {
        let mut bytes = [7u8; 32];
        let address = bytes.as_mut_ptr();
        // The destructive MADV_DONTNEED is not a valid implementation of this hint.
        setup(syscall::nr::MADVISE, [address as u64, 32, 0], 0);
        assert_eq!(unsafe { llvm_host::posix_madvise(address, 32, 4) }, 0);
        assert_eq!(bytes, [7; 32]);
        observed(71);
        setup(syscall::nr::MADVISE, [address as u64, 32, 3], -i64::from(ENOMEM));
        assert_eq!(unsafe { llvm_host::posix_madvise(address, 32, 3) }, ENOMEM);
        observed(71);
    }

    #[test]
    fn posix_fadvise_passes_the_range_and_returns_errors_directly() {
        let number = if cfg!(target_arch = "x86_64") { 221 } else { 223 };
        setup(number, [5, 8192, 0, 3], 0);
        assert_eq!(unsafe { hosted_io::posix_fadvise(5, 8192, 0, 3) }, 0);
        observed(71);
        setup(number, [5, 8192, 0, 3], -29);
        assert_eq!(unsafe { hosted_io::posix_fadvise(5, 8192, 0, 3) }, 29);
        observed(71);
    }

    #[test]
    fn posix_fallocate_passes_the_range_and_returns_errors_directly() {
        let number = if cfg!(target_arch = "x86_64") { 285 } else { 47 };
        setup(number, [5, 0, 8192, 4096], 0);
        assert_eq!(unsafe { hosted_io::posix_fallocate(5, 8192, 4096) }, 0);
        observed(71);
        setup(number, [5, 0, 8192, 4096], -28);
        assert_eq!(unsafe { hosted_io::posix_fallocate(5, 8192, 4096) }, 28);
        observed(71);
    }

    #[test]
    fn readlinkat_returns_bytes_and_never_adds_a_terminator() {
        let mut bytes = [0xccu8; 5];
        let buffer = bytes.as_mut_ptr();
        setup_kernel(move |number, args| {
            assert_eq!(number, syscall::nr::READLINKAT);
            assert_eq!(args, [DIR as u64, PATH as u64, buffer as u64, 3]);
            unsafe { buffer.cast::<[u8; 3]>().write(*b"des") };
            3
        });
        assert_eq!(unsafe { hosted_io::readlinkat(DIR, PATH, buffer, 3) }, 3);
        assert_eq!(&bytes, b"des\xcc\xcc");
        observed(71);
    }

    #[test]
    fn symlinkat_relates_the_link_name_to_dirfd_and_keeps_target_literal() {
        setup(syscall::nr::SYMLINKAT, [TARGET as u64, DIR as u64, PATH as u64],
              -i64::from(ENOENT));
        assert_eq!(unsafe { hosted_io::symlinkat(TARGET, DIR, PATH) }, -1);
        observed(ENOENT);
    }

    #[test]
    fn sync_dispatches_system_wide_writeback() {
        setup(syscall::nr::SYNC, [], 0);
        hosted_io::sync();
        observed(71);
    }
}
