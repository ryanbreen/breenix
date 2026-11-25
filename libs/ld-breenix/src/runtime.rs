//! Map the AMD64 initial load set using the Breenix syscall ABI and ELF gABI §§7–8.

use crate::elf::{Bytes, Image, Segment, PF_R, PF_W, PT_GNU_RELRO, PT_LOAD};
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

struct Auxv {
    entry: u64,
    page: u64,
    base: u64,
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
        };
        // SAFETY: the kernel's auxv is AT_NULL-terminated.
        while unsafe { cursor.read() } != 0 {
            // SAFETY: each non-null tag begins a two-word entry.
            let pair = unsafe { cursor.cast::<[usize; 2]>().read() };
            match pair {
                [6, page] => auxv.page = page as u64,    // AT_PAGESZ
                [7, base] => auxv.base = base as u64,    // AT_BASE
                [9, entry] => auxv.entry = entry as u64, // AT_ENTRY
                [23, secure] if secure != 0 => {
                    return Err("secure exec requires a trusted library search")
                }
                _ => {}
            }
            // SAFETY: a non-null pair is followed by another entry.
            cursor = unsafe { cursor.add(2) };
        }
        if auxv.entry == 0 || auxv.base == 0 || !auxv.page.is_power_of_two() {
            return Err("invalid exec auxiliary vector");
        }
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
}

impl Drop for Mapping {
    fn drop(&mut self) {
        let _ = munmap(self.address, self.len);
    }
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
}

impl DependencyFile {
    fn object(&self) -> Result<Object<'_>, Error<'_>> {
        Object::parse(self.name.bytes(), self.view.bytes(), self.bias)
    }
}

struct FileSet<'a> {
    main_path: &'a [u8],
    page: u64,
    loaded: [OnceCell<DependencyFile>; OBJECTS - 1],
}

impl FileSet<'_> {
    fn open(&self, path: &[u8]) -> Result<Option<Object<'_>>, Error<'_>> {
        let main = Bytes {
            name: self.main_path,
            data: b"",
            offset: 0,
        };
        let fd = match OpenFile::open(path) {
            Ok(fd) => fd,
            Err(SysError::Os(Errno::ENOENT | Errno::ENOTDIR)) => return Ok(None),
            Err(error) => return Err(syscall(main, "open", error)),
        };
        let stat = fs::fstat(fd.0).map_err(|error| syscall(main, "fstat", error))?;
        let identity = (stat.st_dev, stat.st_ino);
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
        let name = Mapping::anonymous(path.len(), PROT_READ | PROT_WRITE)
            .map_err(|error| syscall(main, "mmap object name", error))?;
        // SAFETY: the fresh name mapping is writable and disjoint from the search buffer.
        unsafe { ptr::copy_nonoverlapping(path.as_ptr(), name.address, path.len()) };
        let bias = map_loads(image, &fd, self.page).map_err(stable)?;
        let object = slot.get_or_init(|| DependencyFile {
            identity,
            name,
            view,
            bias,
        });
        object.object().map(Some)
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

fn map_loads<'a>(image: Image<'a>, fd: &OpenFile, page: u64) -> Result<u64, Error<'a>> {
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
    // The loaded objects must remain mapped after the interpreter hands off.
    core::mem::forget(reservation);
    Ok(bias)
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
    let name = &path[..len];
    let program = OpenFile::open(name).unwrap_or_else(|error| fail(error));
    let view = fs::fstat(program.0)
        .and_then(|stat| program.view(stat))
        .unwrap_or_else(|error| fail(error));
    let file = Bytes {
        name,
        data: view.bytes(),
        offset: 0,
    };
    let link_entry = file.u64(24).unwrap_or_else(|error| fail(error));
    let bias = auxv
        .entry
        .checked_sub(link_entry)
        .unwrap_or_else(|| fail("invalid executable bias"));
    let root = Object::parse(name, file.data, bias).unwrap_or_else(|error| fail(error));
    let mut objects = [root; OBJECTS];
    let mut edges = [Dependency::default(); DEPENDENCIES];
    let mut candidate = [0u8; PATH_MAX];
    let files = FileSet {
        main_path: name,
        page: auxv.page,
        loaded: [const { OnceCell::new() }; OBJECTS - 1],
    };
    let scope = Scope::load(root, &mut objects, &mut edges, &mut candidate, |path| {
        files.open(path)
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
            protect_relro(range.start, range.end, auxv.page);
        }
    }
    initialize_libc(stack, scope.objects());
    let mut state = [InitState::default(); OBJECTS];
    scope
        .initializers(&mut state, |array| {
            for slot in array.range.step_by(8) {
                // SAFETY: this slot lies in an in-memory init array validated by Object::parse.
                let address = unsafe { (slot as *const usize).read_unaligned() };
                // SAFETY: ELF init arrays contain callable AMD64 ABI function pointers.
                let initialize: unsafe extern "C" fn() = unsafe { core::mem::transmute(address) };
                // SAFETY: relocation completes before Scope::initializers invokes this callback.
                unsafe { initialize() };
            }
        })
        .unwrap_or_else(|error| fail(error));
}

fn initialize_libc(stack: *const usize, objects: &[Object<'_>]) {
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

fn protect_relro(start: u64, end: u64, page: u64) {
    let start = start & !(page - 1);
    if end % page != 0 || end <= start {
        fail("invalid GNU_RELRO page range");
    }
    mprotect(start as *mut u8, (end - start) as usize, PROT_READ)
        .unwrap_or_else(|error| fail(error));
}

fn protect_interpreter(auxv: &Auxv) {
    let base = auxv.base as *const u8;
    // SAFETY: AT_BASE addresses the interpreter's ELF header in its first load.
    let header = unsafe { slice::from_raw_parts(base, 64) };
    let file = Bytes {
        name: b"ld-breenix.so",
        data: header,
        offset: 0,
    };
    let phoff = file.u64(32).unwrap_or_else(|error| fail(error));
    let phnum = file.u16(56).unwrap_or_else(|error| fail(error));
    // SAFETY: that load also contains the interpreter's program headers.
    let table =
        unsafe { slice::from_raw_parts(base.wrapping_add(phoff as usize), phnum as usize * 56) };
    for (index, data) in table.chunks_exact(56).enumerate() {
        let header = Bytes {
            data,
            offset: phoff + index as u64 * 56,
            ..file
        };
        if header.u32(0).unwrap_or_else(|error| fail(error)) == PT_GNU_RELRO {
            let start = auxv.base + header.u64(16).unwrap_or_else(|error| fail(error));
            let end = start + header.u64(40).unwrap_or_else(|error| fail(error));
            protect_relro(start, end, auxv.page);
        }
    }
}

/// Enter the interpreter after its own bootstrap relocations.
///
/// # Safety
/// The argument is the kernel's original AMD64 process stack, including auxv.
pub unsafe extern "C" fn start(stack: *const usize) -> ! {
    // SAFETY: the naked entry forwards the unchanged initial stack.
    let auxv = unsafe { Auxv::read(stack) }.unwrap_or_else(|error| fail(error));
    protect_interpreter(&auxv);
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
