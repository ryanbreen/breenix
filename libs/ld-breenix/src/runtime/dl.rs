//! POSIX.1-2024 dynamic load scope and the resident object registry.

use super::{
    fail, process, protect_relro, run_initializers, FileSet, Object, Scope, TlsModule,
    DEPENDENCIES, OBJECTS, PATH_MAX,
};
use crate::dl_abi::{DlInfo, DlRuntime, ObjectInfo};
use crate::elf::{Bytes, PT_LOAD};
use crate::InitState;
use core::cell::{OnceCell, UnsafeCell};
use core::ffi::{c_char, c_int, CStr};
use core::sync::atomic::{AtomicU64, Ordering};
use core::{ptr, slice};

const GLOBAL_HANDLE: usize = OBJECTS + 1;

#[derive(Clone, Copy)]
pub(super) struct Entry {
    pub object: Object<'static>,
    pub identity: (u64, u64),
    info: ObjectInfo,
    dependencies: [u8; OBJECTS],
    dependency_count: usize,
    references: usize,
    global: bool,
}

struct Registry {
    entries: [Entry; OBJECTS],
    count: usize,
    global_references: usize,
    interpreter_info: ObjectInfo,
    page: u64,
    modules: usize,
}

impl Registry {
    fn entries(&self) -> &[Entry] {
        &self.entries[..self.count]
    }

    fn position(&self, object: Object<'_>) -> Option<usize> {
        self.entries().iter().position(|entry| {
            entry.object.name() == object.name() && entry.object.bias() == object.bias()
        })
    }

    fn group(&self, root: usize) -> impl Iterator<Item = usize> {
        let mut ordered = [0; OBJECTS];
        let mut count = 1;
        ordered[0] = root;
        let mut next = 0;
        while next < count {
            let entry = self.entries[ordered[next]];
            for &dep in &entry.dependencies[..entry.dependency_count] {
                let dep = usize::from(dep);
                if !ordered[..count].contains(&dep) {
                    ordered[count] = dep;
                    count += 1;
                }
            }
            next += 1;
        }
        ordered.into_iter().take(count)
    }

    fn reference(&mut self, index: usize, global: bool) -> Option<usize> {
        let entry = &mut self.entries[index];
        entry.references = entry.references.checked_add(1)?;
        if global {
            for slot in self.group(index) {
                self.entries[slot].global = true;
            }
        }
        Some(index + 1)
    }

    fn symbol(&self, handle: usize, name: &[u8]) -> Option<u64> {
        if handle == 0 || (handle == GLOBAL_HANDLE && self.global_references != 0) {
            Object::lookup_scope(
                self.entries()
                    .iter()
                    .filter(|entry| entry.global)
                    .map(|entry| entry.object),
                name,
            )
        } else {
            Object::lookup_scope(
                self.group(self.object_handle(handle)?)
                    .map(|index| self.entries[index].object),
                name,
            )
        }
        .ok()
        .flatten()
    }

    fn object_handle(&self, handle: usize) -> Option<usize> {
        let slot = handle.checked_sub(1)?;
        (self.entries().get(slot)?.references != 0).then_some(slot)
    }

    fn close(&mut self, handle: usize) -> Option<()> {
        if handle == GLOBAL_HANDLE {
            self.global_references = self.global_references.checked_sub(1)?;
        } else {
            let slot = self.object_handle(handle)?;
            self.entries[slot].references -= 1;
        }
        Some(())
    }

    fn object(&self, index: usize) -> Option<ObjectInfo> {
        match index {
            1 => Some(self.interpreter_info),
            i => self
                .entries()
                .get(i - usize::from(i > 1))
                .map(|entry| entry.info),
        }
    }

