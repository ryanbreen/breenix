#![no_builtins]

#[allow(dead_code)] // The host check calls only the four requested entry points.
mod mem_asm;
#[allow(dead_code)]
mod words;

use core::ffi::c_void;
use core::ptr::null_mut;

unsafe extern "C" {
    fn getpagesize() -> i32;
    fn mmap(p: *mut c_void, n: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut c_void;
    fn mprotect(p: *mut c_void, n: usize, prot: i32) -> i32;
    fn munmap(p: *mut c_void, n: usize) -> i32;
}

struct Guarded {
    base: *mut u8,
    page: usize,
}

impl Guarded {
    fn new() -> Self {
        let page = unsafe { getpagesize() as usize };
        let anonymous = if cfg!(target_os = "macos") {
            0x1000
        } else {
            0x20
        };
        let base = unsafe { mmap(null_mut(), 3 * page, 0, 2 | anonymous, -1, 0) }.cast::<u8>();
        assert_ne!(base.addr(), usize::MAX);
        assert_eq!(unsafe { mprotect(base.add(page).cast(), page, 3) }, 0);
        Self { base, page }
    }

    fn tail(&self, n: usize, gap: usize) -> *mut u8 {
        unsafe { self.base.add(2 * self.page - n - gap) }
    }
}

impl Drop for Guarded {
    fn drop(&mut self) {
        assert_eq!(unsafe { munmap(self.base.cast(), 3 * self.page) }, 0);
    }
}

#[test]
fn words_match_the_reference_at_guard_pages() {
    let left = Guarded::new();
    let right = Guarded::new();
    let mut state = 0x3e4b_f853_u32;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state as u8
    };
    for n in (0..520).chain([1023, 1024, 2047, 4096]) {
        for gap in 0..8 {
            let a = left.tail(n, 0);
            let b = right.tail(n, gap.min(right.page - n));
            let input: Vec<u8> = (0..n).map(|_| next()).collect();
            unsafe {
                for (i, &byte) in input.iter().enumerate() {
                    a.add(i).write(byte);
                }
                let preceding = n != 0 && b.addr() > right.base.addr() + right.page;
                if preceding {
                    b.sub(1).write(0xab);
                }
                assert_eq!(mem_asm::memcpy(b, a, n), b);
                assert!((0..n).all(|i| b.add(i).read() == input[i]));
                if preceding {
                    assert_eq!(b.sub(1).read(), 0xab);
                }
                assert_eq!(words::memcmp(a, b, n), 0);
                if n != 0 {
                    let i = usize::from(next()) * n / 256;
                    let changed = input[i] ^ 0x80;
                    b.add(i).write(changed);
                    assert_eq!(
                        words::memcmp(a, b, n),
                        i32::from(input[i]) - i32::from(changed)
                    );
                    assert_eq!(
                        words::memcmp(b, a, n),
                        i32::from(changed) - i32::from(input[i])
                    );
                    for j in 0..n - 1 {
                        a.add(j).write(input[j] | 1);
                    }
                    a.add(n - 1).write(0);
                    assert_eq!(words::strlen(a), n - 1);
                }
                let needle = next();
                for needle in [0, needle] {
                    let expected = (0..n)
                        .find(|&i| a.add(i).read() == needle)
                        .map_or(null_mut(), |i| a.add(i));
                    assert_eq!(words::memchr(a, i32::from(needle) + 256, n), expected);
                }
            }
        }
    }
}

#[test]
fn memchr_stops_at_a_match_before_the_guard_page() {
    let mapping = Guarded::new();
    unsafe {
        let last = mapping.tail(1, 0);
        last.write(0x81);
        assert_eq!(words::memchr(last, 0x81, usize::MAX), last);
    }
}
