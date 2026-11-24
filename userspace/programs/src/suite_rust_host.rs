//! Exercise the libc surface needed by hosted Rust tools.
#![deny(unsafe_op_in_unsafe_fn)]

use breenix_suite::suite::{case, category, suite, CaseError, CaseResult, Suite};
use libbreenix::{errno::Errno, error::Error, io, memory, process, types::OwnedFd};
use std::ffi::{c_void, CString};
use std::fs::{self, File, OpenOptions};
use std::hint::black_box;
use std::io::{Read, Seek, Write};
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::Mutex;
use std::thread;

// The mapping sizes and thread count come from the X0.9 resource profile.
const ANON_BYTES: usize = 165_164_496;
const FILE_BYTES: usize = 163_067_344;
const THREAD_COUNT: usize = 7;
const STACK_BYTES: usize = 8 * 1024 * 1024;
const RECURSION_BYTES: usize = 7 * 1024 * 1024;
const TOUCH_STRIDE: usize = 64 * 1024;
const FILE_CHUNK_BYTES: usize = 1024 * 1024;
const RLIMIT_STACK: i32 = 3; // Breenix <sys/resource.h>.

static SUITE: Suite = suite(
    "rust-host",
    "Rust host runtime",
    &[category(
        "libc-surface",
        "Hosted libc surface",
        &[
            case(
                "startup-order",
                "Startup, exit, thread destructors and program headers",
                startup_order,
            ),
            case(
                "memory-limits",
                "Main stack growth and sparse mapping access from seven pthreads",
                memory_limits,
            ),
            case(
                "file-mmap",
                "Large private file mapping matches read",
                file_mmap,
            ),
            case(
                "process-pipes-locks",
                "Self-spawn, close-on-exec pipes, wait statuses and file locks",
                process_pipes_locks,
            ),
        ],
    )],
);

// Opaque pthread_attr_t storage from the Breenix LP64 pthread ABI.
#[repr(C, align(8))]
struct PthreadAttr([MaybeUninit<u8>; 56]);

