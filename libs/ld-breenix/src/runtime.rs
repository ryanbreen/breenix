//! Map the AMD64 initial load set using the Breenix syscall ABI and ELF gABI §§7–8.

mod dl;

use crate::dl_abi::{LoadedObject, ObjectInfo};
use crate::elf::{Bytes, Image, Segment, PF_R, PF_W, PT_DYNAMIC, PT_GNU_RELRO, PT_LOAD};
use crate::tls_module::{TlsModule, MAX_TLS_MODULES};
use crate::{Dependency, Error, ErrorKind, InitState, Object, Scope};
use core::cell::OnceCell;
use core::fmt::{self, Write};
use core::{ptr, slice};
use libbreenix::errno::Errno;
use libbreenix::error::Error as SysError;
use libbreenix::memory::{
    mmap, mprotect, munmap, MAP_ANONYMOUS, MAP_FIXED, MAP_PRIVATE, PROT_EXEC, PROT_NONE, PROT_READ,
    PROT_WRITE,
};
use libbreenix::syscall::{nr, raw};
use libbreenix::{fs, io, process, Fd};

const PATH_MAX: usize = 4096;
const OBJECTS: usize = MAX_TLS_MODULES;
const DEPENDENCIES: usize = 128;
const PF_X: u32 = 1;
const AT_BREENIX_LOAD_BIAS: usize = 0x6000_0000;

struct Auxv {
    entry: u64,
    page: u64,
    base: u64,
    phdr: usize,
    phnum: usize,
    program_bias: u64,
}

impl Auxv {
    unsafe fn read(stack: *const usize) -> Result<Self, &'static str> {
        // SAFETY: the kernel's original stack begins with argc.
        let argc = unsafe { stack.read() };
        // SAFETY: envp follows argc, argc argv entries and their terminating null.
        let mut cursor = unsafe { stack.add(argc + 2) };
        // SAFETY: the kernel's envp is null-terminated.
        while unsafe { cursor.read() } != 0 {
            // SAFETY: the kernel's environment terminator follows the entries.
            cursor = unsafe { cursor.add(1) };
        }
        // SAFETY: the auxv follows the envp terminator.
        cursor = unsafe { cursor.add(1) };
        let mut auxv = Self {
            entry: 0,
            page: 0,
            base: 0,
            phdr: 0,
            phnum: 0,
            program_bias: 0,
        };
        let mut phent = 0;
        let mut program_bias = None;
        // SAFETY: the kernel's auxv is AT_NULL-terminated.
        while unsafe { cursor.read() } != 0 {
            // SAFETY: each non-null tag begins a two-word entry.
            let pair = unsafe { cursor.cast::<[usize; 2]>().read() };
            match pair {
                [3, phdr] => auxv.phdr = phdr,          // AT_PHDR
                [4, size] => phent = size,              // AT_PHENT
                [5, count] => auxv.phnum = count,      // AT_PHNUM
                [6, page] => auxv.page = page as u64,    // AT_PAGESZ
                [7, base] => auxv.base = base as u64,    // AT_BASE
                [9, entry] => auxv.entry = entry as u64, // AT_ENTRY
                [23, secure] if secure != 0 => {
                    return Err("secure exec requires a trusted library search")
                }
                [AT_BREENIX_LOAD_BIAS, bias] => program_bias = Some(bias as u64),
                _ => {}
            }
            // SAFETY: a non-null pair is followed by another entry.
            cursor = unsafe { cursor.add(2) };
        }
        if auxv.entry == 0 || auxv.base == 0 || !auxv.page.is_power_of_two()
            || auxv.phdr == 0 || phent != 56 || !(1..=0xfffe).contains(&auxv.phnum)
        {
            return Err("invalid exec auxiliary vector");
        }
        auxv.program_bias = program_bias.ok_or("missing executable load bias")?;
        Ok(auxv)
    }
}

struct Mapping {
    address: *mut u8,
    len: usize,
}