    fn address(&self, address: u64) -> Option<DlInfo> {
        if let Some(info) = self
            .entries()
            .iter()
            .find_map(|entry| entry.object.address_info(address))
        {
            return Some(info);
        }
        let interpreter = self.interpreter_info.object;
        let offset = address.checked_sub(interpreter.bias)?;
        // SAFETY: the kernel-mapped interpreter's PHDR table is resident.
        let table =
            unsafe { slice::from_raw_parts(interpreter.phdr.cast(), interpreter.phnum * 56) };
        let inside = table.chunks_exact(56).any(|data| {
            let header = Bytes {
                name: b"",
                data,
                offset: 0,
            };
            header.u32(0).ok() == Some(PT_LOAD)
                && header
                    .u64(16)
                    .ok()
                    .and_then(|start| offset.checked_sub(start))
                    .zip(header.u64(40).ok())
                    .is_some_and(|(delta, size)| delta < size)
        });
        inside.then_some(DlInfo {
            dli_fname: interpreter.name,
            dli_fbase: interpreter.bias as *mut _,
            dli_sname: ptr::null(),
            dli_saddr: ptr::null_mut(),
        })
    }

    fn open(
        &mut self,
        name: Option<&[u8]>,
        global: bool,
        initializers: &mut InitQueue,
    ) -> Option<usize> {
        let Some(name) = name else {
            self.global_references = self.global_references.checked_add(1)?;
            return Some(GLOBAL_HANDLE);
        };
        if let Some(index) = self
            .entries()
            .iter()
            .position(|entry| entry.object.matches_name(name))
        {
            return self.reference(index, global);
        }
        let main = self.entries[0].object;
        let files = FileSet {
            main_path: main.name(),
            page: self.page,
            loaded: [const { OnceCell::new() }; OBJECTS - 1],
        };
        let mut path = [0; PATH_MAX];
        let root = Scope::open_name(main, name, &mut path, |path| {
            files.open(path, self.entries())
        })
        .ok()?;
        if let Some(index) = self.position(root) {
            return self.reference(index, global);
        }

        let mut objects = [root; OBJECTS];
        let mut edges = [Default::default(); DEPENDENCIES];
        let scope = Scope::load(root, &mut objects, &mut edges, &mut path, |path| {
            files.open(path, self.entries())
        })
        .ok()?;
        let mut slots = [0; OBJECTS];
        let mut next = self.count;
        for (slot, &object) in slots.iter_mut().zip(scope.objects()) {
            *slot = self.position(object).unwrap_or_else(|| {
                let slot = next;
                next += 1;
                slot
            });
        }
        if next > OBJECTS {
            return None;
        }

        let mut symbols = [main; OBJECTS];
        let mut symbol_count = 0;
        for (i, entry) in self.entries().iter().enumerate() {
            if entry.global || slots[..scope.objects().len()].contains(&i) {
                symbols[symbol_count] = entry.object;
                symbol_count += 1;
            }
        }
        for (&object, &slot) in scope.objects().iter().zip(&slots) {
            if slot >= self.count {
                symbols[symbol_count] = object;
                symbol_count += 1;
            }
        }

        let mut pending = [None; OBJECTS];
        let mut modules = [TlsModule::default(); OBJECTS];
        let mut module_count = 0;
        for (i, (&object, &slot)) in scope.objects().iter().zip(&slots).enumerate() {
            if slot < self.count {
                continue;
            }
            let module = if let Some(tls) = object.tls_module() {
                modules[module_count] = tls;
                module_count += 1;
                self.modules + module_count
            } else {
                0
            };
            let info = object.info(module).ok()?;
            let identity = files.identity(object)?;
            let (dependencies, dependency_count) = dependencies(&scope, i, &slots);
            object
                .relocate_with_module(&symbols[..symbol_count], module, |patch| {
                    // SAFETY: relocation bounds this store to a writable pending load.
                    unsafe { (patch.address as *mut u64).write_unaligned(patch.value.to_le()) };
                })
                .ok()?;
            if let Some(range) = object.relro() {
                protect_relro(range.start, range.end, self.page).ok()?;
            }
            pending[slot] = Some((object, info, identity, dependencies, dependency_count));
        }
        let mut state = [InitState::default(); OBJECTS];
        scope
            .initializers(&mut state, |array| {
                if slots[array.object] >= self.count {
                    initializers.push(array.range);
                }
            })
            .ok()?;
        if module_count != 0 {
            register_tls(self.entries(), &modules[..module_count])?;
        }

        let root_slot = self.count;
        for (index, entry) in pending.iter().enumerate().take(next).skip(self.count) {
            let (object, info, identity, dependencies, dependency_count) =
                entry.expect("a reserved slot has a prepared object");
            self.entries[index] = Entry {
                // SAFETY: the FileSet owns these names, file views and load
                // mappings and is kept resident below.
                object: unsafe { core::mem::transmute::<Object<'_>, Object<'static>>(object) },
                identity,
                info,
                dependencies,
                dependency_count,
                global: false,
                references: 0,
            };
        }
        self.count = next;
        self.modules += module_count;
        core::mem::forget(files);
        self.reference(root_slot, global)
    }
}

