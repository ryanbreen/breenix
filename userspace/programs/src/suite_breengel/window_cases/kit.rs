//! Shared pieces of the window cases: raw calls with their exact returns, windows that destroy
//! themselves, the test pattern, user memory with chosen protections, and helper processes,
//! all of them created through `suite::spawn`.

use crate::*;

use libbreenix::suite::{spawn, Run};
pub(crate) use libbreenix::types::Fd;
pub(crate) use libbreenix::window::abi;
use abi::{op, Plain};

pub(crate) use abi::errno::{EAGAIN, EBUSY, EFAULT, EINTR, EINVAL, ENOENT, ENOMEM, EPERM, ETIMEDOUT};

// ---------------------------------------------------------------------------------------
// Raw calls
// ---------------------------------------------------------------------------------------

/// `window(op, &mut value, size_of(value))`: the raw return.
pub(crate) fn call<T: Plain>(op: u32, value: &mut T) -> i64 {
    // SAFETY: `value` is a live plain structure of its own size.
    unsafe { libbreenix::window::raw_call(op, value as *mut T as *mut u8, core::mem::size_of::<T>()) }
}

/// `window(op, ptr, len)` with any pointer: the raw return.
pub(crate) fn call_at(op: u32, ptr: u64, len: usize) -> i64 {
    // SAFETY: the kernel checks every user pointer; a bad one is EFAULT.
    unsafe { libbreenix::window::raw_call(op, ptr as *mut u8, len) }
}

/// An op whose argument is a window id.
pub(crate) fn call_id(op: u32, id: u64) -> i64 {
    call(op, &mut abi::WinId { id })
}

/// `ret` must be `-errno`.
pub(crate) fn want_errno(ret: i64, errno: i32, section: &str, what: &str) -> CaseResult {
    ensure(ret == -(errno as i64), section, format!(
        "{what} {}; the contract gives {}", outcome(ret), errno_name(errno as i64)))
}

/// `ret` must be a success.
pub(crate) fn want_ok(ret: i64, section: &str, what: &str) -> Result<i64, libbreenix::suite::CaseError> {
    if ret >= 0 {
        Ok(ret)
    } else {
        Err(libbreenix::suite::CaseError::Fail(format!("{what} {} ({CONTRACT} section {section})", outcome(ret))))
    }
}

/// The interface must answer `VERSION` with 2 before a case can check anything else.
pub(crate) fn interface() -> CaseResult {
    match window_version() {
        2 => Ok(()),
        ret => at("17.1", format!("window interface v2 is absent: window(VERSION) {}", outcome(ret))),
    }
}

// ---------------------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------------------

/// A window this process created; destroyed when dropped.
pub(crate) struct W {
    pub(crate) c: abi::Create,
    pub(crate) live: bool,
}

impl W {
    pub(crate) fn new(width: u32, height: u32, buffers: u32) -> Result<W, libbreenix::suite::CaseError> {
        let mut c = abi::Create::ZERO;
        c.width = width;
        c.height = height;
        c.buffers = buffers;
        let ret = call(op::CREATE, &mut c);
        want_ok(ret, "17.1", &format!("CREATE {width}x{height} with {buffers} buffers"))?;
        Ok(W { c, live: true })
    }

    pub(crate) fn id(&self) -> u64 {
        self.c.id
    }

    pub(crate) fn len(&self) -> usize {
        (self.c.width * self.c.height * 4) as usize
    }

    /// Buffer `i` as the owner maps it.
    pub(crate) fn buf(&self, i: u32) -> &mut [u8] {
        // SAFETY: CREATE mapped `len` bytes at this address into this process.
        unsafe { core::slice::from_raw_parts_mut(self.c.buffer[i as usize] as *mut u8, self.len()) }
    }

    pub(crate) fn tree(&self, i: u32) -> &mut [u8] {
        let kib = (self.c.tree_kib as usize) * 1024;
        // SAFETY: CREATE mapped `buffers * tree_kib` KiB of tree pages at `tree`.
        unsafe { core::slice::from_raw_parts_mut((self.c.tree as usize + i as usize * kib) as *mut u8, kib) }
    }