impl Mapping {
    fn anonymous(len: usize, prot: i32) -> Result<Self, SysError> {
        let address = mmap(
            ptr::null_mut(),
            len,
            prot,
            MAP_PRIVATE | MAP_ANONYMOUS,
            -1,
            0,
        )?;
        Ok(Self { address, len })
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: the read-only file view or initialized name remains mapped for this borrow.
        unsafe { slice::from_raw_parts(self.address, self.len) }
    }

    fn name(path: &[u8]) -> Result<Self, SysError> {
        let mapping = Self::anonymous(path.len() + 1, PROT_READ | PROT_WRITE)?;
        // SAFETY: a new anonymous mapping is zero-filled and has room for the path and NUL.
        unsafe { ptr::copy_nonoverlapping(path.as_ptr(), mapping.address, path.len()) };
        Ok(mapping)
    }

    fn path(&self) -> &[u8] {
        &self.bytes()[..self.len - 1]
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        let _ = munmap(self.address, self.len);
    }
}

fn snapshot(bytes: &[u8]) -> Option<&'static [u8]> {
    if bytes.is_empty() {
        return Some(&[]);
    }
    let mapping = Mapping::anonymous(bytes.len(), PROT_READ | PROT_WRITE).ok()?;
    // SAFETY: this private mapping is separate from the executable, survives
    // process startup, and is exposed only as immutable metadata.
    let copy = unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), mapping.address, bytes.len());
        slice::from_raw_parts(mapping.address, mapping.len)
    };
    core::mem::forget(mapping);
    Some(copy)
}

struct OpenFile(Fd);

impl OpenFile {
    fn open(path: &[u8]) -> Result<Self, SysError> {
        if path.len() >= PATH_MAX || path.contains(&0) {
            return Err(Errno::EINVAL.into());
        }
        let mut c_path = [0u8; PATH_MAX];
        c_path[..path.len()].copy_from_slice(path);
        // SAFETY: c_path is a readable NUL-terminated string during open.
        let result =
            unsafe { raw::syscall3(nr::OPEN, c_path.as_ptr() as u64, fs::O_NONBLOCK as u64, 0) };
        SysError::from_syscall(result as i64).map(|fd| Self(Fd::from_raw(fd)))
    }

    fn view(&self, stat: fs::Stat) -> Result<Mapping, SysError> {
        if stat.st_mode & fs::S_IFMT != fs::S_IFREG || stat.st_size <= 0 {
            return Err(Errno::ENOEXEC.into());
        }
        let len = stat.st_size as usize;
        let address = mmap(
            ptr::null_mut(),
            len,
            PROT_READ,
            MAP_PRIVATE,
            self.0.raw() as i32,
            0,
        )?;
        Ok(Mapping { address, len })
    }
}

impl Drop for OpenFile {
    fn drop(&mut self) {
        let _ = io::close(self.0);
    }
}

struct DependencyFile {
    identity: (u64, u64),
    name: Mapping,
    view: Mapping,
    bias: u64,
    _loads: Mapping,
}

impl DependencyFile {
    fn object(&self) -> Result<Object<'_>, Error<'_>> {
        Object::parse(self.name.path(), self.view.bytes(), self.bias)
    }
}

struct FileSet<'a> {
    main_path: &'a [u8],
    page: u64,
    loaded: [OnceCell<DependencyFile>; OBJECTS - 1],
}

