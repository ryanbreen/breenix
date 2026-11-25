//! Run with rustc --edition=2021 --test tests/libc_x2_io_host.rs.
//! Exercise the hosted wrappers at the raw syscall boundary without a guest boot.

extern crate self as libbreenix;
extern crate alloc;

#[path = "../libs/libbreenix-libc/src/hosted_io.rs"]
mod hosted_io;
#[path = "../libs/libbreenix-libc/src/llvm_host.rs"]
mod llvm_host;
#[path = "../kernel/src/syscall/iovec_range.rs"]
mod iovec_range;

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
const ESPIPE: i32 = 29;

mod errno {
    pub const EINVAL: i32 = super::EINVAL;
    pub const EFAULT: i32 = 14;
    pub const ENOMEM: i32 = super::ENOMEM;
}

#[repr(C)]
struct Iovec {
    iov_base: *mut u8,
    iov_len: usize,
}

mod pthread {
    pub unsafe fn cancel_syscall(number: u64, args: [u64; 6]) -> i64 {
        assert_eq!(args[4..], [0, 0]);
        super::raw::syscall4(number, args[0], args[1], args[2], args[3]) as i64
    }
}

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
        pub const DUP3: u64 = if cfg!(target_arch = "x86_64") {
            292
        } else {
            24
        };
        pub const MADVISE: u64 = if cfg!(target_arch = "x86_64") {
            28
        } else {
            233
        };
        pub const MKDIRAT: u64 = if cfg!(target_arch = "x86_64") {
            258
        } else {
            34
        };
        pub const NEWFSTATAT: u64 = if cfg!(target_arch = "x86_64") {
            262
        } else {
            79
        };
        pub const SYMLINKAT: u64 = if cfg!(target_arch = "x86_64") {
            266
        } else {
            36
        };
        pub const READLINKAT: u64 = if cfg!(target_arch = "x86_64") {
            267
        } else {
            78
        };
        pub const SYNC: u64 = if cfg!(target_arch = "x86_64") {
            162
        } else {
            81
        };
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
            unsafe {
                status
                    .cast::<u8>()
                    .add(mode_offset)
                    .cast::<u32>()
                    .write(0o120777)
            };
            0
        });
        assert_eq!(
            unsafe { hosted_io::fstatat(DIR, PATH, status.cast(), 0x100) },
            0
        );
        assert_eq!(
            unsafe { status.cast::<u8>().add(mode_offset).cast::<u32>().read() },
            0o120777
        );
        observed(71);
    }

    #[test]
    fn getrusage_returns_child_cpu_times() {
        let mut usage = [0i64; 18];
        let status = usage.as_mut_ptr();
        setup_kernel(move |number, args| {
            assert_eq!(
                number,
                if cfg!(target_arch = "x86_64") {
                    98
                } else {
                    165
                }
            );
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
        setup(
            if cfg!(target_arch = "x86_64") {
                26
            } else {
                227
            },
            [address as u64, 8192, 4],
            -5,
        );
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
        setup(
            syscall::nr::MADVISE,
            [address as u64, 32, 3],
            -i64::from(ENOMEM),
        );
        assert_eq!(unsafe { llvm_host::posix_madvise(address, 32, 3) }, ENOMEM);
        observed(71);
    }

    #[test]
    fn posix_fadvise_passes_the_range_and_returns_errors_directly() {
        let number = if cfg!(target_arch = "x86_64") {
            221
        } else {
            223
        };
        setup(number, [5, 8192, 0, 3], 0);
        assert_eq!(unsafe { hosted_io::posix_fadvise(5, 8192, 0, 3) }, 0);
        observed(71);
        setup(number, [5, 8192, 0, 3], -29);
        assert_eq!(unsafe { hosted_io::posix_fadvise(5, 8192, 0, 3) }, 29);
        observed(71);
    }

    #[test]
    fn posix_fallocate_passes_the_range_and_returns_errors_directly() {
        let number = if cfg!(target_arch = "x86_64") {
            285
        } else {
            47
        };
        setup(number, [5, 0, 8192, 4096], 0);
        assert_eq!(unsafe { hosted_io::posix_fallocate(5, 8192, 4096) }, 0);
        observed(71);
        setup(number, [5, 0, 8192, 4096], -28);
        assert_eq!(unsafe { hosted_io::posix_fallocate(5, 8192, 4096) }, 28);
        observed(71);
    }

    #[test]
    fn preadv_scatters_a_short_read_from_the_explicit_offset() {
        use iovec_range::{positioned_offset, snapshot_vectors, vector_prefix};
        let number = if cfg!(target_arch = "x86_64") { 295 } else { 69 };
        let mut first = [b'!'; 2];
        let mut second = [b'!'; 4];
        let iov = [
            Iovec { iov_base: first.as_mut_ptr(), iov_len: first.len() },
            Iovec { iov_base: second.as_mut_ptr(), iov_len: second.len() },
        ];
        setup_kernel(move |actual, args| {
            assert_eq!(actual, number);
            let [fd, address, count, raw_offset] = *args else { panic!("wrong syscall arity") };
            assert_eq!(fd, 5);
            let offset = positioned_offset(raw_offset as i64, count as i32).unwrap();
            let (vectors, len) = snapshot_vectors(address, count,
                |address| Ok(unsafe { (address as *const iovec_range::IoVec).read() })).unwrap();
            let file = b"012345";
            let data = &file[offset as usize..][..len.min(file.len() - offset as usize)];
            for (base, range) in vector_prefix(&vectors, data.len()) {
                unsafe { std::ptr::copy_nonoverlapping(data[range.clone()].as_ptr(), base as *mut u8, range.len()) };
            }
            data.len() as i64
        });
        assert_eq!(unsafe { hosted_io::preadv(5, iov.as_ptr(), 2, 3) }, 3);
        assert_eq!((&first, &second), (b"34", b"5!!!"));
        observed(71);

        setup(number, [9, iov.as_ptr() as u64, 2, 3], -i64::from(ESPIPE));
        assert_eq!(unsafe { hosted_io::preadv(9, iov.as_ptr(), 2, 3) }, -1);
        observed(ESPIPE);
    }

    #[test]
    fn pwritev_gathers_one_transfer_and_rejects_bad_position_or_count() {
        use iovec_range::{positioned_offset, snapshot_vectors, vector_prefix, IoVec};
        let number = if cfg!(target_arch = "x86_64") { 296 } else { 70 };
        let mut first = *b"12";
        let mut second = *b"345";
        let iov = [
            Iovec { iov_base: first.as_mut_ptr(), iov_len: first.len() },
            Iovec { iov_base: second.as_mut_ptr(), iov_len: second.len() },
        ];
        setup_kernel(move |actual, args| {
            assert_eq!(actual, number);
            let [fd, address, count, raw_offset] = *args else { panic!("wrong syscall arity") };
            assert_eq!(fd, 5);
            let offset = positioned_offset(raw_offset as i64, count as i32).unwrap();
            let (vectors, len) = snapshot_vectors(address, count,
                |address| Ok(unsafe { (address as *const IoVec).read() })).unwrap();
            let mut file = *b"abcdefghi";
            let cursor = 6;
            let data = &mut file[offset as usize..][..len];
            for (base, range) in vector_prefix(&vectors, len) {
                unsafe { std::ptr::copy_nonoverlapping(base as *const u8, data[range.clone()].as_mut_ptr(), range.len()) };
            }
            assert_eq!(&file, b"ab12345hi");
            assert_eq!(file[cursor], b'5');
            len as i64
        });
        assert_eq!(unsafe { hosted_io::pwritev(5, iov.as_ptr(), 2, 2) }, 5);
        observed(71);

        for (count, offset) in [(2, -1), (0, 2), (1025, 2), (-1, 2)] {
            setup_kernel(move |actual, args| {
                assert_eq!(actual, number);
                let error = positioned_offset(args[3] as i64, args[2] as i32).unwrap_err();
                -(error as i64)
            });
            assert_eq!(unsafe { hosted_io::pwritev(5, iov.as_ptr(), count, offset) }, -1);
            observed(EINVAL);
        }
        let overflow = |address| Ok(IoVec {
            iov_base: 1,
            iov_len: if address == 16 { isize::MAX as u64 } else { 1 },
        });
        assert_eq!(snapshot_vectors(16, 2, overflow).err(), Some(EINVAL as u64));
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
        setup(
            syscall::nr::SYMLINKAT,
            [TARGET as u64, DIR as u64, PATH as u64],
            -i64::from(ENOENT),
        );
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