fn dependencies(scope: &Scope<'_, '_>, index: usize, slots: &[usize]) -> ([u8; OBJECTS], usize) {
    let mut dependencies = [0; OBJECTS];
    let mut count = 0;
    for dep in scope.dependencies(index).map(|dep| slots[dep] as u8) {
        if !dependencies[..count].contains(&dep) {
            dependencies[count] = dep;
            count += 1;
        }
    }
    (dependencies, count)
}

fn register_tls(entries: &[Entry], modules: &[TlsModule]) -> Option<()> {
    let libc = entries
        .iter()
        .find(|entry| entry.object.matches_name(b"libc.so"))?;
    let address = libc.object.lookup(b"__breenix_libc_register_tls").ok()??;
    // SAFETY: this export of the hosted libc implements the private TLS ABI.
    let register: unsafe extern "C" fn(*const TlsModule, usize) -> c_int =
        unsafe { core::mem::transmute(address as usize) };
    // SAFETY: the templates are valid until registration returns. On success
    // their mapped images are kept resident for any subsequent thread access.
    (unsafe { register(modules.as_ptr(), modules.len()) } == 0).then_some(())
}

struct State {
    owner: AtomicU64,
    registry: UnsafeCell<Option<Registry>>,
}

// SAFETY: startup initializes the registry on the initial thread. Thereafter
// only the thread owning the recursive loader lock accesses the cell.
unsafe impl Sync for State {}

static STATE: State = State {
    owner: AtomicU64::new(0),
    registry: UnsafeCell::new(None),
};

struct Guard {
    outer: bool,
}

impl Guard {
    fn acquire() -> Self {
        let tid = process::gettid().unwrap_or_else(|error| fail(error)).raw();
        loop {
            match STATE
                .owner
                .compare_exchange(0, tid, Ordering::Acquire, Ordering::Relaxed)
            {
                Ok(_) => return Self { outer: true },
                Err(owner) if tid == owner => return Self { outer: false },
                Err(_) => {
                    let _ = process::yield_now();
                }
            }
        }
    }

    fn with<R>(&self, apply: impl FnOnce(&mut Registry) -> R) -> R {
        // SAFETY: access is serialized by the loader lock. These closures do
        // not invoke guest constructors or reenter the registry.
        let state = unsafe { &mut *STATE.registry.get() };
        apply(state.as_mut().expect("loader API published after startup"))
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if self.outer {
            STATE.owner.store(0, Ordering::Release);
        }
    }
}

#[derive(Default)]
struct InitQueue {
    ranges: [Option<(u64, u64)>; OBJECTS],
    count: usize,
}

impl InitQueue {
    fn push(&mut self, range: core::ops::Range<u64>) {
        self.ranges[self.count] = Some((range.start, range.end));
        self.count += 1;
    }
}