impl FileSet<'_> {
    fn open(&self, path: &[u8], resident: &[dl::Entry]) -> Result<Option<Object<'_>>, Error<'_>> {
        let main = Bytes {
            name: self.main_path,
            data: b"",
            offset: 0,
        };
        if let Some(entry) = resident
            .iter()
            .find(|entry| entry.object.matches_name(path))
        {
            return Ok(Some(entry.object));
        }
        let fd = match OpenFile::open(path) {
            Ok(fd) => fd,
            Err(SysError::Os(Errno::ENOENT | Errno::ENOTDIR)) => return Ok(None),
            Err(error) => return Err(syscall(main, "open", error)),
        };
        let stat = fs::fstat(fd.0).map_err(|error| syscall(main, "fstat", error))?;
        let identity = (stat.st_dev, stat.st_ino);
        if let Some(entry) = resident.iter().find(|entry| entry.identity == Some(identity)) {
            return Ok(Some(entry.object));
        }
        if let Some(existing) = self
            .loaded
            .iter()
            .filter_map(OnceCell::get)
            .find(|object| object.identity == identity)
        {
            return existing.object().map(Some);
        }
        let slot = self
            .loaded
            .iter()
            .find(|slot| slot.get().is_none())
            .ok_or_else(|| main.error(ErrorKind::Capacity))?;
        let view = fd
            .view(stat)
            .map_err(|error| syscall(main, "mmap file view", error))?;
        // The search buffer changes on each candidate. Bind early errors to
        // the stable program name until the dependency name has its own map.
        let file = Bytes {
            data: view.bytes(),
            ..main
        };
        let stable = |error: Error<'_>| Error {
            object: self.main_path,
            offset: error.offset,
            kind: error.kind,
            detail: b"",
        };
        let image = match Image::parse(file, 0) {
            Ok(image) if image.shared => image,
            Ok(_)
            | Err(Error {
                kind: ErrorKind::UnsupportedElf,
                ..
            }) => return Ok(None),
            Err(error) => return Err(stable(error)),
        };
        let mut absolute = [0u8; PATH_MAX];
        let path = if path.starts_with(b"/") {
            path
        } else {
            let len =
                process::getcwd(&mut absolute).map_err(|error| syscall(main, "getcwd", error))?;
            let end = len
                .checked_add(path.len())
                .filter(|&end| end < PATH_MAX)
                .ok_or_else(|| main.error(ErrorKind::Capacity))?;
            let slash = len
                .checked_sub(1)
                .ok_or_else(|| main.error(ErrorKind::InvalidDependency))?;
            absolute[slash] = b'/';
            absolute[len..end].copy_from_slice(path);
            &absolute[..end]
        };
        let name = Mapping::name(path).map_err(|error| syscall(main, "mmap object name", error))?;
        let (bias, loads) = map_loads(image, &fd, self.page).map_err(stable)?;
        let object = slot.get_or_init(|| DependencyFile {
            identity,
            name,
            view,
            bias,
            _loads: loads,
        });
        object.object().map(Some)
    }

    fn identity(&self, object: Object<'_>) -> Option<(u64, u64)> {
        self.loaded
            .iter()
            .filter_map(OnceCell::get)
            .find(|file| file.name.path() == object.name() && file.bias == object.bias())
            .map(|file| file.identity)
    }
}

fn syscall<'a>(file: Bytes<'a>, op: &'static str, error: SysError) -> Error<'a> {
    let SysError::Os(errno) = error;
    file.error(ErrorKind::SystemCall(op, errno as i32))
}

fn align_up(value: u64, alignment: u64) -> Option<u64> {
    value
        .checked_add(alignment - 1)
        .map(|top| top & !(alignment - 1))
}

