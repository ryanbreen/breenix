//! Initial-load TLS templates for the hosted AArch64 shared libc.

#![deny(unsafe_op_in_unsafe_fn)]

use super::{descriptor_from_tp, RUNTIME_READY};
use crate::tls_module::{TlsModule, MAX_TLS_MODULES};
use crate::tls_variant_i::StaticTls;
use core::ffi::{c_void, CStr};
use core::mem::MaybeUninit;
use core::ptr;
use core::sync::atomic::{AtomicUsize, Ordering::{Acquire, Release}};

#[derive(Clone, Copy)]
struct Module {
    template: TlsModule,
    offset: usize,
}

static mut MODULES: [MaybeUninit<Module>; MAX_TLS_MODULES] =
    [MaybeUninit::uninit(); MAX_TLS_MODULES];
static mut LAYOUT: StaticTls = StaticTls::new();
static COUNT: AtomicUsize = AtomicUsize::new(0);

/// Register the TLS modules in initial load order before allocating any thread.
#[no_mangle]
pub unsafe extern "C" fn __breenix_libc_register_tls(
    modules: *const TlsModule,
    count: usize,
) -> i32 {
    if modules.is_null() || !(1..=MAX_TLS_MODULES).contains(&count)
        || COUNT.load(Acquire) != 0 || RUNTIME_READY.load(Acquire)
    {
        return crate::EINVAL;
    }
    // SAFETY: the loader supplies count initialized templates.
    let templates = unsafe { core::slice::from_raw_parts(modules, count) };
    let mut layout = StaticTls::new();
    let table = ptr::addr_of_mut!(MODULES).cast::<Module>();
    for (index, &template) in templates.iter().enumerate() {
        if template.file_size > template.mem_size
            || (template.file_size != 0 && template.image.is_null())
        {
            return crate::EINVAL;
        }
        let Some(offset) = layout.push(template) else {
            return crate::EINVAL;
        };
        // SAFETY: this initial registration is the sole writer; count fits the table.
        let slot = unsafe { table.add(index) };
        // SAFETY: slot is writable and is still unpublished.
        unsafe { slot.write(Module { template, offset }) };
    }
    // SAFETY: the sole registration runs before any threads exist.
    unsafe { LAYOUT = layout };
    COUNT.store(count, Release);
    0
}

pub(super) fn layout() -> Option<StaticTls> {
    if COUNT.load(Acquire) == 0 {
        return None;
    }
    // SAFETY: the initial registration published this immutable layout.
    Some(unsafe { LAYOUT })
}

fn modules() -> &'static [Module] {
    let count = COUNT.load(Acquire);
    // SAFETY: the interpreter published exactly count immutable entries.
    unsafe { core::slice::from_raw_parts(ptr::addr_of!(MODULES).cast(), count) }
}

/// The calling thread's control area is writable and starts zero-filled.
pub(super) unsafe fn initialize(tp: *mut u8) {
    for module in modules() {
        let template = module.template;
        if template.file_size != 0 {
            // SAFETY: the shared layout placed this template within the control area.
            let destination = unsafe { tp.add(module.offset) };
            // SAFETY: the relocated tdata mapping is readable and disjoint from the thread.
            unsafe { ptr::copy_nonoverlapping(template.image, destination, template.file_size) };
        }
    }
}

pub(crate) fn tls_data(module: usize) -> *mut c_void {
    let Some(template) = module.checked_sub(1).and_then(|index| modules().get(index)) else {
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
    // SAFETY: the live descriptor holds this thread's ABI thread pointer.
    let tp = unsafe { (*thread).tp };
    (tp + template.offset) as *mut c_void
}

pub(crate) fn replace_dl_error(message: Option<&'static CStr>) -> Option<&'static CStr> {
    if !RUNTIME_READY.load(Acquire) {
        return None;
    }
    // SAFETY: each initialized thread has its own libc descriptor.
    let thread = unsafe { descriptor_from_tp() };
    if thread.is_null() {
        return None;
    }
    // SAFETY: only the current thread accesses its error cell.
    unsafe { (*thread).dl_error.replace(message) }
}
