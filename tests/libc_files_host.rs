//! Focused host tests for the FILES implementations, linked to the host allocator.
//! Compile directly with rustc --test; no kernel or other libc modules are built.
#[path = "../libs/libbreenix-libc/src/getopt.rs"]
mod getopt;
#[path = "../libs/libbreenix-libc/src/regex/mod.rs"]
mod regex;
mod env {
    pub fn getenv_bytes(_: &[u8]) -> Option<&'static [u8]> {
        None
    }
}
#[test]
fn missing_argument_advances_two_and_unknown_suboption_value() {
    unsafe {
        let mut program = *b"test\0";
        let mut option = *b"-x\0";
        let argv = [
            program.as_mut_ptr(),
            option.as_mut_ptr(),
            core::ptr::null_mut(),
        ];
        getopt::optind = 0;
        getopt::opterr = 0;
        assert_eq!(
            getopt::getopt(2, argv.as_ptr(), b":x:\0".as_ptr()),
            b':' as i32
        );
        let index = getopt::optind;
        assert_eq!(index, 3);
        let mut options = *b"unknown=value,other\0";
        let mut next = options.as_mut_ptr();
        let tokens = [b"known\0".as_ptr(), core::ptr::null()];
        let mut value = core::ptr::null_mut();
        assert_eq!(
            getopt::getsubopt(&mut next, tokens.as_ptr(), &mut value),
            -1
        );
        assert_eq!(std::ffi::CStr::from_ptr(value.cast()).to_bytes(), b"value");
        assert_eq!(
            getopt::getsubopt(&mut next, tokens.as_ptr(), &mut value),
            -1
        );
        assert!(value.is_null());
    }
}
extern crate self as libbreenix;
#[path = "../libs/libbreenix-libc/src/dirent.rs"]
mod dirent;
pub mod syscall {
    pub mod nr {
        pub const GETDENTS64: u64 = 61;
    }
}
pub mod raw {
    pub static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    pub static MAX: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(128);
    pub static ERROR: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    pub static LONG_NAME: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    pub unsafe fn syscall3(_: u64, _: u64, buffer: u64, _: u64) -> u64 {
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if n > MAX.load(std::sync::atomic::Ordering::Relaxed) {
            return if ERROR.load(std::sync::atomic::Ordering::Relaxed) {
                (-5i64) as u64
            } else {
                0
            };
        }
        let long = LONG_NAME.load(std::sync::atomic::Ordering::Relaxed);
        let size = if long { 280 } else { 24 };
        let mut record = [0u8; 280];
        record[..8].copy_from_slice(&n.to_ne_bytes());
        record[8..16].copy_from_slice(&n.to_ne_bytes());
        record[16..18].copy_from_slice(&(size as u16).to_ne_bytes());
        record[18] = 8;
        record[19..if long { 274 } else { 20 }].fill(b'a');
        core::ptr::copy_nonoverlapping(record.as_ptr(), buffer as *mut u8, size);
        size as u64
    }
}
#[cfg(target_os = "macos")]
#[no_mangle]
unsafe extern "C" fn __errno_location() -> *mut i32 {
    extern "C" {
        fn __error() -> *mut i32;
    }
    __error()
}
#[test]
fn long_program_name_keeps_diagnostic() {
    unsafe {
        extern "C" {
            fn pipe(fds: *mut i32) -> i32;
            fn dup(fd: i32) -> i32;
            fn dup2(fd: i32, dest: i32) -> i32;
            fn read(fd: i32, buf: *mut u8, n: usize) -> isize;
            fn close(fd: i32) -> i32;
        }
        let mut fds = [0; 2];
        assert_eq!(pipe(fds.as_mut_ptr()), 0);
        let stderr = dup(2);
        assert_eq!(dup2(fds[1], 2), 2);
        let mut program = [b'p'; 1025];
        program[1024] = 0;
        let mut option = *b"-z\0";
        let argv = [
            program.as_mut_ptr(),
            option.as_mut_ptr(),
            core::ptr::null_mut(),
        ];
        getopt::optind = 0;
        getopt::opterr = 1;
        assert_eq!(
            getopt::getopt(2, argv.as_ptr(), b"x\0".as_ptr()),
            b'?' as i32
        );
        assert_eq!(dup2(stderr, 2), 2);
        close(stderr);
        close(fds[1]);
        let mut output = [0; 512];
        let n = read(fds[0], output.as_mut_ptr(), output.len());
        close(fds[0]);
        assert!(n > 0);
        assert!(output[..n as usize].ends_with(b": invalid option -- 'z'\n"));
    }
}