extern "C" {
    fn posix_spawn(
        pid: *mut i32,
        path: *const u8,
        actions: *const c_void,
        attr: *const c_void,
        argv: *const *const u8,
        envp: *const *const u8,
    ) -> i32;
    fn getrlimit(resource: i32, limit: *mut [u64; 2]) -> i32;
    fn setrlimit(resource: i32, limit: *const [u64; 2]) -> i32;
    fn mmap(
        addr: *mut c_void,
        len: usize,
        prot: i32,
        flags: i32,
        fd: i32,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(addr: *mut c_void, len: usize) -> i32;
    fn pthread_self() -> usize;
    fn pthread_getattr_np(thread: usize, attr: *mut PthreadAttr) -> i32;
    fn pthread_attr_getstacksize(attr: *const PthreadAttr, size: *mut usize) -> i32;
    fn pthread_attr_destroy(attr: *mut PthreadAttr) -> i32;
}

fn startup_order() -> CaseResult {
    let (read_fd, write_fd) =
        io::pipe().map_err(|err| CaseError::Fail(format!("startup pipe: {err}")))?;
    // SAFETY: pipe returned two distinct descriptors owned by this process.
    let reader = unsafe { File::from_raw_fd(read_fd.raw() as i32) };
    let writer = OwnedFd::new(write_fd);
    io::fcntl_setfd(read_fd, io::fd_flags::FD_CLOEXEC)
        .map_err(|err| CaseError::Fail(format!("startup pipe close-on-exec: {err}")))?;

    let probe = c"/usr/libexec/breenix/rust-host/startup-probe";
    let fd_arg = CString::new(write_fd.raw().to_string()).expect("decimal descriptor");
    let argv = [probe.as_ptr().cast(), fd_arg.as_ptr().cast(), null()];
    let envp = [null()];
    let mut pid = 0;
    // SAFETY: the path and both null-terminated vectors live through the call.
    // Null actions and attributes request POSIX defaults.
    let result = unsafe {
        posix_spawn(
            &mut pid,
            argv[0],
            null(),
            null(),
            argv.as_ptr(),
            envp.as_ptr(),
        )
    };
    drop(writer);
    if result != 0 {
        return Err(CaseError::Fail(format!(
            "posix_spawn startup probe: errno {result}"
        )));
    }
    if pid <= 0 {
        return Err(CaseError::Fail(format!(
            "posix_spawn returned invalid child PID {pid}"
        )));
    }

    // ELF gABI initialization, joined thread destruction, process LIFO and reverse fini.
    const EXPECTED: &[u8] = b"abcdmstxrjkehzy";
    let mut order = Vec::new();
    let mut reader = reader.take((EXPECTED.len() + 1) as u64);
    let read_result = reader.read_to_end(&mut order);
    drop(reader);

    let mut status = 0;
    loop {
        match process::waitpid(pid, &mut status, 0) {
            Ok(_) => break,
            Err(Error::Os(Errno::EINTR)) => continue,
            Err(err) => return Err(CaseError::Fail(format!("wait for startup probe: {err}"))),
        }
    }
    read_result.map_err(|err| CaseError::Fail(format!("read startup pipe: {err}")))?;
    if !process::wifexited(status) || process::wexitstatus(status) != 0 || order != EXPECTED {
        return Err(CaseError::Fail(format!(
            "startup probe wait status {status:#x}; events {:?}, expected {:?}",
            String::from_utf8_lossy(&order),
            String::from_utf8_lossy(EXPECTED),
        )));
    }
    Ok(())
}

struct Mapping {
    address: *mut c_void,
    len: usize,
}

impl Mapping {
    fn private(len: usize, prot: i32, flags: i32, fd: i32) -> Result<Self, String> {
        // SAFETY: null lets libc choose the address. The caller provides an appropriate fd.
        let address = unsafe { mmap(null_mut(), len, prot, memory::MAP_PRIVATE | flags, fd, 0) };
        if address == (-1isize) as *mut c_void {
            return Err(format!(
                "mmap {len} bytes: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Self { address, len })
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: this object owns the mapped range and its borrows have ended.
        unsafe { munmap(self.address, self.len) };
    }
}

#[inline(never)]
fn recurse_stack(start: usize) -> usize {
    // Keep each recursive frame below one page so it faults in successive pages.
    let mut frame = [0u8; 2048];
    let used = start.saturating_sub(black_box(frame.as_mut_ptr()) as usize);
    let depth = if used >= RECURSION_BYTES {
        used
    } else {
        recurse_stack(start)
    };
    // Keep this frame live across the recursive call, including in release builds.
    black_box(&mut frame);
    depth
}

fn pthread_stack_size() -> Result<usize, String> {
    let mut attr = MaybeUninit::<PthreadAttr>::uninit();
    // SAFETY: attr has storage for the current thread's attributes.
    let result = unsafe { pthread_getattr_np(pthread_self(), attr.as_mut_ptr()) };
    if result != 0 {
        return Err(format!("pthread_getattr_np: errno {result}"));
    }
    let mut size = 0;
    // SAFETY: getattr initialized attr; size is writable.
    let result = unsafe { pthread_attr_getstacksize(attr.as_ptr(), &mut size) };
    // SAFETY: attr is initialized and is no longer needed.
    let destroy = unsafe { pthread_attr_destroy(attr.as_mut_ptr()) };
    if result != 0 || destroy != 0 {
        return Err(format!(
            "pthread stack attributes: getsize={result}, destroy={destroy}"
        ));
    }
    Ok(size)
}

fn memory_limits() -> CaseResult {
    let mut limit = [0u64; 2];
    // SAFETY: limit is writable storage for soft and hard RLIMIT_STACK.
    if unsafe { getrlimit(RLIMIT_STACK, &mut limit) } != 0 {
        return Err(format!(
            "getrlimit(RLIMIT_STACK): {}",
            std::io::Error::last_os_error()
        )
        .into());
    }
    limit[0] = STACK_BYTES as u64;
    // SAFETY: limit supplies both rlimit fields, preserving the hard limit.
    if unsafe { setrlimit(RLIMIT_STACK, &limit) } != 0 {
        return Err(format!(
            "setrlimit(RLIMIT_STACK): {}",
            std::io::Error::last_os_error()
        )
        .into());
    }
    let stack_anchor = 0u8;
    let depth = recurse_stack(black_box(&stack_anchor) as *const u8 as usize);
    if !(RECURSION_BYTES..STACK_BYTES).contains(&depth) {
        return Err(format!("main-thread stack depth {depth} outside 7–8 MiB").into());
    }

    let mapping = Mapping::private(
        ANON_BYTES,
        memory::PROT_READ | memory::PROT_WRITE,
        memory::MAP_ANONYMOUS,
        -1,
    )?;
    // SAFETY: the anonymous mapping is initialized, writable and uniquely owned.
    let bytes =
        unsafe { std::slice::from_raw_parts_mut(mapping.address.cast::<u8>(), mapping.len) };
    let chunk_bytes = ANON_BYTES.div_ceil(THREAD_COUNT * TOUCH_STRIDE) * TOUCH_STRIDE;
    let chunks = bytes.chunks_mut(chunk_bytes);
    if chunks.len() != THREAD_COUNT {
        return Err("anonymous mapping did not split into seven worker ranges".into());
    }
    let start = Mutex::new(());
    thread::scope(|scope| -> CaseResult {
        let mut workers = Vec::with_capacity(THREAD_COUNT);
        // Keep the seven worker stacks live together at the end of creation.
        let gate = start.lock().unwrap();
        for (index, chunk) in chunks.enumerate() {
            let start = &start;
            let worker = thread::Builder::new()
                .stack_size(STACK_BYTES)
                .spawn_scoped(scope, move || -> Result<(), String> {
                    drop(start.lock().unwrap());
                    let size = pthread_stack_size()?;
                    if size != STACK_BYTES {
                        return Err(format!(
                            "pthread {index} stack is {size} bytes, expected {STACK_BYTES}"
                        ));
                    }
                    let tag = (index + 1) as u8;
                    for byte in chunk.iter_mut().step_by(TOUCH_STRIDE) {
                        // SAFETY: each worker owns its disjoint writable slice.
                        unsafe { std::ptr::write_volatile(byte, tag) };
                        // SAFETY: the same worker owns the byte it just wrote.
                        if unsafe { std::ptr::read_volatile(byte) } != tag {
                            return Err(format!(
                                "pthread {index} could not read back its mapping byte"
                            ));
                        }
                    }
                    Ok(())
                })
                .map_err(|err| format!("create pthread {index}: {err}"))?;
            workers.push(worker);
        }
        drop(gate);
        for worker in workers {
            worker.join().map_err(|_| "mapping worker panicked")??;
        }
        Ok(())
    })
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(case: &str) -> Result<Self, String> {
        let path = PathBuf::from(format!("/tmp/rust-host-{case}-{}", std::process::id()));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .map_err(|err| format!("create scratch directory: {err}"))?;
        Ok(Self(path))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn file_mmap() -> CaseResult {
    let scratch = Scratch::new("mmap")?;
    let path = scratch.0.join("rlib-sized");
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|err| format!("create mapping fixture: {err}"))?;
    let mut buffer = vec![0u8; FILE_CHUNK_BYTES];
    for offset in (0..FILE_BYTES).step_by(FILE_CHUNK_BYTES) {
        let count = buffer.len().min(FILE_BYTES - offset);
        for (word, bytes) in buffer[..count].chunks_mut(8).enumerate() {
            let position = (offset + word * 8) as u64;
            bytes.copy_from_slice(&position.to_le_bytes()[..bytes.len()]);
        }
        file.write_all(&buffer[..count])
            .map_err(|err| format!("write mapping fixture at {offset}: {err}"))?;
    }
    file.rewind()
        .map_err(|err| format!("rewind mapping fixture: {err}"))?;
    let mapping = Mapping::private(FILE_BYTES, memory::PROT_READ, 0, file.as_raw_fd())?;
    // SAFETY: the file was written to the mapped length and is kept open and unchanged.
    let mapped = unsafe { std::slice::from_raw_parts(mapping.address.cast::<u8>(), mapping.len) };
    for (index, chunk) in mapped.chunks(buffer.len()).enumerate() {
        let offset = index * FILE_CHUNK_BYTES;
        let read = &mut buffer[..chunk.len()];
        file.read_exact(read)
            .map_err(|err| format!("read mapping fixture at {offset}: {err}"))?;
        if chunk != read {
            let byte = chunk
                .iter()
                .zip(read)
                .position(|(left, right)| left != right)
                .expect("equal-length chunks differ");
            return Err(format!("mapping differs from read at byte {}", offset + byte).into());
        }
    }
    if file
        .read(&mut buffer[..1])
        .map_err(|err| format!("read mapping fixture EOF: {err}"))?
        != 0
    {
        return Err("mapping fixture has bytes past the expected end".into());
    }
    Ok(())
}

fn process_pipes_locks() -> CaseResult {
    let scratch = Scratch::new("locks")?;
    let output = std::process::Command::new("/usr/libexec/breenix/rust-host/process-probe")
        .arg(&scratch.0)
        .output()
        .map_err(|err| format!("run process probe: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "process probe {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim(),
        )
        .into());
    }
    Ok(())
}

fn main() {
    SUITE.run()
}