fn map_loads<'a>(image: Image<'a>, fd: &OpenFile, page: u64) -> Result<(u64, Mapping), Error<'a>> {
    let mask = page - 1;
    let bad = || image.file.error(ErrorKind::InvalidProgramHeader);
    let mut first = u64::MAX;
    let mut last = 0;
    let mut align = page;
    for ph in image.segments() {
        let ph = ph?;
        if ph.kind != PT_LOAD || ph.mem_size == 0 {
            continue;
        }
        let start = ph.address & !mask;
        let end = align_up(ph.address + ph.mem_size, page).ok_or_else(bad)?;
        if start < last
            || ph.address & mask != ph.offset & mask
            || (ph.align > 1
                && (!ph.align.is_power_of_two() || ph.address % ph.align != ph.offset % ph.align))
        {
            return Err(image.at(ph.header_offset, ErrorKind::InvalidProgramHeader));
        }
        first = first.min(start);
        last = end;
        align = align.max(ph.align);
    }
    let span = last
        .checked_sub(first)
        .and_then(|size| size.checked_add(align - page))
        .filter(|&size| size > 0)
        .ok_or_else(bad)?;
    let reservation = Mapping::anonymous(span as usize, PROT_NONE)
        .map_err(|error| syscall(image.file, "reserve PT_LOAD range", error))?;
    let bias = (reservation.address as u64)
        .checked_sub(first)
        .and_then(|start| align_up(start, align))
        .ok_or_else(bad)?;
    bias.checked_add(last).ok_or_else(bad)?;
    for ph in image.segments() {
        let ph = ph?;
        if ph.kind == PT_LOAD && ph.mem_size != 0 {
            map_segment(image.file, ph, fd, bias, page)?;
        }
    }
    Ok((bias, reservation))
}

fn map_segment<'a>(
    mut file: Bytes<'a>,
    ph: Segment,
    fd: &OpenFile,
    bias: u64,
    page: u64,
) -> Result<(), Error<'a>> {
    file.offset = ph.header_offset;
    let mask = page - 1;
    let vaddr = bias + ph.address;
    let start = vaddr & !mask;
    let end = bias + ((ph.address + ph.mem_size + mask) & !mask);
    let prot = ((ph.flags & PF_R) >> 2 | (ph.flags & PF_W) | (ph.flags & PF_X) << 2) as i32;
    let mut anonymous = start;
    if ph.file_size != 0 {
        let file_end = vaddr + ph.file_size;
        anonymous = bias + ((ph.address + ph.file_size + mask) & !mask);
        let tail = if ph.file_size < ph.mem_size {
            anonymous - file_end
        } else {
            0
        };
        let file_prot = if tail != 0 {
            (prot | PROT_WRITE) & !PROT_EXEC
        } else {
            prot
        };
        mmap(
            start as *mut u8,
            (anonymous - start) as usize,
            file_prot,
            MAP_PRIVATE | MAP_FIXED,
            fd.0.raw() as i32,
            (ph.offset & !mask) as i64,
        )
        .map_err(|error| syscall(file, "mmap file PT_LOAD", error))?;
        if tail != 0 {
            // SAFETY: these bytes lie in the final writable private file page of this load.
            unsafe { ptr::write_bytes(file_end as *mut u8, 0, tail as usize) };
        }
        if file_prot != prot {
            mprotect(start as *mut u8, (anonymous - start) as usize, prot)
                .map_err(|error| syscall(file, "mprotect PT_LOAD", error))?;
        }
    }
    if anonymous < end {
        mmap(
            anonymous as *mut u8,
            (end - anonymous) as usize,
            prot,
            MAP_PRIVATE | MAP_FIXED | MAP_ANONYMOUS,
            -1,
            0,
        )
        .map_err(|error| syscall(file, "mmap PT_LOAD bss", error))?;
    }
    Ok(())
}