#[path = "../libs/libbreenix-libc/src/fnmatch.rs"]
mod fnmatch;
#[path = "../libs/libbreenix-libc/src/glob.rs"]
mod glob;
#[no_mangle]
unsafe extern "C" fn open(_: *const u8, _: i32, _: u32) -> i32 {
    99
}
#[no_mangle]
unsafe extern "C" fn lstat(path: *const u8, buf: *mut u8) -> i32 {
    if std::ffi::CStr::from_ptr(path.cast()).to_bytes() == b"a/b" {
        core::ptr::write_unaligned(buf.add(16).cast::<u32>(), 0o100000);
        0
    } else {
        *__errno_location() = 5;
        -1
    }
}
#[no_mangle]
unsafe extern "C" fn stat(path: *const u8, buf: *mut u8) -> i32 {
    lstat(path, buf)
}

mod regex_regressions {

    use crate::regex::*;
    fn matched(pattern: &str, text: &str, flags: i32) -> std::vec::Vec<(isize, isize)> {
        let p = std::ffi::CString::new(pattern).unwrap();
        let t = std::ffi::CString::new(text).unwrap();
        unsafe {
            let mut re: Regex = core::mem::zeroed();
            assert_eq!(regcomp(&mut re, p.as_ptr().cast(), flags), 0);
            let mut matches: std::vec::Vec<RegMatch> =
                (0..pattern.bytes().filter(|&b| b == b'(').count() + 1)
                    .map(|_| core::mem::zeroed::<RegMatch>())
                    .collect();
            assert_eq!(
                regexec(
                    &re,
                    t.as_ptr().cast(),
                    matches.len(),
                    matches.as_mut_ptr(),
                    0
                ),
                0
            );
            let result = matches
                .iter()
                .map(|m| {
                    let offsets =
                        core::slice::from_raw_parts((m as *const RegMatch).cast::<isize>(), 2);
                    (offsets[0], offsets[1])
                })
                .collect();
            regfree(&mut re);
            result
        }
    }
    #[test]
    fn capture_precedence() {
        assert_eq!(
            matched("(a|aa)(a?)", "aa", REG_EXTENDED),
            [(0, 2), (0, 2), (2, 2)]
        );
        assert_eq!(matched("(a*)*", "b", REG_EXTENDED), [(0, 0), (0, 0)]);
    }
    #[test]
    fn nested_capture_last_iteration() {
        assert_eq!(
            matched("(a(b)?)+", "aba", REG_EXTENDED),
            [(0, 3), (2, 3), (-1, -1)]
        );
    }
    #[test]
    fn minimal_repetitions() {
        assert_eq!(matched(".*?c", "abc abc", REG_EXTENDED)[0], (0, 3));
        assert_eq!(
            matched(".*c", "abc abc", REG_EXTENDED | REG_MINIMAL)[0],
            (0, 3)
        );
        assert_eq!(
            matched(".*?c", "abc abc", REG_EXTENDED | REG_MINIMAL)[0],
            (0, 7)
        );
        assert_eq!(matched("(.*?).*", "abcdef", REG_EXTENDED), [(0, 6), (0, 0)]);
        assert_eq!(matched("a{1,3}?", "aaa", REG_EXTENDED)[0], (0, 1));
        assert_eq!(matched("a??", "a", REG_EXTENDED)[0], (0, 0));
    }
}

mod dirent_regressions {