    pub(crate) fn register(&self, title: &[u8]) -> i64 {
        title_op(op::REGISTER, self.id(), title.as_ptr() as u64, title.len() as u32)
    }

    pub(crate) fn acquire(&self, timeout_ns: u64) -> Result<abi::Acquire, i64> {
        acquire(self.id(), timeout_ns)
    }

    pub(crate) fn commit(&self, buffer: u32, damage: &[abi::Rect]) -> Result<abi::Commit, i64> {
        commit(self.id(), buffer, damage, None)
    }

    /// Acquire a buffer, fill it with pattern `seed` and commit it with whole-window damage.
    pub(crate) fn frame(&self, seed: u32) -> Result<abi::Commit, String> {
        let a = self.acquire(2_000_000_000).map_err(|r| format!("ACQUIRE {}", outcome(r)))?;
        fill(self.buf(a.buffer), self.c.width, self.c.height, seed);
        self.commit(a.buffer, &[full(&self.c)]).map_err(|r| format!("COMMIT {}", outcome(r)))
    }

    pub(crate) fn destroy(&mut self) -> i64 {
        self.live = false;
        call_id(op::DESTROY, self.id())
    }
}

impl Drop for W {
    fn drop(&mut self) {
        if self.live {
            let _ = call_id(op::DESTROY, self.c.id);
        }
    }
}

pub(crate) fn full(c: &abi::Create) -> abi::Rect {
    abi::Rect::new(0, 0, c.width as i32, c.height as i32)
}

pub(crate) fn title_op(op: u32, id: u64, ptr: u64, len: u32) -> i64 {
    let mut t = abi::Title::ZERO;
    t.id = id;
    t.ptr = ptr;
    t.len = len;
    call(op, &mut t)
}

pub(crate) fn acquire(id: u64, timeout_ns: u64) -> Result<abi::Acquire, i64> {
    let mut a = abi::Acquire::ZERO;
    a.id = id;
    a.timeout_ns = timeout_ns;
    let r = call(op::ACQUIRE, &mut a);
    if r < 0 { Err(r) } else { Ok(a) }
}

pub(crate) fn commit(id: u64, buffer: u32, damage: &[abi::Rect], meta: Option<&abi::Meta>) -> Result<abi::Commit, i64> {
    let mut c = abi::Commit::ZERO;
    c.id = id;
    c.buffer = buffer;
    c.damage_count = damage.len() as u32;
    for (slot, r) in c.damage.iter_mut().zip(damage) {
        *slot = *r;
    }
    if let Some(m) = meta {
        c.meta = m as *const abi::Meta as u64;
        c.meta_len = core::mem::size_of::<abi::Meta>() as u32;
    }
    let r = call(op::COMMIT, &mut c);
    if r < 0 { Err(r) } else { Ok(c) }
}

/// Every registered window.
pub(crate) fn list() -> Result<Vec<abi::WinInfo>, String> {
    let mut v = vec![abi::WinInfo::ZERO; 256];
    let mut l = abi::List::ZERO;
    l.ptr = v.as_mut_ptr() as u64;
    l.capacity = v.len() as u32;
    let n = call(op::LIST, &mut l);
    if n < 0 {
        return Err(format!("LIST {}", outcome(n)));
    }
    v.truncate((n as usize).min(256));
    Ok(v)
}

pub(crate) fn listed(id: u64) -> Result<Option<abi::WinInfo>, String> {
    Ok(list()?.into_iter().find(|w| w.id == id))
}