fn relocate_and_initialize(stack: *const usize, auxv: &Auxv) {
    let mut path = [0u8; PATH_MAX];
    let len = fs::readlink("/proc/self/exe", &mut path).unwrap_or_else(|error| fail(error));
    if len == 0 || len >= path.len() || path[0] != b'/' {
        fail("invalid executable path");
    }
    let name_map = Mapping::name(&path[..len]).unwrap_or_else(|error| fail(error));
    let name = name_map.path();
    // SAFETY: exec validated and mapped the entire PHDR table and its loads.
    // The pathname above is only for display and $ORIGIN dependency search.
    let root = unsafe {
        let headers = slice::from_raw_parts(auxv.phdr as *const u8, auxv.phnum * 56);
        Object::from_mapped(name, headers, auxv.program_bias, auxv.entry, snapshot)
    }
    .unwrap_or_else(|error| fail(error));
    let interpreter =
        describe_interpreter(auxv, root.interpreter_name().unwrap_or(c"ld-breenix.so"));
    let mut objects = [root; OBJECTS];
    let mut edges = [Dependency::default(); DEPENDENCIES];
    let mut candidate = [0u8; PATH_MAX];
    let files = FileSet {
        main_path: name,
        page: auxv.page,
        loaded: [const { OnceCell::new() }; OBJECTS - 1],
    };
    let scope = Scope::load(root, &mut objects, &mut edges, &mut candidate, |path| {
        files.open(path, &[])
    })
    .unwrap_or_else(|error| fail(error));
    for object in scope.objects() {
        object
            .relocate(scope.objects(), |patch| {
                // SAFETY: the relocation core bounds this store to the object's writable PT_LOAD.
                unsafe { (patch.address as *mut u64).write_unaligned(patch.value.to_le()) };
            })
            .unwrap_or_else(|error| fail(error));
    }
    for object in scope.objects() {
        if let Some(range) = object.relro() {
            protect_relro(range.start, range.end, auxv.page).unwrap_or_else(|error| fail(error));
        }
    }
    // SAFETY: after successful startup we keep these files and mapped loads
    // resident. During failure fail() terminates the process.
    let dl = unsafe {
        dl::publish_initial(&scope, &files, auxv.page, interpreter)
    };
    initialize_libc(stack, scope.objects(), dl);
    let mut state = [InitState::default(); OBJECTS];
    scope
        .initializers(&mut state, |array| run_initializers(array.range))
        .unwrap_or_else(|error| fail(error));
    core::mem::forget(files);
    core::mem::forget(name_map);
}

fn run_initializers(range: core::ops::Range<u64>) {
    for slot in range.step_by(8) {
        // SAFETY: this slot lies in an INIT_ARRAY validated and relocated by the loader.
        let address = unsafe { (slot as *const usize).read_unaligned() };
        // SAFETY: the gABI INIT_ARRAY holds AMD64 ABI function pointers.
        let initialize: unsafe extern "C" fn() = unsafe { core::mem::transmute(address) };
        // SAFETY: the object's relocations have completed before initialization.
        unsafe { initialize() };
    }
}

