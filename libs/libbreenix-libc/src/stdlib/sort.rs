//! qsort, qsort_r and bsearch (ISO C17 7.22.5, POSIX.1-2024).
//!
//! qsort is an introsort: quicksort with a median-of-three pivot, insertion sort for short
//! runs, and heapsort once the recursion depth passes 2 log2 n, so the worst case stays
//! O(n log n) without allocating.

use core::ffi::c_void;

type Compare = unsafe extern "C" fn(*const c_void, *const c_void) -> i32;
type CompareR = unsafe extern "C" fn(*const c_void, *const c_void, *mut c_void) -> i32;

struct Array<'a> {
    base: *mut u8,
    width: usize,
    cmp: &'a mut dyn FnMut(*const u8, *const u8) -> i32,
}

impl Array<'_> {
    unsafe fn at(&self, i: usize) -> *mut u8 {
        self.base.add(i * self.width)
    }

    unsafe fn less(&mut self, i: usize, j: usize) -> bool {
        let (a, b) = (self.at(i), self.at(j));
        (self.cmp)(a, b) < 0
    }

    unsafe fn swap(&self, i: usize, j: usize) {
        if i == j {
            return;
        }
        let (mut a, mut b) = (self.at(i), self.at(j));
        let mut n = self.width;
        while n >= 8 {
            let t = (a as *const u64).read_unaligned();
            (a as *mut u64).write_unaligned((b as *const u64).read_unaligned());
            (b as *mut u64).write_unaligned(t);
            a = a.add(8);
            b = b.add(8);
            n -= 8;
        }
        while n > 0 {
            core::ptr::swap(a, b);
            a = a.add(1);
            b = b.add(1);
            n -= 1;
        }
    }

    /// Sorts elements [lo, lo + n).
    unsafe fn sort(&mut self, mut lo: usize, mut n: usize, mut depth: u32) {
        loop {
            if n <= 12 {
                self.insertion(lo, n);
                return;
            }
            if depth == 0 {
                self.heapsort(lo, n);
                return;
            }
            depth -= 1;
            let (first, mid, last) = (lo, lo + n / 2, lo + n - 1);
            if self.less(mid, first) {
                self.swap(mid, first);
            }
            if self.less(last, mid) {
                self.swap(last, mid);
                if self.less(mid, first) {
                    self.swap(mid, first);
                }
            }
            // The median becomes the pivot at lo.
            self.swap(lo, mid);
            let (mut i, mut j) = (lo + 1, lo + n - 1);
            loop {
                while i <= j && self.less(i, lo) {
                    i += 1;
                }
                while i <= j && self.less(lo, j) {
                    j -= 1;
                }
                if i >= j {
                    break;
                }
                self.swap(i, j);
                i += 1;
                j -= 1;
            }
            self.swap(lo, j);
            // Recurse into the smaller side and loop on the larger one.
            let (left, right) = (j - lo, lo + n - (j + 1));
            if left < right {
                self.sort(lo, left, depth);
                lo = j + 1;
                n = right;
            } else {
                self.sort(j + 1, right, depth);
                n = left;
            }
        }
    }

    unsafe fn insertion(&mut self, lo: usize, n: usize) {
        for i in 1..n {
            let mut j = lo + i;
            while j > lo && self.less(j, j - 1) {
                self.swap(j, j - 1);
                j -= 1;
            }
        }
    }

    unsafe fn heapsort(&mut self, lo: usize, n: usize) {
        for start in (0..n / 2).rev() {
            self.sift(lo, start, n);
        }
        for end in (1..n).rev() {
            self.swap(lo, lo + end);
            self.sift(lo, 0, end);
        }
    }

    unsafe fn sift(&mut self, lo: usize, mut root: usize, n: usize) {
        loop {
            let mut child = 2 * root + 1;
            if child >= n {
                return;
            }
            if child + 1 < n && self.less(lo + child, lo + child + 1) {
                child += 1;
            }
            if !self.less(lo + root, lo + child) {
                return;
            }
            self.swap(lo + root, lo + child);
            root = child;
        }
    }
}

unsafe fn sort_with(base: *mut u8, n: usize, width: usize, cmp: &mut dyn FnMut(*const u8, *const u8) -> i32) {
    if n < 2 || width == 0 {
        return;
    }
    let depth = 2 * (usize::BITS - n.leading_zeros());
    Array { base, width, cmp }.sort(0, n, depth);
}

#[no_mangle]
pub unsafe extern "C" fn qsort(base: *mut c_void, n: usize, width: usize, cmp: Compare) {
    sort_with(base as *mut u8, n, width, &mut |a, b| cmp(a as *const c_void, b as *const c_void));
}

#[no_mangle]
pub unsafe extern "C" fn qsort_r(base: *mut c_void, n: usize, width: usize, cmp: CompareR, arg: *mut c_void) {
    sort_with(base as *mut u8, n, width, &mut |a, b| cmp(a as *const c_void, b as *const c_void, arg));
}

#[no_mangle]
pub unsafe extern "C" fn bsearch(
    key: *const c_void,
    base: *const c_void,
    n: usize,
    width: usize,
    cmp: Compare,
) -> *mut c_void {
    let (mut lo, mut hi) = (0, n);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let elem = (base as *const u8).add(mid * width) as *const c_void;
        match cmp(key, elem) {
            c if c < 0 => hi = mid,
            c if c > 0 => lo = mid + 1,
            _ => return elem as *mut c_void,
        }
    }
    core::ptr::null_mut()
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn by_int(a: *const c_void, b: *const c_void) -> i32 {
        let (x, y) = (*(a as *const i32), *(b as *const i32));
        (x > y) as i32 - (x < y) as i32
    }

    #[test]
    fn sorts_every_shape() {
        let mut seed = 99u32;
        for n in [0usize, 1, 2, 3, 13, 100, 1000, 5000] {
            for shape in 0..4 {
                let mut v: [i32; 5000] = [0; 5000];
                for i in 0..n {
                    seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                    v[i] = match shape {
                        0 => (seed >> 8) as i32 % 1000,
                        1 => i as i32,
                        2 => (n - i) as i32,
                        _ => 7,
                    };
                }
                unsafe { qsort(v.as_mut_ptr() as *mut c_void, n, 4, by_int) };
                assert!(v[..n].windows(2).all(|w| w[0] <= w[1]), "n={n} shape={shape}");
            }
        }
    }
}