/// A capture of window `id` (`w` x `h`), with its tree page when `tree` is given.
pub(crate) fn capture(id: u64, w: u32, h: u32, tree: Option<&mut [u8]>) -> Result<(abi::Capture, Vec<u8>), i64> {
    let mut px = vec![0u8; (w * h * 4) as usize];
    let mut c = abi::Capture::ZERO;
    c.id = id;
    c.dst = px.as_mut_ptr() as u64;
    c.dst_len = px.len() as u64;
    if let Some(t) = tree {
        c.tree_dst = t.as_mut_ptr() as u64;
        c.tree_len = t.len() as u64;
    }
    let r = call(op::CAPTURE, &mut c);
    if r < 0 { Err(r) } else { Ok((c, px)) }
}

// ---------------------------------------------------------------------------------------
// The test pattern (the same formula raw-window_test draws)
// ---------------------------------------------------------------------------------------

/// Pattern `seed` at (x, y), BGRX: B = (7x + 13seed) mod 256, G = (5y + 29seed) mod 256,
/// R = (x xor y xor seed) mod 256, X = 0.
pub(crate) fn pattern(seed: u32, x: u32, y: u32) -> [u8; 4] {
    [((7 * x + 13 * seed) & 0xFF) as u8, ((5 * y + 29 * seed) & 0xFF) as u8, ((x ^ y ^ seed) & 0xFF) as u8, 0]
}

pub(crate) fn fill(buf: &mut [u8], w: u32, h: u32, seed: u32) {
    fill_rect(buf, w, abi::Rect::new(0, 0, w as i32, h as i32), seed);
}

/// Pattern `seed` over `r` of a `w`-pixel-wide buffer.
pub(crate) fn fill_rect(buf: &mut [u8], w: u32, r: abi::Rect, seed: u32) {
    for y in r.y..r.y + r.h {
        for x in r.x..r.x + r.w {
            let at = ((y as u32 * w + x as u32) * 4) as usize;
            buf[at..at + 4].copy_from_slice(&pattern(seed, x as u32, y as u32));
        }
    }
}

/// The first pixel of a `w`-wide image whose colour differs from `want(x, y)`, comparing B, G and
/// R (and X when `with_x`).
pub(crate) fn first_diff(img: &[u8], w: u32, h: u32, with_x: bool, want: impl Fn(u32, u32) -> [u8; 4]) -> Option<(u32, u32, [u8; 4], [u8; 4])> {
    crate::remember_frame(img,w as usize,h as usize,None);
    let n = if with_x { 4 } else { 3 };
    for y in 0..h {
        for x in 0..w {
            let at = ((y * w + x) * 4) as usize;
            let expect = want(x, y);
            if img[at..at + n] != expect[..n] {
                let mut got = [0u8; 4];
                got.copy_from_slice(&img[at..at + 4]);
                return Some((x, y, got, expect));
            }
        }
    }
    None
}

/// `img` equals `want` everywhere, or a message naming the first difference.
pub(crate) fn same(img: &[u8], w: u32, h: u32, with_x: bool, what: &str, section: &str, want: impl Fn(u32, u32) -> [u8; 4]) -> CaseResult {
    let difference=first_diff(img,w,h,with_x,&want);
    if crate::PICTURES.load(core::sync::atomic::Ordering::Relaxed) {
        let mut pixels=img[..w as usize*h as usize*4].to_vec();
        let mut reference=Vec::with_capacity(pixels.len());
        for y in 0..h {for x in 0..w {let mut p=want(x,y);p[3]=255;reference.extend_from_slice(&p);}}
        for p in pixels.chunks_exact_mut(4){p[3]=255;}
        *crate::LAST_JUDGED.lock().unwrap_or_else(|e|e.into_inner())=Some((w as usize,h as usize,pixels,Some(reference)));
    }
    match difference {
        None => Ok(()),
        Some((x, y, got, expect)) => at(section, format!(
            "{what}: pixel ({x},{y}) is {:02x?}, the formula gives {:02x?}", got, expect)),
    }
}

// ---------------------------------------------------------------------------------------
// User memory with chosen protections
// ---------------------------------------------------------------------------------------