unsafe extern "C" fn open(name: *const c_char, global: bool) -> usize {
    let name = if name.is_null() {
        None
    } else {
        // SAFETY: the dlopen caller supplies a NUL-terminated filename.
        Some(unsafe { CStr::from_ptr(name) }.to_bytes())
    };
    if name.is_some_and(|name| name.is_empty() || name.len() >= PATH_MAX) {
        return 0;
    }
    let mut initializers = InitQueue::default();
    let guard = Guard::acquire();
    let Some(handle) = guard.with(|registry| registry.open(name, global, &mut initializers)) else {
        return 0;
    };
    // Keep competing opens serialized through constructor completion. The
    // owning thread can reenter the loader without aliasing a registry borrow.
    for (start, end) in initializers.ranges.into_iter().flatten() {
        run_initializers(start..end);
    }
    handle
}

extern "C" fn close(handle: usize) -> c_int {
    Guard::acquire().with(|registry| registry.close(handle).map_or(-1, |()| 0))
}

unsafe extern "C" fn symbol(handle: usize, name: *const c_char, out: *mut u64) -> c_int {
    // SAFETY: libc forwards a NUL-terminated symbol name.
    let name = unsafe { CStr::from_ptr(name) }.to_bytes();
    let Some(address) = Guard::acquire().with(|registry| registry.symbol(handle, name)) else {
        return -1;
    };
    // SAFETY: libc supplies a writable output word.
    unsafe { out.write(address) };
    0
}

unsafe extern "C" fn address(address: u64, out: *mut DlInfo) -> c_int {
    let Some(info) = Guard::acquire().with(|registry| registry.address(address)) else {
        return 0;
    };
    // SAFETY: libc supplies a writable Dl_info.
    unsafe { out.write(info) };
    1
}

extern "C" fn count() -> usize {
    Guard::acquire().with(|registry| registry.count + 1)
}

unsafe extern "C" fn object(index: usize, out: *mut ObjectInfo) -> bool {
    let Some(info) = Guard::acquire().with(|registry| registry.object(index)) else {
        return false;
    };
    // SAFETY: libc supplies a writable ObjectInfo.
    unsafe { out.write(info) };
    true
}

static RUNTIME: DlRuntime = DlRuntime {
    open,
    close,
    symbol,
    address,
    count,
    object,
};

/// The initial file views, C names and runtime mappings stay resident.
pub(super) unsafe fn publish_initial(
    scope: &Scope<'_, '_>,
    files: &FileSet<'_>,
    root_identity: (u64, u64),
    page: u64,
    interpreter_info: ObjectInfo,
) -> &'static DlRuntime {
    let mut slots = [0; OBJECTS];
    for (i, slot) in slots.iter_mut().enumerate() {
        *slot = i;
    }
    let mut modules = 0;
    let mut entry = |index, object: Object<'_>| {
        let module = if object.tls_module().is_some() {
            modules += 1;
            modules
        } else {
            0
        };
        let identity = if index == 0 {
            root_identity
        } else {
            files
                .identity(object)
                .unwrap_or_else(|| fail("missing initial DSO identity"))
        };
        let info = object.info(module).unwrap_or_else(|error| fail(error));
        let (dependencies, dependency_count) = dependencies(scope, index, &slots);
        Entry {
            // SAFETY: the caller keeps the maps referenced by this object resident.
            object: unsafe { core::mem::transmute::<Object<'_>, Object<'static>>(object) },
            identity,
            info,
            dependencies,
            dependency_count,
            references: 0,
            global: true,
        }
    };
    let root = entry(0, scope.objects()[0]);
    let mut entries = [root; OBJECTS];
    for (index, &object) in scope.objects().iter().enumerate().skip(1) {
        entries[index] = entry(index, object);
    }

    let registry = Registry {
        entries,
        count: scope.objects().len(),
        global_references: 0,
        interpreter_info,
        modules,
        page,
    };
    // SAFETY: this is the initial thread before the ABI table is published.
    unsafe { STATE.registry.get().write(Some(registry)) };
    &RUNTIME
}