    use crate::dirent::*;
    use core::ptr::null_mut;
    use core::sync::atomic::Ordering;
    extern "C" {
        fn free(ptr: *mut u8);
    }
    unsafe fn set_errno(e: i32) {
        *crate::__errno_location() = e;
    }
    #[test]
    fn empty_sort_and_initialized_stream() {
        unsafe {
            sort_ptrs::<Dirent>(null_mut(), 0, |_, _| panic!("empty sort compared"));
            let d = opendir(b"mock\0".as_ptr());
            assert!(!d.is_null());
            crate::raw::NEXT.store(1, Ordering::Relaxed);
            let entry = readdir(d);
            assert!(!entry.is_null());
            assert_eq!((*entry).d_name[0], b'a');
            assert_eq!((&(*entry).d_name)[1..], [0; 255]);
            free(d.cast());
        }
    }
    #[test]
    fn scandir_filter_errno_does_not_turn_eof_into_error() {
        unsafe extern "C" fn filter(_: *const Dirent) -> i32 {
            set_errno(123);
            1
        }
        unsafe {
            crate::raw::NEXT.store(1, Ordering::Relaxed);
            crate::raw::MAX.store(1, Ordering::Relaxed);
            let mut list = null_mut();
            assert_eq!(
                scandir(b"mock\0".as_ptr(), &mut list, Some(filter), None),
                1
            );
            free((*list).cast());
            free(list.cast());
            crate::raw::MAX.store(128, Ordering::Relaxed);
        }
    }
    #[test]
    fn concurrent_readdir_r_serializes_stream() {
        crate::raw::NEXT.store(1, Ordering::Relaxed);
        let d = unsafe { opendir(b"mock\0".as_ptr()) } as usize;
        let mut threads = std::vec::Vec::new();
        for _ in 0..4 {
            threads.push(std::thread::spawn(move || {
                let mut seen = std::vec::Vec::new();
                loop {
                    let mut entry: Dirent = unsafe { core::mem::zeroed() };
                    let mut result = null_mut();
                    assert_eq!(
                        unsafe { readdir_r(d as *mut Dir, &mut entry, &mut result) },
                        0
                    );
                    if result.is_null() {
                        break;
                    }
                    seen.push(entry.d_ino);
                }
                seen
            }));
        }
        let mut seen: std::vec::Vec<_> = threads
            .into_iter()
            .flat_map(|t| t.join().unwrap())
            .collect();
        seen.sort_unstable();
        assert_eq!(seen, (1..=128).collect::<std::vec::Vec<_>>());
        unsafe {
            free(d as *mut u8);
        }
    }
}

mod glob_regressions {

    use crate::glob::{glob, globfree, Glob};
    const GLOB_ERR: i32 = 1;
    const GLOB_ABORTED: i32 = 2;
    #[repr(C)]
    struct View {
        gl_pathc: usize,
        gl_pathv: *mut *mut u8,
        gl_offs: usize,
        gl_flags: i32,
    }
    unsafe fn view(g: &Glob) -> &View {
        &*(g as *const Glob).cast::<View>()
    }
    use core::sync::atomic::Ordering;
    unsafe extern "C" fn error(path: *const u8, code: i32) -> i32 {
        assert_eq!(code, 5);
        assert_eq!(
            core::ffi::CStr::from_ptr(path.cast()).to_bytes(),
            b"prefix/"
        );
        1
    }
    unsafe extern "C" fn stat_error(path: *const u8, code: i32) -> i32 {
        assert_eq!(code, 5);
        assert_eq!(
            core::ffi::CStr::from_ptr(path.cast()).to_bytes(),
            b"inaccessible"
        );
        1
    }
    #[test]
    fn abort_preserves_scanned_paths_and_current_error_path() {
        unsafe {
            crate::raw::NEXT.store(1, Ordering::Relaxed);
            crate::raw::MAX.store(1, Ordering::Relaxed);
            crate::raw::ERROR.store(true, Ordering::Relaxed);
            crate::raw::LONG_NAME.store(true, Ordering::Relaxed);
            let mut result: Glob = core::mem::zeroed();
            assert_eq!(
                glob(b"prefix/*\0".as_ptr(), GLOB_ERR, Some(error), &mut result),
                GLOB_ABORTED
            );
            assert_eq!(view(&result).gl_pathc, 1);
            let path = core::ffi::CStr::from_ptr((*view(&result).gl_pathv).cast()).to_bytes();
            assert!(path.starts_with(b"prefix/"));
            assert_eq!(path.len(), 7 + 255);
            globfree(&mut result);
            crate::raw::MAX.store(128, Ordering::Relaxed);
            crate::raw::ERROR.store(false, Ordering::Relaxed);
            crate::raw::LONG_NAME.store(false, Ordering::Relaxed);
        }
    }
    #[test]
    fn final_stat_errors_and_quoted_slash() {
        unsafe {
            let mut result: Glob = core::mem::zeroed();
            assert_eq!(
                glob(
                    b"inaccessible\0".as_ptr(),
                    GLOB_ERR,
                    Some(stat_error),
                    &mut result
                ),
                GLOB_ABORTED
            );
            globfree(&mut result);
            assert_eq!(glob(b"a\\/b\0".as_ptr(), 0, None, &mut result), 0);
            assert_eq!(view(&result).gl_pathc, 1);
            assert_eq!(
                core::ffi::CStr::from_ptr((*view(&result).gl_pathv).cast()).to_bytes(),
                b"a/b"
            );
            globfree(&mut result);
        }
    }
}