/// `pages` pages of anonymous memory, the first `readable` readable and writable or only
/// readable, the rest unmapped.
pub(crate) struct Pages {
    pub(crate) base: *mut u8,
    pub(crate) len: usize,
}

impl Pages {
    /// `mapped` pages followed by `unmapped` pages that are not mapped.
    pub(crate) fn new(mapped: usize, unmapped: usize, writable: bool) -> Result<Pages, String> {
        let len = (mapped + unmapped) * PAGE;
        let base = memory::mmap(core::ptr::null_mut(), len, memory::PROT_READ | memory::PROT_WRITE,
            memory::MAP_PRIVATE | memory::MAP_ANONYMOUS, -1, 0).map_err(|e| format!("mmap: {e}"))?;
        if unmapped > 0 {
            // SAFETY: the tail lies inside the mapping just made.
            memory::munmap(unsafe { base.add(mapped * PAGE) }, unmapped * PAGE).map_err(|e| format!("munmap: {e}"))?;
        }
        if !writable {
            memory::mprotect(base, mapped * PAGE, memory::PROT_READ).map_err(|e| format!("mprotect: {e}"))?;
        }
        Ok(Pages { base, len: mapped * PAGE })
    }

    /// Like `new`, with the mapped pages filled by `fill` before any protection change.
    pub(crate) fn filled(mapped: usize, unmapped: usize, writable: bool, fill: impl FnOnce(&mut [u8])) -> Result<Pages, String> {
        let p = Pages::new(mapped, unmapped, true)?;
        // SAFETY: the mapped part is readable and writable until the protection below.
        fill(unsafe { core::slice::from_raw_parts_mut(p.base, p.len) });
        if !writable {
            memory::mprotect(p.base, p.len, memory::PROT_READ).map_err(|e| format!("mprotect: {e}"))?;
        }
        Ok(p)
    }

    pub(crate) fn addr(&self) -> u64 {
        self.base as u64
    }

    /// The address `back` bytes before the end of the mapped part.
    pub(crate) fn end_minus(&self, back: usize) -> u64 {
        self.base as u64 + (self.len - back) as u64
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        // SAFETY: the mapped part is readable.
        unsafe { core::slice::from_raw_parts(self.base, self.len) }
    }
}

impl Drop for Pages {
    fn drop(&mut self) {
        // No mapped part (`unmapped()`'s page is gone already): nothing to unmap, and a
        // zero-length munmap is an error the kernel reports in its log.
        if self.len > 0 {
            let _ = memory::munmap(self.base, self.len);
        }
    }
}

/// An address no mapping covers: a page mapped and unmapped again.
pub(crate) fn unmapped() -> u64 {
    match Pages::new(0, 1, true) {
        Ok(p) => p.addr(),
        Err(_) => 0x10,
    }
}

// ---------------------------------------------------------------------------------------
// Processes and accounting
// ---------------------------------------------------------------------------------------

#[cfg(target_arch = "aarch64")]
mod nr {
    pub const SETUID: u64 = 146;
    pub const PRLIMIT64: u64 = 261;
    pub const GET_MOUSE_POS: u64 = 413;
}
pub(crate) use nr::GET_MOUSE_POS;

pub(crate) fn setuid(uid: u32) -> i64 {
    // SAFETY: setuid takes no pointer.
    unsafe { raw::syscall1(nr::SETUID, uid as u64) as i64 }
}

/// Set this process's `RLIMIT_AS` soft and hard limits to `bytes`.
pub(crate) fn set_rlimit_as(bytes: u64) -> i64 {
    let new = [bytes, bytes];
    // SAFETY: prlimit64 reads two u64s from `new`.
    unsafe { raw::syscall4(nr::PRLIMIT64, 0, 9, new.as_ptr() as u64, 0) as i64 }
}

/// A `/proc` file field `name:` in kB.
fn proc_kb(path: &str, name: &str) -> Result<u64, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
    text.lines()
        .find_map(|l| l.strip_prefix(name).map(|rest| rest.trim_start_matches(':').trim().trim_end_matches("kB").trim().to_string()))
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| format!("{path} has no {name} line"))
}

