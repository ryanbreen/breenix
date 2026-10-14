//! A growable array of `Copy` values in malloc'd storage (the library has no `alloc`).

use core::ptr::null_mut;

extern "C" {
    fn realloc(ptr: *mut u8, size: usize) -> *mut u8;
    fn free(ptr: *mut u8);
}

pub(crate) struct Buf<T: Copy> {
    ptr: *mut T,
    len: usize,
    cap: usize,
}

impl<T: Copy> Buf<T> {
    pub(crate) const fn new() -> Self {
        Buf { ptr: null_mut(), len: 0, cap: 0 }
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Room for `extra` more elements; false when memory runs out.
    pub(crate) fn reserve(&mut self, extra: usize) -> bool {
        let Some(need) = self.len.checked_add(extra) else {
            return false;
        };
        if need <= self.cap {
            return true;
        }
        let cap = need.max(self.cap.saturating_mul(2)).max(8);
        let Some(bytes) = cap.checked_mul(core::mem::size_of::<T>()) else {
            return false;
        };
        let grown = unsafe { realloc(self.ptr as *mut u8, bytes) } as *mut T;
        if grown.is_null() {
            return false;
        }
        self.ptr = grown;
        self.cap = cap;
        true
    }

    pub(crate) fn push(&mut self, v: T) -> bool {
        if !self.reserve(1) {
            return false;
        }
        unsafe { self.ptr.add(self.len).write(v) };
        self.len += 1;
        true
    }

    /// Insert `v` at `at`, moving the elements after it up by one.
    pub(crate) fn insert(&mut self, at: usize, v: T) -> bool {
        if !self.reserve(1) {
            return false;
        }
        unsafe {
            core::ptr::copy(self.ptr.add(at), self.ptr.add(at + 1), self.len - at);
            self.ptr.add(at).write(v);
        }
        self.len += 1;
        true
    }

    /// Fill to `len` elements with `v`.
    pub(crate) fn resize(&mut self, len: usize, v: T) -> bool {
        if len > self.len && !self.reserve(len - self.len) {
            return false;
        }
        while self.len < len {
            unsafe { self.ptr.add(self.len).write(v) };
            self.len += 1;
        }
        self.len = len;
        true
    }

    pub(crate) fn truncate(&mut self, len: usize) {
        if len < self.len {
            self.len = len;
        }
    }

    pub(crate) fn as_slice(&self) -> &[T] {
        if self.ptr.is_null() {
            &[]
        } else {
            unsafe { core::slice::from_raw_parts(self.ptr, self.len) }
        }
    }

    pub(crate) fn as_mut_slice(&mut self) -> &mut [T] {
        if self.ptr.is_null() {
            &mut []
        } else {
            unsafe { core::slice::from_raw_parts_mut(self.ptr, self.len) }
        }
    }
}

impl<T: Copy> Drop for Buf<T> {
    fn drop(&mut self) {
        unsafe { free(self.ptr as *mut u8) };
    }
}
