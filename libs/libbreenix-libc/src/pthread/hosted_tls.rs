//! AMD64 psABI §§4.4.1 and 10.3 local-dynamic TLS for the hosted compiler.

use crate::stdlib::malloc::{free, posix_memalign};
use crate::tls_module::{TlsModule, MAX_TLS_MODULES};
use core::ptr::{self, null_mut};

/// The LP64 pair addressed by the local-dynamic GOT reference.
#[repr(C)]
pub(super) struct TlsIndex {
    pub module: usize,
    pub offset: usize,
}

pub(super) struct Dtv {
    blocks: [*mut u8; MAX_TLS_MODULES],
}

impl Dtv {
    pub const fn new() -> Self {
        Self {
            blocks: [null_mut(); MAX_TLS_MODULES],
        }
    }

    /// Each template's initialized bytes remain readable for the process's life.
    pub unsafe fn get_addr(&mut self, modules: &[TlsModule], index: &TlsIndex) -> *mut u8 {
        let Some(slot) = index.module.checked_sub(1) else {
            return null_mut();
        };
        let Some((module, block)) = modules.get(slot).zip(self.blocks.get_mut(slot)) else {
            return null_mut();
        };
        if index.offset >= module.mem_size {
            return null_mut();
        }
        if block.is_null() {
            let align = module.align.max(core::mem::size_of::<usize>());
            // SAFETY: block is a writable pointer slot for the allocator.
            if unsafe { posix_memalign(block, align, module.mem_size) } != 0 {
                return null_mut();
            }
            if module.file_size != 0 {
                // SAFETY: the relocated template is disjoint from this new block.
                unsafe { ptr::copy_nonoverlapping(module.image, *block, module.file_size) };
            }
            // SAFETY: file_size <= mem_size, and the allocation holds mem_size bytes.
            let tbss = unsafe { block.add(module.file_size) };
            // SAFETY: the remainder of this allocation belongs to tbss.
            unsafe { tbss.write_bytes(0, module.mem_size - module.file_size) };
        }
        // SAFETY: offset is in bounds for the thread's live TLS allocation.
        unsafe { block.add(index.offset) }
    }
}

impl Drop for Dtv {
    fn drop(&mut self) {
        for &block in self.blocks.iter().filter(|block| !block.is_null()) {
            // SAFETY: this non-null slot owns its posix_memalign allocation.
            unsafe { free(block) };
        }
    }
}

#[cfg(not(test))]
pub(crate) use runtime::{replace_dl_error, tls_data};

#[cfg(not(test))]
mod runtime {
    use super::super::{descriptor_from_tp, RUNTIME_READY};
    use super::{TlsIndex, TlsModule, MAX_TLS_MODULES};
    use core::ffi::{c_void, CStr};
    use core::mem::MaybeUninit;
    use core::ptr;
    use core::sync::atomic::{
        AtomicUsize,
        Ordering::{Acquire, Release},
    };

    static mut MODULES: [MaybeUninit<TlsModule>; MAX_TLS_MODULES] =
        [MaybeUninit::uninit(); MAX_TLS_MODULES];
    static MODULE_COUNT: AtomicUsize = AtomicUsize::new(0);

    /// The interpreter appends templates in TLS module ID order, before
    /// publishing the corresponding objects under the loader lock.
    #[no_mangle]
    pub unsafe extern "C" fn __breenix_libc_register_tls(
        modules: *const TlsModule,
        count: usize,
    ) -> i32 {
        let start = MODULE_COUNT.load(Acquire);
        if count == 0 || count > MAX_TLS_MODULES - start || modules.is_null() {
            return crate::EINVAL;
        }
        // SAFETY: the interpreter supplies count initialized records.
        let templates = unsafe { core::slice::from_raw_parts(modules, count) };
        if templates.iter().any(|module| {
            module.file_size > module.mem_size
                || module.file_size > isize::MAX as usize
                || (module.file_size != 0 && module.image.is_null())
                || !module.align.is_power_of_two()
        }) {
            return crate::EINVAL;
        }
        // SAFETY: count fits in the uninitialized suffix after start.
        let table = unsafe { ptr::addr_of_mut!(MODULES).cast::<TlsModule>().add(start) };
        // SAFETY: the interpreter is the sole writer to this unpublished suffix.
        unsafe { ptr::copy_nonoverlapping(modules, table, count) };
        MODULE_COUNT.store(start + count, Release);
        0
    }

    pub(crate) fn tls_data(module: usize) -> *mut c_void {
        let Some(index) = module.checked_sub(1) else {
            return ptr::null_mut();
        };
        if !RUNTIME_READY.load(Acquire) {
            return ptr::null_mut();
        }
        // SAFETY: each initialized thread has its own libc descriptor.
        let thread = unsafe { descriptor_from_tp() };
        if thread.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: only this thread accesses its DTV slots.
        let dtv = unsafe { &(*thread).dtv };
        dtv.blocks
            .get(index)
            .copied()
            .unwrap_or(ptr::null_mut())
            .cast()
    }

    pub(crate) fn replace_dl_error(message: Option<&'static CStr>) -> Option<&'static CStr> {
        if !RUNTIME_READY.load(Acquire) {
            return None;
        }
        // SAFETY: the thread owns its descriptor and its error slot.
        let thread = unsafe { descriptor_from_tp() };
        if thread.is_null() {
            return None;
        }
        // SAFETY: only this thread accesses the error cell.
        let error = unsafe { &(*thread).dl_error };
        error.replace(message)
    }

    #[no_mangle]
    pub unsafe extern "C" fn __tls_get_addr(index: *const TlsIndex) -> *mut u8 {
        let count = MODULE_COUNT.load(Acquire);
        if index.is_null() || count == 0 || !RUNTIME_READY.load(Acquire) {
            crate::exit_group(127);
        }
        // SAFETY: initialized by the interpreter before any TLS accesses and immutable since.
        let modules = unsafe { core::slice::from_raw_parts(ptr::addr_of!(MODULES).cast(), count) };
        // SAFETY: libc startup and pthread creation install this thread's control block.
        let thread = unsafe { descriptor_from_tp() };
        if thread.is_null() {
            crate::exit_group(127);
        }
        // SAFETY: the ABI caller supplies a readable pair.
        let index = unsafe { &*index };
        // SAFETY: the current thread owns its DTV.
        let dtv = unsafe { &mut (*thread).dtv };
        // SAFETY: the registered images remain mapped for the process lifetime.
        let address = unsafe { dtv.get_addr(modules, index) };
        if address.is_null() {
            crate::exit_group(127);
        }
        address
    }
}