/// The bytes this process has mapped, as `RLIMIT_AS` counts them (`/proc/self/status` VmSize).
pub(crate) fn vm_size() -> Result<u64, String> {
    let pid = process::getpid().map(|p| p.raw()).unwrap_or(0);
    proc_kb(&format!("/proc/{pid}/status"), "VmSize").map(|kb| kb * 1024)
}

/// Free physical memory (`/proc/meminfo` MemFree), in kB.
pub(crate) fn mem_free_kb() -> Result<u64, String> {
    proc_kb("/proc/meminfo", "MemFree")
}

/// A trace counter's value (`/proc/trace/counters`).
pub(crate) fn counter(name: &str) -> Result<u64, String> {
    let text = std::fs::read_to_string("/proc/trace/counters").map_err(|e| format!("read /proc/trace/counters: {e}"))?;
    for line in text.lines() {
        let mut words = line.split_whitespace();
        if words.next().map(|w| w.trim_end_matches(':')) == Some(name) {
            if let Some(v) = words.next().and_then(|w| w.parse().ok()) {
                return Ok(v);
            }
        }
    }
    Err(format!("/proc/trace/counters has no counter {name}"))
}

/// Run `f` in a child created through `suite::spawn`, as `uid` when given, within `limit_ms`.
/// The child's `Err` message comes back as the result.
pub(crate) fn child(what: &str, limit_ms: u64, uid: Option<u32>, f: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    let (rd, wr) = io::pipe().map_err(|e| format!("pipe: {e}"))?;
    let mut f = Some(f);
    let mut body = || {
        // Only `isolated` hands a child's judged frame to the runner. A copy kept here would
        // never be shown, and would count against limits this child sets (RLIMIT_AS).
        crate::PICTURES.store(false, core::sync::atomic::Ordering::Relaxed);
        let report = Fd::from_raw(3);
        let result = match uid {
            Some(u) if setuid(u) != 0 => Err(format!("setuid({u}) failed")),
            _ => (f.take().unwrap())(),
        };
        match result {
            Ok(()) => 0,
            Err(msg) => {
                let _ = io::write(report, msg.as_bytes());
                1
            }
        }
    };
    let pid = spawn(&[(wr, Fd::from_raw(3))], Run::Call(&mut body)).map_err(|e| format!("spawn: {e}"))?;
    let _ = io::close(wr);
    let pid = pid.raw() as i32;
    let start = now_ms();
    let mut status = 0;
    let exited = loop {
        match process::waitpid(pid, &mut status, WNOHANG) {
            Ok(p) if p.raw() != 0 => break true,
            Ok(_) => {}
            Err(e) => return Err(format!("waitpid: {e}")),
        }
        if now_ms() - start > limit_ms {
            let _ = signal::kill(pid, signal::SIGKILL);
            let _ = process::waitpid(pid, &mut status, 0);
            break false;
        }
        let _ = time::sleep_ms(1);
    };
    let mut msg = Vec::new();
    let mut buf = [0u8; 512];
    while let Ok(n) = io::read(rd, &mut buf) {
        if n == 0 { break; }
        msg.extend_from_slice(&buf[..n]);
    }
    let _ = io::close(rd);
    let msg = String::from_utf8_lossy(&msg).into_owned();
    if !exited {
        Err(format!("{what} did not finish within {limit_ms} ms"))
    } else if process::wifsignaled(status) {
        Err(format!("{what} died of signal {}", process::wtermsig(status)))
    } else if process::wifexited(status) && process::wexitstatus(status) == 0 {
        Ok(())
    } else if msg.is_empty() {
        Err(format!("{what} failed without a message"))
    } else {
        Err(msg)
    }
}

/// How a child ended: its exit status, or the signal that killed it.
pub(crate) enum Ended {
    Exit(i32),
    Signal(i32),
}