fn initialize_libc(
    stack: *const usize,
    objects: &[Object<'_>],
    runtime: &'static crate::dl_abi::DlRuntime,
) {
    let mut modules = [TlsModule::default(); MAX_TLS_MODULES];
    let mut count = 0;
    for module in objects.iter().filter_map(|object| object.tls_module()) {
        modules[count] = module;
        count += 1;
    }
    let libc = objects
        .iter()
        .find(|object| object.name().ends_with(b"/libc.so"));
    let lookup =
        |name| libc.and_then(|object| object.lookup(name).unwrap_or_else(|error| fail(error)));
    if count != 0 {
        let address = lookup(b"__breenix_libc_register_tls")
            .unwrap_or_else(|| fail("no Breenix libc TLS registration export"));
        // SAFETY: the shared libc export uses the TlsModule startup ABI.
        let register: unsafe extern "C" fn(*const TlsModule, usize) -> i32 =
            unsafe { core::mem::transmute(address as usize) };
        // SAFETY: modules is readable for count entries. Their images are
        // in relocated PT_LOAD mappings that outlive the interpreter handoff.
        if unsafe { register(modules.as_ptr(), count) } != 0 {
            fail("Breenix libc rejected TLS templates");
        }
    }
    if let Some(address) = lookup(b"__breenix_libc_register_dl") {
        // SAFETY: this export of the shared libc accepts the private loader table.
        let register: unsafe extern "C" fn(*const crate::dl_abi::DlRuntime) =
            unsafe { core::mem::transmute(address as usize) };
        // SAFETY: the table is immutable and resident in the interpreter.
        unsafe { register(runtime) };
    }
    if let Some(address) = lookup(b"__breenix_libc_init") {
        // SAFETY: this export of the shared Breenix libc has this startup ABI.
        let initialize: unsafe extern "C" fn(*const usize) =
            unsafe { core::mem::transmute(address as usize) };
        // SAFETY: the object is relocated and stack is the kernel's initial stack.
        unsafe { initialize(stack) };
    } else if count != 0 {
        fail("no Breenix libc startup export");
    }
}

fn protect_relro(start: u64, end: u64, page: u64) -> Result<(), SysError> {
    let start = start & !(page - 1);
    if end % page != 0 || end <= start {
        return Err(Errno::EINVAL.into());
    }
    mprotect(start as *mut u8, (end - start) as usize, PROT_READ)
}

fn describe_interpreter(auxv: &Auxv, name: &core::ffi::CStr) -> ObjectInfo {
    let base = auxv.base as *const u8;
    // SAFETY: AT_BASE addresses the interpreter's ELF header in its first load.
    let header = unsafe { slice::from_raw_parts(base, 64) };
    let file = Bytes {
        name: name.to_bytes(),
        data: header,
        offset: 0,
    };
    let phoff = file.u64(32).unwrap_or_else(|error| fail(error));
    let phnum = file.u16(56).unwrap_or_else(|error| fail(error));
    // SAFETY: that load also contains the interpreter's program headers.
    let table =
        unsafe { slice::from_raw_parts(base.wrapping_add(phoff as usize), phnum as usize * 56) };
    let mut dynamic = ptr::null();
    for (index, data) in table.chunks_exact(56).enumerate() {
        let header = Bytes {
            data,
            offset: phoff + index as u64 * 56,
            ..file
        };
        let kind = header.u32(0).unwrap_or_else(|error| fail(error));
        let load = auxv.base + header.u64(16).unwrap_or_else(|error| fail(error));
        match kind {
            PT_GNU_RELRO => {
                let end = load + header.u64(40).unwrap_or_else(|error| fail(error));
                protect_relro(load, end, auxv.page).unwrap_or_else(|error| fail(error));
            }
            PT_DYNAMIC => dynamic = load as *const _,
            _ => {}
        }
    }
    extern "C" {
        static __breenix_text_start: u8;
        static __breenix_got_start: u8;
    }
    ObjectInfo {
        object: LoadedObject {
            name: name.as_ptr(),
            bias: auxv.base,
            phdr: table.as_ptr().cast(),
            phnum: usize::from(phnum),
            dynamic,
            tls_module_id: 0,
        },
        text_base: ptr::addr_of!(__breenix_text_start).expose_provenance() as u64,
        got_base: ptr::addr_of!(__breenix_got_start).expose_provenance() as u64,
    }
}

/// Enter the interpreter after its own bootstrap relocations.
///
/// # Safety
/// The argument is the kernel's original AMD64 process stack, including auxv.
pub unsafe extern "C" fn start(stack: *const usize) -> ! {
    // SAFETY: the naked entry forwards the unchanged initial stack.
    let auxv = unsafe { Auxv::read(stack) }.unwrap_or_else(|error| fail(error));
    relocate_and_initialize(stack, &auxv);
    // SAFETY: AT_ENTRY names the loaded executable's entry. Restore its initial
    // stack and clear the psABI's optional termination callback register.
    unsafe {
        core::arch::asm!(
            "mov rsp, rdi",
            "xor edx, edx",
            "jmp rsi",
            in("rdi") stack, in("rsi") auxv.entry, options(noreturn),
        )
    }
}

struct Stderr;

impl Write for Stderr {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let mut bytes = text.as_bytes();
        while !bytes.is_empty() {
            let count = io::write(Fd::from_raw(2), bytes).map_err(|_| fmt::Error)?;
            if count == 0 {
                return Err(fmt::Error);
            }
            bytes = &bytes[count..];
        }
        Ok(())
    }
}

pub fn fail(error: impl fmt::Display) -> ! {
    let _ = writeln!(Stderr, "ld-breenix: {error}");
    process::exit(127)
}