/// Run `f` in a child (through `suite::spawn`) and report how it ended, within `limit_ms`.
pub(crate) fn child_end(limit_ms: u64, f: impl FnOnce() -> i32) -> Result<Ended, String> {
    let mut f = Some(f);
    let mut body = || (f.take().unwrap())();
    let pid = spawn(&[], Run::Call(&mut body)).map_err(|e| format!("spawn: {e}"))?.raw() as i32;
    let start = now_ms();
    let mut status = 0;
    loop {
        match process::waitpid(pid, &mut status, WNOHANG) {
            Ok(p) if p.raw() != 0 => break,
            Ok(_) => {}
            Err(e) => return Err(format!("waitpid: {e}")),
        }
        if now_ms() - start > limit_ms {
            let _ = signal::kill(pid, signal::SIGKILL);
            let _ = process::waitpid(pid, &mut status, 0);
            return Err(format!("the child did not finish within {limit_ms} ms"));
        }
        let _ = time::sleep_ms(1);
    }
    if process::wifsignaled(status) {
        Ok(Ended::Signal(process::wtermsig(status)))
    } else {
        Ok(Ended::Exit(process::wexitstatus(status)))
    }
}

/// A helper process with a command pipe on its standard input and its standard output read
/// back line by line. Killed and reaped when dropped.
pub(crate) struct Helper {
    pub(crate) pid: i32,
    to: Fd,
    from: Fd,
    pending: Vec<u8>,
}

impl Helper {
    /// Exec `path` with `argv` (each NUL-terminated) as a helper.
    pub(crate) fn exec(path: &[u8], argv: &[&[u8]]) -> Result<Helper, String> {
        Helper::start(Run::Exec(path, argv))
    }

    /// Run `f` (given the helper's input and output descriptors 0 and 1) as a helper.
    pub(crate) fn call(f: &mut dyn FnMut() -> i32) -> Result<Helper, String> {
        Helper::start(Run::Call(f))
    }

    fn start(run: Run<'_>) -> Result<Helper, String> {
        let (in_r, in_w) = io::pipe().map_err(|e| format!("pipe: {e}"))?;
        let (out_r, out_w) = io::pipe().map_err(|e| format!("pipe: {e}"))?;
        let pid = spawn(&[(in_r, Fd::from_raw(0)), (out_w, Fd::from_raw(1))], run).map_err(|e| format!("spawn: {e}"))?;
        let _ = io::close(in_r);
        let _ = io::close(out_w);
        Ok(Helper { pid: pid.raw() as i32, to: in_w, from: out_r, pending: Vec::new() })
    }

    /// The next line it prints, within `limit_ms`.
    pub(crate) fn line(&mut self, limit_ms: u64) -> Result<String, String> {
        self.try_line(limit_ms)?.ok_or_else(|| format!("no line from the helper within {limit_ms} ms"))
    }

    /// The next line it prints within `limit_ms`, or `None` when it prints none in that time.
    /// An error when it closed its output (it exited) or the read failed.
    pub(crate) fn try_line(&mut self, limit_ms: u64) -> Result<Option<String>, String> {
        let start = now_ms();
        loop {
            if let Some(end) = self.pending.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.pending.drain(..=end).collect();
                return Ok(Some(String::from_utf8_lossy(&line[..line.len() - 1]).into_owned()));
            }
            let left = limit_ms.saturating_sub(now_ms() - start);
            if left == 0 {
                return Ok(None);
            }
            let mut fds = [io::PollFd { fd: self.from.raw() as i32, events: io::poll_events::POLLIN, revents: 0 }];
            if io::poll(&mut fds, left.min(100) as i32).unwrap_or(0) == 0 {
                continue;
            }
            let mut buf = [0u8; 1024];
            match io::read(self.from, &mut buf) {
                Ok(0) => return Err(String::from("the helper closed its output")),
                Ok(n) => self.pending.extend_from_slice(&buf[..n]),
                Err(e) => return Err(format!("read from the helper: {e}")),
            }
        }
    }

    /// Send one command line and return its answer line.
    pub(crate) fn ask(&mut self, cmd: &str) -> Result<String, String> {
        io::write(self.to, format!("{cmd}\n").as_bytes()).map_err(|e| format!("write to the helper: {e}"))?;
        self.line(5_000)
    }

    /// Kill and reap it, whatever state it is in (cleanup).
    pub(crate) fn kill(&mut self) {
        if self.pid > 0 {
            let _ = signal::kill(self.pid, signal::SIGKILL);
            let mut status = 0;
            let _ = process::waitpid(self.pid, &mut status, 0);
            self.pid = 0;
        }
    }

    /// Kill it and reap it, and require that the kill is what ended it: a helper that had
    /// already exited, or that the kill or the reap did not reach, is an error.
    pub(crate) fn kill_checked(&mut self) -> Result<(), String> {
        if self.pid <= 0 {
            return Err(String::from("the helper was already reaped"));
        }
        let pid = self.pid;
        self.pid = 0;
        signal::kill(pid, signal::SIGKILL).map_err(|e| format!("kill({pid}, SIGKILL): {e}"))?;
        let mut status = 0;
        process::waitpid(pid, &mut status, 0).map_err(|e| format!("waitpid({pid}): {e}"))?;
        if process::wifsignaled(status) && process::wtermsig(status) == signal::SIGKILL {
            Ok(())
        } else if process::wifsignaled(status) {
            Err(format!("the helper died of signal {}, not the SIGKILL", process::wtermsig(status)))
        } else {
            Err(format!("the helper had exited with status {} before the SIGKILL", process::wexitstatus(status)))
        }
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        self.kill();
        let _ = io::close(self.to);
        let _ = io::close(self.from);
    }
}

/// `kiosk_test`, claimed: its epoch.
pub(crate) fn kiosk() -> Result<(Helper, u64), String> {
    let mut k = Helper::exec(b"/usr/local/test/bin/kiosk_test\0", &[b"kiosk_test\0"])?;
    let line = k.line(5_000)?;
    match line.strip_prefix("ready ").and_then(|e| e.parse().ok()) {
        Some(epoch) => Ok((k, epoch)),
        None => Err(format!("kiosk_test did not claim the compositor: it said `{line}`")),
    }
}

/// `raw-window_test`, started.
pub(crate) fn raw_client() -> Result<Helper, String> {
    let mut r = Helper::exec(b"/usr/local/test/bin/raw-window_test\0", &[b"raw-window_test\0"])?;
    match r.line(5_000)?.as_str() {
        "ready" => Ok(r),
        other => Err(format!("raw-window_test said `{other}` instead of ready")),
    }
}

/// An event's 320 bytes as the 640 hex digits `kiosk_test event` takes.
pub(crate) fn event_hex(e: &abi::WindowEvent) -> String {
    // SAFETY: WindowEvent is plain data of its own size.
    let bytes = unsafe { core::slice::from_raw_parts(e as *const abi::WindowEvent as *const u8, core::mem::size_of::<abi::WindowEvent>()) };
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Read every queued event of window `id` (the caller owns it) without blocking.
pub(crate) fn read_events(id: u64) -> Result<Vec<abi::WindowEvent>, i64> {
    let mut out = vec![abi::WindowEvent::ZERO; 128];
    let mut r = abi::ReadEvents::ZERO;
    r.id = id;
    r.ptr = out.as_mut_ptr() as u64;
    r.capacity = out.len() as u32;
    let n = call(op::READ_EVENTS, &mut r);
    if n < 0 {
        return Err(n);
    }
    out.truncate(n as usize);
    Ok(out)
}

/// Milliseconds since `start`.
pub(crate) fn since(start: u64) -> u64 {
    now_ms().saturating_sub(start)
}

pub(crate) fn getpid() -> u64 {
    process::getpid().map(|p| p.raw()).unwrap_or(0)
}
