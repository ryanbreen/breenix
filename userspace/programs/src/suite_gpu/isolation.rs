//! `isolation`: per-process GPU isolation (`docs/gui/gpu.md` sections 1.4, 1.4.1, 1.10 and
//! 1.16; layouts and codes in `docs/gui/gpu-abi.md`).
//!
//! Cases that need a second process fork one and talk to it over two pipes in framed
//! messages: a tag byte, a length and the payload. A child ends by sending `K` (done) or
//! `F` with the reason it failed, and exits without returning into the case. Global GPU
//! state is observed through `/proc/trace/counters` (`gpu_contexts`, `gpu_resources`,
//! `gpu_resource_bytes`, `gpu_irq_masked_max_us`) and a thread's kernel output through
//! `/proc/<pid>/task/<tid>/kernel_output` (section 1.16).

use libbreenix::gpu::{self, abi, abi::Plain};
use libbreenix::process::{self, ForkResult};
use libbreenix::signal::{self, SIGKILL};
use libbreenix::suite::{value, CaseError, CaseResult};
use libbreenix::types::Fd;
use libbreenix::{graphics, io, memory, time};

use super::common::*;

type R<T> = Result<T, CaseError>;

const RED: u32 = bgra(0, 0, 0xFF, 0xFF);
const GREEN: u32 = bgra(0, 0xFF, 0, 0xFF);
const BLUE: u32 = bgra(0xFF, 0, 0, 0xFF);
const GREY: u32 = bgra(0x40, 0x40, 0x40, 0xFF);
const TEAL: u32 = bgra(0x80, 0x80, 0x10, 0xFF);

/// K, the kill and observation bound (`docs/gui/gpu.md` section 1.4.1).
const K_MS: u64 = 250;
/// W, the watchdog period (section 1.4.1).
const W_MS: u64 = 2_000;
/// The per-process GPU memory quota (section 1.4).
const QUOTA: u64 = 192 << 20;
/// Test-control points 2 `irq-hold` and 3 `emit-diagnostic` (`docs/gui/gpu-abi.md`).
const IRQ_HOLD: u32 = 2;
const EMIT_DIAGNOSTIC: u32 = 3;
/// What point 3 writes for the calling thread: the 32-byte payload and a newline.
const DIAGNOSTIC_BYTES: u64 = 33;
/// The unprivileged user `test-hooks-absent` runs a child as.
const NOBODY: u64 = 65_534;
/// `setuid`'s system call number.
#[cfg(target_arch = "aarch64")]
const SYS_SETUID: u64 = 146;
#[cfg(target_arch = "x86_64")]
const SYS_SETUID: u64 = 105;
/// The fresh image that checks `exec-reclaims`' post-exec process.
const EXEC_PROBE_BINARY: &str = "/bin/gpu-exec-probe";

// ---------------------------------------------------------------------------------------
// Processes and messages.

fn read_exact(fd: Fd, buf: &mut [u8]) -> bool {
    let mut done = 0;
    while done < buf.len() {
        match io::read(fd, &mut buf[done..]) {
            Ok(0) | Err(_) => return false,
            Ok(n) => done += n,
        }
    }
    true
}

fn write_all(fd: Fd, buf: &[u8]) -> bool {
    let mut done = 0;
    while done < buf.len() {
        match io::write(fd, &buf[done..]) {
            Ok(0) | Err(_) => return false,
            Ok(n) => done += n,
        }
    }
    true
}

fn send_frame(fd: Fd, tag: u8, payload: &[u8]) -> bool {
    let mut head = [0u8; 5];
    head[0] = tag;
    head[1..].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    write_all(fd, &head) && write_all(fd, payload)
}

fn recv_frame(fd: Fd) -> Option<(u8, Vec<u8>)> {
    let mut head = [0u8; 5];
    if !read_exact(fd, &mut head) {
        return None;
    }
    let len = u32::from_le_bytes([head[1], head[2], head[3], head[4]]) as usize;
    let mut payload = vec![0u8; len];
    read_exact(fd, &mut payload).then_some((head[0], payload))
}

fn words64(values: &[u64]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn read64(bytes: &[u8]) -> Vec<u64> {
    bytes.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect()
}

/// The child's end of its two pipes.
pub struct Line {
    rd: Fd,
    wr: Fd,
}

impl Line {
    pub fn send(&self, tag: u8, payload: &[u8]) -> Result<(), String> {
        send_frame(self.wr, tag, payload).then_some(()).ok_or_else(|| "the pipe to the parent closed".to_string())
    }

    pub fn send64(&self, tag: u8, values: &[u64]) -> Result<(), String> {
        self.send(tag, &words64(values))
    }

    /// The parent's next message, which must carry `tag`.
    pub fn recv(&self, tag: u8) -> Result<Vec<u8>, String> {
        match recv_frame(self.rd) {
            Some((t, p)) if t == tag => Ok(p),
            Some((t, _)) => Err(format!("the parent sent message {:?}, expected {:?}", t as char, tag as char)),
            None => Err("the pipe from the parent closed".to_string()),
        }
    }

    /// The raw descriptors, for a child that keeps them across an exec.
    pub fn raw(&self) -> (u64, u64) {
        (self.rd.raw(), self.wr.raw())
    }

    /// End the child: report how its body ended and exit.
    pub fn finish(&self, outcome: Result<(), String>) -> ! {
        let _ = match outcome {
            Ok(()) => self.send(b'K', &[]),
            Err(msg) => self.send(b'F', msg.as_bytes()),
        };
        process::exit(0)
    }
}

/// A forked child and the parent's ends of its pipes. Dropping it kills and reaps the
/// child if the case has not.
pub struct Kid {
    pid: i32,
    rd: Fd,
    wr: Fd,
    reaped: bool,
}

impl Kid {
    pub fn send(&self, tag: u8, payload: &[u8]) -> R<()> {
        ensure(send_frame(self.wr, tag, payload), || "the pipe to the child closed".to_string())
    }

    /// The child's next message, which must carry `tag`; a child that failed reports why.
    pub fn recv(&self, tag: u8) -> R<Vec<u8>> {
        match recv_frame(self.rd) {
            Some((t, p)) if t == tag => Ok(p),
            Some((b'F', p)) => Err(fail_msg(format!("the child: {}", String::from_utf8_lossy(&p)))),
            Some((t, _)) => Err(fail_msg(format!("the child sent message {:?}, expected {:?}", t as char, tag as char))),
            None => Err(fail_msg(format!("the child ended before sending {:?}", tag as char))),
        }
    }

    pub fn recv64(&self, tag: u8) -> R<Vec<u64>> {
        self.recv(tag).map(|p| read64(&p))
    }

    /// Wait for the child's `K` and its exit.
    pub fn done(mut self) -> R<()> {
        self.recv(b'K')?;
        self.reap().map(drop)
    }

    /// Reap the child: its wait status.
    pub fn reap(&mut self) -> R<i32> {
        let mut status = 0i32;
        process::waitpid(self.pid, &mut status, 0).map_err(|e| fail_msg(format!("waitpid: {e:?}")))?;
        self.reaped = true;
        Ok(status)
    }

    pub fn kill(&self) -> R<()> {
        signal::kill(self.pid, SIGKILL).map_err(|e| fail_msg(format!("kill: {e:?}")))
    }
}

impl Drop for Kid {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = signal::kill(self.pid, SIGKILL);
            let _ = process::waitpid(self.pid, core::ptr::null_mut(), 0);
        }
        let _ = io::close(self.rd);
        let _ = io::close(self.wr);
    }
}

/// Fork a child that runs `body` and reports how it ended.
pub fn spawn(body: impl FnOnce(&Line) -> Result<(), String>) -> R<Kid> {
    let (up_rd, up_wr) = io::pipe().map_err(|e| fail_msg(format!("pipe: {e:?}")))?;
    let (down_rd, down_wr) = io::pipe().map_err(|e| fail_msg(format!("pipe: {e:?}")))?;
    match process::fork().map_err(|e| fail_msg(format!("fork: {e:?}")))? {
        ForkResult::Child => {
            let _ = io::close(up_rd);
            let _ = io::close(down_wr);
            let line = Line { rd: down_rd, wr: up_wr };
            let outcome = body(&line);
            line.finish(outcome)
        }
        ForkResult::Parent(pid) => {
            let _ = io::close(up_wr);
            let _ = io::close(down_rd);
            Ok(Kid { pid: pid.raw() as i32, rd: up_rd, wr: down_wr, reaped: false })
        }
    }
}

/// `Err` with `msg` unless `cond`, in a child.
fn need(cond: bool, msg: impl FnOnce() -> String) -> Result<(), String> {
    if cond {
        Ok(())
    } else {
        Err(msg())
    }
}

fn child<T>(r: R<T>) -> Result<T, String> {
    r.map_err(|e| match e {
        CaseError::Fail(m) => m,
        other => format!("{other:?}"),
    })
}

// ---------------------------------------------------------------------------------------
// Observation outside syscall 416.

/// A trace counter from `/proc/trace/counters`.
pub fn trace_counter(name: &str) -> R<u64> {
    let text = std::fs::read_to_string("/proc/trace/counters").map_err(|e| fail_msg(format!("reading /proc/trace/counters: {e}")))?;
    text.lines()
        .find_map(|line| {
            let (key, rest) = line.split_once(':')?;
            (key.trim() == name).then(|| rest.split_whitespace().next()?.parse::<u64>().ok()).flatten()
        })
        .ok_or_else(|| fail_msg(format!("/proc/trace/counters has no {name} (docs/gui/gpu.md section 1.16)")))
}

/// `gpu_contexts`, `gpu_resources` and `gpu_resource_bytes`.
fn gauges() -> R<[u64; 3]> {
    Ok([trace_counter("gpu_contexts")?, trace_counter("gpu_resources")?, trace_counter("gpu_resource_bytes")?])
}

fn show(g: [u64; 3]) -> String {
    format!("contexts {} resources {} bytes {}", g[0], g[1], g[2])
}

/// The gauges once earlier cases' work has drained: three equal readings 50 ms apart.
fn quiet_gauges() -> R<[u64; 3]> {
    let start = now_ns();
    let mut last = gauges()?;
    let mut same = 0;
    while same < 2 {
        ensure(ms_since(start) < 4_000, || format!("the GPU gauges did not settle within 4 s ({})", show(last)))?;
        let _ = time::sleep_ms(50);
        let g = gauges()?;
        same = if g == last { same + 1 } else { 0 };
        last = g;
    }
    Ok(last)
}

/// Poll the gauges until they equal `base`, for at most `ms`; the milliseconds it took.
fn back_to(base: [u64; 3], ms: u64, what: &str) -> R<u64> {
    let start = now_ns();
    loop {
        let g = gauges()?;
        if g == base {
            return Ok(ms_since(start));
        }
        if ms_since(start) > ms {
            return Err(fail_msg(format!("{what}: the gauges read {} after {} ms, expected {} within {ms} ms", show(g), ms_since(start), show(base))));
        }
        let _ = time::sleep_ms(5);
    }
}

/// This thread's kernel output count (section 1.16, BreenGL's section 22).
fn kernel_output() -> R<u64> {
    let pid = process::getpid().map_err(|e| fail_msg(format!("getpid: {e:?}")))?.raw();
    let tid = process::gettid().map_err(|e| fail_msg(format!("gettid: {e:?}")))?.raw();
    let path = format!("/proc/{pid}/task/{tid}/kernel_output");
    let text = std::fs::read_to_string(&path).map_err(|e| fail_msg(format!("reading {path}: {e}")))?;
    text.trim().parse::<u64>().map_err(|_| fail_msg(format!("{path} read {text:?}, not a decimal count")))
}

// ---------------------------------------------------------------------------------------
// Shared setup.

/// A context and a target of `w` by `h` with a frame drawn and completed: what the frame
/// must read back as.
fn drawn(fmt: u32, w: u32, h: u32, bg: u32) -> R<(u64, u64, Canvas)> {
    let ctx = context()?;
    let t = target(fmt, w, h)?;
    let rects = [([1, 1, 3, 3], RED), ([2, 0, 5, 2], BLUE)];
    frame(ctx, &scene(t, bg, &rects))?;
    Ok((ctx, t, scene_canvas(w, h, bg, &rects)))
}

#[path = "refusals.rs"]
mod refusals;
use refusals::refusals_for;

fn all_einval(h: u64, own_ctx: u64, what: &str) -> R<()> {
    for (op, r) in refusals_for(h, own_ctx) {
        ensure(r == -EINVAL, || format!("{op} with {what} ({h:#018x}) returned {}, expected EINVAL", errno(r)))?;
    }
    Ok(())
}

fn child_all_einval(h: u64, own_ctx: u64, what: &str) -> Result<(), String> {
    child(all_einval(h, own_ctx, what))
}

// ---------------------------------------------------------------------------------------
// The cases.

pub fn handle_private() -> CaseResult {
    let _case = begin()?;
    let (ctx, t, want) = drawn(format::B8G8R8A8, 8, 6, GREY)?;
    let mine = counters(0)?;
    let kid = spawn(|line| {
        let (c, ct, cw) = child(drawn(format::B8G8R8A8, 8, 6, TEAL))?;
        let tex = child(gpu::texture_create(format::R8, 4, 4).map_err(|e| fail_msg(format!("texture: {}", errno(e)))))?;
        let buf = child(gpu::buffer_create(64).map_err(|e| fail_msg(format!("buffer: {}", errno(e)))))?;
        line.send64(b'H', &[c, ct, tex, buf])?;
        line.recv(b'G')?;
        // The parent tried these numbers; they still name this process's own resources.
        child(expect(ct, &cw, "the child's target after the parent named its handles").map(drop))?;
        let k = child(counters(0))?;
        need(k.contexts == 1 && k.resources == 3, || format!("the child holds {} contexts and {} resources after the parent named them", k.contexts, k.resources))?;
        child(frame(c, &scene(ct, RED, &[])).map(drop))
    })?;
    let theirs = kid.recv64(b'H')?;
    for (h, what) in theirs.iter().zip(["the child's context", "the child's target", "the child's texture", "the child's buffer"]) {
        all_einval(*h, ctx, what)?;
    }
    kid.send(b'G', &[])?;
    kid.done()?;
    let after = counters(0)?;
    ensure(after.contexts == mine.contexts && after.resources == mine.resources && after.submissions == mine.submissions,
        || "naming another process's handles changed this process's counters".to_string())?;
    expect(t, &want, "this process's target after naming the child's handles")?;
    pass()
}

pub fn handle_guess() -> CaseResult {
    let _case = begin()?;
    let (ctx, t, want) = drawn(format::B8G8R8A8, 8, 6, GREY)?;
    let tex = gpu::texture_create(format::B8G8R8A8, 4, 4).map_err(|e| fail_msg(format!("texture: {}", errno(e))))?;
    let buf = gpu::buffer_create(256).map_err(|e| fail_msg(format!("buffer: {}", errno(e))))?;
    let own = [ctx, t, tex, buf];
    let mut guesses: Vec<u64> = (0..64).chain([u64::MAX, u64::MAX - 1, 1 << 32, 1 << 63, 0x100, 0x101, 0x102]).collect();
    for h in own {
        guesses.extend([h.wrapping_add(1), h.wrapping_sub(1), h ^ 1, h ^ (1 << 63), h.rotate_left(8), h >> 32, h & 0xFFFF_FFFF, !h]);
    }
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    for _ in 0..512 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        guesses.push(seed);
    }
    let before = counters(0)?;
    let mut tried = 0u64;
    for g in guesses.into_iter().filter(|g| !own.contains(g)) {
        for (op, r) in refusals_for(g, ctx) {
            // Target 0 names the display; this process is not its owner (ruling 42).
            let expected = if g == 0 && op == "READBACK of it" { -abi::errno::EPERM } else { -EINVAL };
            ensure(r == expected, || format!("{op} with a guessed handle ({g:#018x}) returned {}, expected {}", errno(r), errno(expected)))?;
        }
        tried += 1;
    }
    value("guesses", tried as i64, "", None);
    let after = counters(0)?;
    ensure(after.contexts == before.contexts && after.resources == before.resources,
        || format!("guessing handles changed the process's holdings: {} contexts, {} resources", after.contexts, after.resources))?;
    expect(t, &want, "the target after the guesses")?;
    pass()
}

pub fn fork_no_handles() -> CaseResult {
    let _case = begin()?;
    let (ctx, t, want) = drawn(format::B8G8R8A8, 8, 6, GREY)?;
    let tex = gpu::texture_create(format::B8G8R8A8, 4, 4).map_err(|e| fail_msg(format!("texture: {}", errno(e))))?;
    let kid = spawn(|_| {
        let k = child(counters(0))?;
        need(k.contexts == 0 && k.resources == 0 && k.resource_bytes == 0 && k.device_contexts == 0,
            || format!("the forked child starts with {} contexts, {} resources, {} resource bytes, {} device contexts",
                k.contexts, k.resources, k.resource_bytes, k.device_contexts))?;
        let c = child(context())?;
        for (h, what) in [(ctx, "the parent's context"), (t, "the parent's target"), (tex, "the parent's texture")] {
            child_all_einval(h, c, what)?;
        }
        Ok(())
    })?;
    kid.done()?;
    let k = counters(0)?;
    ensure(k.contexts == 1 && k.resources == 2, || format!("after the child the parent holds {} contexts, {} resources", k.contexts, k.resources))?;
    frame(ctx, &scene(t, GREY, &[([1, 1, 3, 3], RED), ([2, 0, 5, 2], BLUE)]))?;
    expect(t, &want, "the parent's target after the child")?;
    pass()
}

pub fn exec_reclaims() -> CaseResult {
    let _case = begin()?;
    let base = quiet_gauges()?;
    let kid = spawn(|line| {
        let (c, t, _) = child(drawn(format::B8G8R8A8, 64, 64, GREY))?;
        let tex = child(gpu::texture_create(format::B8G8R8A8, 256, 256).map_err(|e| fail_msg(format!("texture: {}", errno(e)))))?;
        let buf = child(gpu::buffer_create(1 << 20).map_err(|e| fail_msg(format!("buffer: {}", errno(e)))))?;
        line.send(b'C', &[])?;
        line.recv(b'E')?;
        let (rd, wr) = line.raw();
        let mut args: Vec<String> = vec![EXEC_PROBE_BINARY.to_string(), rd.to_string(), wr.to_string()];
        args.extend([c, t, tex, buf].iter().map(|h| format!("{h:x}")));
        let cs: Vec<Vec<u8>> = args.iter().map(|a| { let mut v = a.as_bytes().to_vec(); v.push(0); v }).collect();
        let mut argv: Vec<*const u8> = cs.iter().map(|v| v.as_ptr()).collect();
        argv.push(core::ptr::null());
        let path = format!("{EXEC_PROBE_BINARY}\0");
        let e = process::execv(path.as_bytes(), argv.as_ptr());
        Err(format!("exec of {EXEC_PROBE_BINARY} failed: {e:?}"))
    })?;
    kid.recv(b'C')?;
    let held = gauges()?;
    ensure(held[0] > base[0] && held[1] > base[1] && held[2] > base[2],
        || format!("with the child's GPU state the gauges read {}, not above {}", show(held), show(base)))?;
    kid.send(b'E', &[])?;
    // The probe runs in the exec'd image, which created and destroyed one context of its own.
    kid.recv(b'X')?;
    let ms = back_to(base, K_MS, "after the child's exec")?;
    value("reclaimed-ms", ms as i64, "ms", Some((0, K_MS as i64)));
    kid.send(b'Q', &[])?;
    kid.done()
}

pub fn exit_reclaims() -> CaseResult {
    let _case = begin()?;
    let base = quiet_gauges()?;
    let mut kid = spawn(|line| {
        let (_c, _t, _) = child(drawn(format::B8G8R8A8, 64, 64, GREY))?;
        let _tex = child(gpu::texture_create(format::B8G8R8A8, 512, 512).map_err(|e| fail_msg(format!("texture: {}", errno(e)))))?;
        let _buf = child(gpu::buffer_create(1 << 20).map_err(|e| fail_msg(format!("buffer: {}", errno(e)))))?;
        line.send(b'C', &[])?;
        line.recv(b'E')?;
        Ok(())
    })?;
    kid.recv(b'C')?;
    let held = gauges()?;
    ensure(held[0] > base[0] && held[1] > base[1] && held[2] >= base[2] + (1 << 20),
        || format!("with the child's GPU state the gauges read {}, against {}", show(held), show(base)))?;
    kid.send(b'E', &[])?;
    kid.recv(b'K')?;
    let status = kid.reap()?;
    ensure(process::wifexited(status) && process::wexitstatus(status) == 0, || format!("the child ended with status {status:#x}"))?;
    let ms = back_to(base, K_MS, "after the child exited and was reaped")?;
    value("reclaimed-ms", ms as i64, "ms", Some((0, K_MS as i64)));
    pass()
}

pub fn kill_mid_submit() -> CaseResult {
    let _case = begin()?;
    let (ctx, t, _) = drawn(format::B8G8R8A8, 32, 32, GREY)?;
    let mut kid = spawn(|line| {
        let c = child(context())?;
        let c2 = child(context())?;
        let ct = child(target(format::B8G8R8A8, 512, 512))?;
        let mut n = 0u32;
        loop {
            let rects: Vec<[i32; 4]> = (0..256).map(|i| [i, i, 512 - i, 512 - i]).collect();
            let mut s = Vec::new();
            encode::begin(&mut s, ct);
            encode::clear(&mut s, GREY);
            encode::draw(&mut s, pipeline::SOLID, RED.wrapping_add(n), &rects);
            let (r, _) = gpu::submit(if n % 2 == 0 { c } else { c2 }, &s);
            need(r == 0, || format!("submission {n} returned {}", errno(r)))?;
            n += 1;
            if n == 8 {
                line.send(b'S', &[])?;
            }
        }
    })?;
    kid.recv(b'S')?;
    let _ = time::sleep_ms(30);
    kid.kill()?;
    let status = kid.reap()?;
    ensure(process::wifsignaled(status) && process::wtermsig(status) == SIGKILL, || format!("the child ended with status {status:#x}, not by SIGKILL"))?;
    // The device and this process's context keep working.
    let rects = [([0, 0, 16, 16], BLUE), ([8, 8, 30, 20], RED)];
    frame(ctx, &scene(t, TEAL, &rects))?;
    expect(t, &scene_canvas(32, 32, TEAL, &rects), "this process's frame after the child was killed mid-submit")?;
    let k = counters(ctx)?;
    ensure(k.state == context_state::LIVE && k.device_errors == 0, || format!("this process's context is in state {} with {} device errors", k.state, k.device_errors))?;
    let (c2, t2, want2) = drawn(format::R8G8B8A8, 9, 7, GREEN)?;
    expect(t2, &want2, "a new context after the kill")?;
    ensure(counters(c2)?.state == context_state::LIVE, || "a new context is not live".to_string())?;
    pass()
}

pub fn killed_work_drains() -> CaseResult {
    let _case = begin()?;
    let base = quiet_gauges()?;
    let mut kid = spawn(|line| {
        let c = child(context())?;
        let ct = child(target(format::B8G8R8A8, 128, 128))?;
        let _buf = child(gpu::buffer_create(1 << 20).map_err(|e| fail_msg(format!("buffer: {}", errno(e)))))?;
        line.send(b'R', &[])?;
        line.recv(b'A')?;
        // The parent armed withhold-fence: this submission's completion is ignored, so its
        // work stays outstanding until the watchdog ends it.
        let (r, _) = gpu::submit(c, &scene(ct, RED, &[]));
        need(r == 0, || format!("SUBMIT returned {}", errno(r)))?;
        line.send(b'S', &[])?;
        loop {
            let _ = time::sleep_ms(1_000);
        }
    })?;
    kid.recv(b'R')?;
    arm(test_point::WITHHOLD_FENCE, test_point::WITHHOLD_ONE)?;
    kid.send(b'A', &[])?;
    kid.recv(b'S')?;
    let sent = now_ns();
    kid.kill()?;
    let status = kid.reap()?;
    ensure(process::wifsignaled(status), || format!("the child ended with status {status:#x}, not by a signal"))?;
    // Until the withheld work's fence completes, by the watchdog W after it was sent, the
    // killed process's context and the target its work holds stay.
    let mut last = gauges()?;
    while ms_since(sent) < W_MS - 20 {
        last = gauges()?;
        ensure(last[0] > base[0] && last[1] > base[1],
            || format!("{} ms after the killed child's work was sent, before its fence completed, the gauges read {} against {}",
                ms_since(sent), show(last), show(base)))?;
        let _ = time::sleep_ms(20);
    }
    value("held-until-ms", ms_since(sent) as i64, "ms", None);
    // The watchdog completes the fence within [W, W + 2K]; everything goes within K after.
    let left = (W_MS + 3 * K_MS).saturating_sub(ms_since(sent));
    back_to(base, left, "after the killed child's withheld work was ended by the watchdog")?;
    let ms = ms_since(sent);
    value("drained-ms", ms as i64, "ms", Some((W_MS as i64, (W_MS + 3 * K_MS) as i64)));
    ensure(fired(test_point::WITHHOLD_FENCE) >= 1, || "withhold-fence never fired".to_string())?;
    let _ = last;
    pass()
}

/// Textures of 4096 by 4096 B8G8R8A8: 64 MiB of backing each.
struct Held(Vec<u64>);

impl Drop for Held {
    fn drop(&mut self) {
        for t in self.0.drain(..) {
            let _ = gpu::resource_destroy(t);
        }
    }
}

/// A stream of `bytes` (a multiple of 16) that validates: a BEGIN of `t` and a clear, then
/// SCISSOR commands of the whole target.
fn filler(t: u64, w: i32, h: i32, bytes: usize) -> Vec<u32> {
    let words = bytes / 4;
    let mut s = Vec::with_capacity(words);
    encode::begin(&mut s, t);
    encode::clear(&mut s, GREY);
    let left = words - s.len();
    // SCISSOR is 6 words, BEGIN 4 and CLEAR 3: the remainder after the scissors is made
    // of BEGINs and CLEARs (1 = 4 + 3 - 6, 2 = 4 + 4 - 6, 3, 4, 5 = 4 + 4 + 3 - 6).
    let tail: &[u32] = match left % 6 {
        0 => &[],
        1 => &[4, 3],
        2 => &[4, 4],
        3 => &[3],
        4 => &[4],
        _ => &[4, 4, 3],
    };
    let tail_words: usize = tail.iter().map(|&n| n as usize).sum();
    for _ in 0..(left - tail_words) / 6 {
        s.extend_from_slice(&[stream::cmd::SCISSOR, 6, 0, 0, w as u32, h as u32]);
    }
    for &n in tail {
        if n == 4 {
            encode::begin(&mut s, t);
        } else {
            encode::clear(&mut s, GREY);
        }
    }
    s
}

pub fn quota_memory() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = target(format::B8G8R8A8, 64, 64)?;
    frame(ctx, &scene(t, GREY, &[]))?;
    // 1024 x 512 B8G8R8A8: a readback of it stages 2 MiB, more than the room left below.
    let wide = target(format::B8G8R8A8, 1024, 512)?;
    frame(ctx, &scene(wide, RED, &[]))?;
    let base = counters(0)?.resource_bytes;
    const ONE: u64 = 64 << 20;
    let mut held = Held(Vec::new());
    while base + (held.0.len() as u64 + 1) * ONE <= QUOTA {
        held.0.push(gpu::texture_create(format::B8G8R8A8, 4096, 4096).map_err(|e| fail_msg(format!("a 64 MiB texture under the quota: {}", errno(e))))?);
    }
    // Fill the rest to within 1 MiB of the quota with 1 MiB buffers.
    let mut small = Held(Vec::new());
    while counters(0)?.resource_bytes + (2 << 20) <= QUOTA {
        small.0.push(gpu::buffer_create(1 << 20).map_err(|e| fail_msg(format!("a 1 MiB buffer under the quota: {}", errno(e))))?);
    }
    let full = counters(0)?.resource_bytes;
    let room = QUOTA - full;
    ensure(room >= 1 << 20 && room < 2 << 20, || format!("{room} bytes of room left; the case needs between 1 and 2 MiB"))?;
    // A resource past the quota.
    let r = gpu::buffer_create(room + 4096);
    ensure(r == Err(-abi::errno::ENOMEM), || format!("a buffer of {} bytes with {room} bytes of quota left: {r:?}", room + 4096))?;
    // A submission whose stream copy cannot be charged.
    let before = counters(ctx)?;
    let big = filler(t, 64, 64, (room as usize + 4096).next_multiple_of(16));
    let (r, a) = gpu::submit(ctx, &big);
    ensure(r == -abi::errno::ENOMEM && a.error == stream::error::NO_MEMORY && a.error_index == 0,
        || format!("a {}-byte stream with {room} bytes of quota left returned {} (command {} error {})", big.len() * 4, errno(r), a.error_index, a.error))?;
    let after = counters(ctx)?;
    ensure(after.submissions == before.submissions, || "the refused submission was counted".to_string())?;
    let k = counters(0)?;
    ensure(k.resource_bytes == full && k.copy_bytes == 0, || format!("the refused submission left {} resource bytes and {} copy bytes charged", k.resource_bytes - full, k.copy_bytes))?;
    // A readback whose staging cannot be charged (ruling 18).
    let mut px = vec![0u8; 1024 * 512 * 4];
    let mut rec = [0u8; record::SIZE];
    let r = gpu::readback(wide, [0, 0, 1024, 512], &mut px, 4096, &mut rec);
    ensure(r == -abi::errno::ENOMEM, || format!("a 2 MiB readback with {room} bytes of quota left returned {}", errno(r)))?;
    drop(px);
    // Freed quota is usable again by the same submission.
    drop(small);
    let fence = submit(ctx, &big)?;
    wait_ok(ctx, fence, 2_000)?;
    expect(t, &scene_canvas(64, 64, GREY, &[]), "the stream's frame once the quota had room")?;
    pass()
}

pub fn quota_handles() -> CaseResult {
    let _case = begin()?;
    let base = counters(0)?.resources as usize;
    let mut held = Held(Vec::new());
    for i in base..256 {
        held.0.push(gpu::buffer_create(16).map_err(|e| fail_msg(format!("resource handle {i} of the 256 quota: {}", errno(e))))?);
    }
    let r = gpu::buffer_create(16);
    ensure(r == Err(-EAGAIN), || format!("resource handle 257: {r:?}"))?;
    let r = gpu::target_create(format::B8G8R8A8, 1, 1);
    ensure(r == Err(-EAGAIN), || format!("a target past the handle quota: {r:?}"))?;
    let last = held.0.pop().ok_or_else(|| fail_msg("no handle".to_string()))?;
    ensure(gpu::resource_destroy(last) == 0, || "RESOURCE_DESTROY".to_string())?;
    held.0.push(gpu::buffer_create(16).map_err(|e| fail_msg(format!("after one was freed: {}", errno(e))))?);
    let mut ctxs = Vec::new();
    for i in 0..limits::CONTEXTS {
        ctxs.push(gpu::context_create().map_err(|e| fail_msg(format!("context {i} of the quota: {}", errno(e))))?);
    }
    let r = gpu::context_create();
    ensure(r == Err(-EAGAIN), || format!("a context past the quota of {}: {r:?}", limits::CONTEXTS))?;
    let k = counters(0)?;
    ensure(k.contexts == limits::CONTEXTS as u64 && k.resources == 256, || format!("the process holds {} contexts and {} resources", k.contexts, k.resources))?;
    pass()
}

pub fn quota_contexts() -> CaseResult {
    let _case = begin()?;
    let mut ctxs = Vec::new();
    for i in 0..limits::CONTEXTS {
        ctxs.push(gpu::context_create().map_err(|e| fail_msg(format!("context {i} of the quota: {}", errno(e))))?);
    }
    ensure(gpu::context_create() == Err(-EAGAIN), || "a ninth context was created".to_string())?;
    let kid = spawn(|_| {
        let mut theirs = Vec::new();
        for i in 0..limits::CONTEXTS {
            theirs.push(gpu::context_create().map_err(|e| format!("the other process's context {i}: {}", errno(e)))?);
        }
        need(gpu::context_create() == Err(-EAGAIN), || "the other process got a ninth context".to_string())?;
        let t = child(target(format::B8G8R8A8, 8, 8))?;
        child(frame(theirs[7], &scene(t, RED, &[([2, 2, 6, 6], BLUE)])).map(drop))?;
        child(expect(t, &scene_canvas(8, 8, RED, &[([2, 2, 6, 6], BLUE)]), "the other process's frame").map(drop))
    })?;
    kid.done()?;
    ensure(gpu::context_create() == Err(-EAGAIN), || "the quota changed after the other process".to_string())?;
    pass()
}

pub fn work_estimate_refused() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    // gradient_v weighs 32,768 steps a fragment (gpu-abi.md, G5): 256 x 256 of them is 2^31.
    let t = target(format::B8G8R8A8, 256, 256)?;
    let mut params = [0u32; 16];
    params[..3].copy_from_slice(&[0, 256, BLUE]);
    let mut at_cap = Vec::new();
    encode::begin(&mut at_cap, t);
    abi::draw::encode(&mut at_cap, pipeline::GRADIENT_V, RED, 0, 0, 0, &[abi::draw::Instance { rect: [0, 0, 256, 256], params }]);
    let (r, a) = gpu::submit(ctx, &at_cap);
    ensure(r == 0 && a.estimate == limits::WORK, || format!("a stream of exactly 2^31 steps returned {} with estimate {}", errno(r), a.estimate))?;
    wait_ok(ctx, a.fence, 2_000)?;
    let before = counters(ctx)?;
    let mut over = at_cap.clone();
    encode::draw(&mut over, pipeline::SOLID, GREEN, &[[0, 0, 1, 1]]);
    let (r, a) = gpu::submit(ctx, &over);
    ensure(r == -EDQUOT && a.error == stream::error::WORK_LIMIT && a.error_index == 2,
        || format!("one step over the cap returned {} (command {} error {})", errno(r), a.error_index, a.error))?;
    let after = counters(ctx)?;
    ensure(after.submissions == before.submissions && after.work_estimate == before.work_estimate,
        || "the refused stream was counted".to_string())?;
    pass()
}

pub fn foreign_target_draw() -> CaseResult {
    let _case = begin()?;
    let (_ctx, t, want) = drawn(format::B8G8R8A8, 8, 6, GREY)?;
    let (_, rec) = read(t, 8, 6)?;
    let generation = u64_at(&rec, record::GENERATION);
    let kid = spawn(|_| {
        let c = child(context())?;
        let own = child(target(format::B8G8R8A8, 8, 6))?;
        let tex = child(gpu::texture_create(format::B8G8R8A8, 8, 6).map_err(|e| fail_msg(format!("texture: {}", errno(e)))))?;
        let mut streams: Vec<(&str, Vec<u32>, u32)> = Vec::new();
        streams.push(("BEGIN of the parent's target", scene(t, RED, &[]), 0));
        let mut s = scene(own, RED, &[]);
        encode::copy(&mut s, own, [0, 0, 8, 6], t, [0, 0]);
        streams.push(("COPY into the parent's target", s, 2));
        let mut s = Vec::new();
        encode::copy(&mut s, tex, [0, 0, 8, 6], t, [0, 0]);
        streams.push(("COPY of a texture into the parent's target", s, 0));
        for (what, s, index) in streams {
            let (r, a) = gpu::submit(c, &s);
            need(r == -EINVAL && a.error == stream::error::BAD_HANDLE && a.error_index == index,
                || format!("{what} returned {} (command {} error {}), expected EINVAL code 4 at {index}", errno(r), a.error_index, a.error))?;
        }
        let k = child(counters(c))?;
        need(k.submissions == 0 && k.draws == 0, || format!("the refused streams counted {} submissions", k.submissions))
    })?;
    kid.done()?;
    let rec = expect(t, &want, "the parent's target after another process drew to it")?;
    ensure(u64_at(&rec, record::GENERATION) == generation, || "the parent's target's generation changed".to_string())?;
    pass()
}

pub fn foreign_readback() -> CaseResult {
    let _case = begin()?;
    let (ctx, t, _) = drawn(format::B8G8R8A8, 8, 6, GREY)?;
    let readbacks = counters(0)?.readbacks;
    let kid = spawn(|_| {
        let _c = child(context())?;
        let mut px = vec![0xA5u8; 8 * 6 * 4];
        let mut rec = [0x5Au8; record::SIZE];
        let r = gpu::readback(t, [0, 0, 8, 6], &mut px, 32, &mut rec);
        need(r == -EINVAL, || format!("READBACK of the parent's target returned {}", errno(r)))?;
        need(px.iter().all(|&b| b == 0xA5) && rec.iter().all(|&b| b == 0x5A), || "the refused readback wrote the caller's memory".to_string())?;
        let k = child(counters(0))?;
        need(k.readbacks == 0, || format!("the refused readback counted {}", k.readbacks))
    })?;
    kid.done()?;
    ensure(counters(0)?.readbacks == readbacks, || "the other process's readback counted toward the owner".to_string())?;
    let _ = ctx;
    pass()
}

pub fn display_context_unreachable() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = scanout_target(format::B8G8R8A8, 16, 12)?;
    let rects = [([2, 2, 10, 8], BLUE)];
    frame(ctx, &scene(t, GREY, &rects))?;
    graphics::take_over_display().map_err(|e| fail_msg(format!("take display: {e:?}")))?;
    let (r, p) = gpu::present(ctx, t);
    ensure(r == 0, || format!("PRESENT returned {}", errno(r)))?;
    let rec = expect(t, &scene_canvas(16, 12, GREY, &rects), "the owner's scanout")?;
    let (dev_ctx, dev_res) = (u64_at(&rec, record::CONTEXT), u64_at(&rec, record::RESOURCE));
    let kid = spawn(|_| {
        let c = child(context())?;
        for (h, what) in [(ctx, "the display owner's context"), (t, "the display owner's scanout target"),
                          (dev_ctx, "the display owner's device context id"), (dev_res, "the display owner's device resource id")] {
            child_all_einval(h, c, what)?;
        }
        let tex = child(target(format::B8G8R8A8, 16, 12))?;
        child(frame(c, &scene(tex, RED, &[])).map(drop))?;
        let mut s = scene(tex, RED, &[]);
        encode::draw_sampled(&mut s, pipeline::COPY, t, &[[0, 0, 16, 12, 0, 0]]);
        let (r, a) = gpu::submit(c, &s);
        need(r == -EINVAL && a.error == stream::error::BAD_HANDLE && a.error_index == 2,
            || format!("sampling the owner's scanout returned {} (command {} error {})", errno(r), a.error_index, a.error))?;
        let (r, _) = gpu::present(c, t);
        need(r == -abi::errno::EPERM || r == -EINVAL, || format!("PRESENT of the owner's target returned {}", errno(r)))?;
        Ok(())
    })?;
    kid.done()?;
    let rec = expect(t, &scene_canvas(16, 12, GREY, &rects), "the owner's scanout after the client")?;
    ensure(u32_at(&rec, record::FLAGS) & 1 == 1 && u64_at(&rec, record::PRESENT_FENCE) == p.fence,
        || "the owner's scanout binding changed".to_string())?;
    pass()
}

/// A handle must not have an address's form (section 1.16).
fn handle_form(h: u64, what: &str) -> R<()> {
    let top = h >> 56;
    ensure(top != 0 && top != 0xFF, || format!("{what} {h:#018x} has the form of an address (top byte {top:#04x})"))
}

/// A value of a kind that is a count, serial or id: below 2^32.
fn small(v: u64, what: &str) -> R<()> {
    ensure(v < 1 << 32, || format!("{what} is {v:#x}, not a count, serial or device id below 2^32"))
}

pub fn no_addresses() -> CaseResult {
    let _case = begin()?;
    let mut a = abi::ContextCreate::ZERO;
    let r = gpu::call_with(abi::op::CONTEXT_CREATE, &mut a);
    ensure(r == 0, || format!("CONTEXT_CREATE returned {}", errno(r)))?;
    let ctx = a.handle;
    handle_form(ctx, "a context handle")?;
    small(a.warmed as u64, "CONTEXT_CREATE's warmed")?;
    warmed(ctx)?;
    for _ in 0..6 {
        let h = gpu::context_create().map_err(|e| fail_msg(format!("CONTEXT_CREATE: {}", errno(e))))?;
        handle_form(h, "a context handle")?;
        let _ = gpu::context_destroy(h);
    }
    let t = scanout_target(format::B8G8R8A8, 8, 8)?;
    let tex = gpu::texture_create(format::R8, 8, 8).map_err(|e| fail_msg(format!("texture: {}", errno(e))))?;
    let buf = gpu::buffer_create(4096).map_err(|e| fail_msg(format!("buffer: {}", errno(e))))?;
    for (h, what) in [(t, "a target handle"), (tex, "a texture handle"), (buf, "a buffer handle")] {
        handle_form(h, what)?;
    }
    for _ in 0..200 {
        let b = gpu::buffer_create(16).map_err(|e| fail_msg(format!("buffer: {}", errno(e))))?;
        handle_form(b, "a buffer handle")?;
        let _ = gpu::resource_destroy(b);
    }
    let (r, _) = gpu::upload(ctx, tex, [0, 0, 8, 8], &[7u8; 64], 8, abi::upload_source::R8, abi::purpose::IMAGE);
    ensure(r == 0, || format!("UPLOAD returned {}", errno(r)))?;
    // Fences are ns << 32 | n: one namespace per context, n counting its fenced operations.
    let mut ns = None;
    for n in 1..=3u64 {
        let (r, s) = gpu::submit(ctx, &scene(t, RED, &[([1, 1, 3, 3], BLUE)]));
        ensure(r == 0, || format!("SUBMIT returned {}", errno(r)))?;
        small(s.estimate, "SUBMIT's estimate")?;
        ensure(s.error == 0 && s.error_index == 0, || "SUBMIT wrote a refusal".to_string())?;
        ensure(s.fence & 0xFFFF_FFFF == n, || format!("fence {:#x} of the context's submission {n}", s.fence))?;
        let space = s.fence >> 32;
        ensure(*ns.get_or_insert(space) == space, || "one context's fences changed namespace".to_string())?;
        small(space, "a fence's namespace")?;
        let (r, w) = gpu::wait(ctx, s.fence, 2_000_000_000);
        ensure(r == 0 && w.status == 0 && w.answer == 0, || format!("WAIT returned {} status {} answer {}", errno(r), w.status, w.answer))?;
    }
    graphics::take_over_display().map_err(|e| fail_msg(format!("take display: {e:?}")))?;
    let (r, p) = gpu::present(ctx, t);
    ensure(r == 0 && p.fence == ns.unwrap_or(0) << 32 | 4, || format!("PRESENT returned {} with fence {:#x}", errno(r), p.fence))?;
    small(p.answer as u64, "PRESENT's answer")?;
    let (_, rec) = read(t, 8, 8)?;
    let (_, rec2) = read(t, 8, 8)?;
    ensure(u64_at(&rec, record::TARGET) == t, || "the record's target is not the caller's handle".to_string())?;
    for (at, what) in [(record::SERIAL, "the record's serial"), (record::CONTEXT, "the record's device context id"),
                       (record::RESOURCE, "the record's device resource id"), (record::GENERATION, "the record's generation"),
                       (record::RENDER_SERIAL, "the record's render serial"), (record::RENDER_FENCE, "the record's render fence"),
                       (record::READBACK_FENCE, "the record's readback fence"), (record::READBACK_SURFACE, "the record's readback surface"),
                       (record::READBACK_MOB, "the record's readback MOB"), (record::COMMIT, "the record's commit"),
                       (record::COMPOSITE_SERIAL, "the record's composite serial")] {
        small(u64_at(&rec, at), what)?;
    }
    ensure(u64_at(&rec, record::PRESENT_FENCE) == p.fence, || "the record's present fence is not PRESENT's".to_string())?;
    ensure(u64_at(&rec2, record::SERIAL) == u64_at(&rec, record::SERIAL) + 1, || "two readbacks' serials do not count by one".to_string())?;
    ensure(u64_at(&rec2, record::GENERATION) == u64_at(&rec, record::GENERATION), || "a readback changed the generation".to_string())?;
    let (_, trec) = read_r8(tex)?;
    small(u64_at(&trec, record::RENDER_FENCE), "the texture record's render fence")?;
    for c in [counters(0)?, counters(ctx)?] {
        for (v, what) in [(c.context, "COUNTERS' context")] {
            ensure(v == 0 || v == ctx, || format!("{what} is {v:#x}"))?;
        }
        for (v, what) in [(c.submissions, "submissions"), (c.draws, "draws"), (c.instances, "instances"), (c.fragment_area, "fragment area"),
                          (c.work_estimate, "work estimate"), (c.uploads_image, "image uploads"), (c.uploads_fixture, "fixture uploads"),
                          (c.readbacks, "readbacks"), (c.ring_waits, "ring waits"), (c.device_errors, "device errors"), (c.state, "state"),
                          (c.contexts, "contexts"), (c.device_contexts, "device contexts"), (c.resources, "resources"),
                          (c.resource_bytes, "resource bytes"), (c.warmed, "warmed")] {
            small(v, &format!("COUNTERS' {what}"))?;
        }
    }
    let _ = buf;
    pass()
}

fn read_r8(tex: u64) -> R<(Vec<u8>, [u8; record::SIZE])> {
    let mut px = vec![0u8; 64];
    let mut rec = [0u8; record::SIZE];
    let r = gpu::readback(tex, [0, 0, 8, 8], &mut px, 8, &mut rec);
    ensure(r == 0, || format!("READBACK of the R8 texture returned {}", errno(r)))?;
    Ok((px, rec))
}

/// xorshift64.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn coord(&mut self, max: i32) -> i32 {
        self.below(max as u64 + 1) as i32
    }
}

/// The resources the fuzzed streams name.
struct FuzzSet {
    target: u64,
    rgba: u64,
    texture: u64,
    buffer: u64,
}

/// One valid command for the current state: whether a target is current.
fn valid_command(g: &mut Rng, f: &FuzzSet, has_target: bool, s: &mut Vec<u32>) -> bool {
    let choice = if has_target { g.below(7) } else { 0 };
    match choice {
        0 => {
            if g.below(2) == 0 {
                encode::begin(s, f.target);
                return true;
            }
            // COPY needs no current target.
            encode::copy(s, f.texture, [g.below(8) as u32, g.below(8) as u32, 8, 8], f.target, [g.below(32) as u32, g.below(32) as u32]);
        }
        1 => encode::clear(s, g.next() as u32),
        2 => {
            let n = 1 + g.below(4) as usize;
            let rects: Vec<[i32; 4]> = (0..n).map(|_| { let (x, y) = (g.coord(40) - 4, g.coord(40) - 4); [x, y, x + g.coord(16), y + g.coord(16)] }).collect();
            encode::draw(s, pipeline::SOLID, g.next() as u32, &rects);
        }
        3 => {
            let (x, y) = (g.coord(30), g.coord(30));
            s.extend_from_slice(&[stream::cmd::SCISSOR, 6, x as u32, y as u32, (x + g.coord(30)) as u32, (y + g.coord(30)) as u32]);
        }
        4 => encode::draw_buffer(s, pipeline::SOLID, g.next() as u32, f.buffer, 16 * g.below(8) as u32, 1 + g.below(8) as u32),
        5 => encode::draw_sampled(s, pipeline::COPY, f.texture, &[[g.coord(20), g.coord(20), 20 + g.coord(10), 20 + g.coord(10), 0, 0]]),
        _ => {
            let mut params = [0u32; 16];
            params[..3].copy_from_slice(&[g.coord(32) as u32, g.coord(32) as u32, g.below(12) as u32]);
            abi::draw::encode(s, pipeline::CIRCLE_FILL, g.next() as u32, 0, 0, 0, &[abi::draw::Instance { rect: [0, 0, 32, 32], params }]);
        }
    }
    has_target
}

/// One malformed command for the current state, appended to `s`, with the code its
/// refusal must name; `last` is whether the command must end the stream.
fn malformed(g: &mut Rng, f: &FuzzSet, has_target: bool, s: &mut Vec<u32>) -> (u32, bool) {
    use stream::error as e;
    loop {
        match g.below(14) {
            0 => {
                let code = [0u32, 10, 11, 64, 0x7FFF_FFFF, u32::MAX][g.below(6) as usize];
                s.extend_from_slice(&[code, 2]);
                return (e::UNKNOWN_COMMAND, false);
            }
            1 => {
                s.extend_from_slice(&[stream::cmd::CLEAR, g.below(2) as u32]);
                return (e::TRUNCATED, true);
            }
            2 => {
                s.extend_from_slice(&[stream::cmd::DRAW, 9 + 4 * (1 + g.below(4) as u32), pipeline::SOLID, 0, 1, 0, 0, 1, 1]);
                return (e::TRUNCATED, true);
            }
            3 if !has_target => {
                encode::draw(s, pipeline::SOLID, 0, &[[0, 0, 1, 1]]);
                return (e::NO_TARGET, false);
            }
            4 => {
                let h = [g.next() | 1 << 56, f.texture, f.buffer, 0][g.below(4) as usize];
                encode::begin(s, h);
                return (e::BAD_HANDLE, false);
            }
            5 if has_target => {
                encode::draw(s, 17 + g.below(1_000) as u32, 0, &[[0, 0, 1, 1]]);
                return (e::BAD_PIPELINE, false);
            }
            6 if has_target => {
                let x = g.coord(20);
                encode::draw(s, pipeline::SOLID, 0, &[[0, 0, 4, 4], [x + 1, 0, x, 4]]);
                return (e::BAD_RECT, false);
            }
            7 => {
                s.extend_from_slice(&[stream::cmd::BEGIN, 5, f.target as u32, (f.target >> 32) as u32, 0]);
                return (e::BAD_LENGTH, false);
            }
            8 if has_target => {
                s.extend_from_slice(&[stream::cmd::DRAW, 9, pipeline::SOLID, 0, 2, 0, 0, 1, 1]);
                return (e::BAD_LENGTH, false);
            }
            9 if has_target => {
                let offset = if g.below(2) == 0 { 8 } else { 4096 };
                encode::draw_buffer(s, pipeline::SOLID, 0, f.buffer, offset, 1);
                return (e::BAD_RANGE, false);
            }
            10 => {
                encode::copy(s, f.rgba, [0, 0, 4, 4], f.target, [0, 0]);
                return (e::FORMAT_MISMATCH, false);
            }
            11 if has_target => {
                encode::draw(s, pipeline::COPY, 0, &[[0, 0, 1, 1]]);
                return (e::WRONG_COMMAND, false);
            }
            12 if has_target => {
                encode::draw_sampled(s, pipeline::COPY, f.target, &[[0, 0, 4, 4, 0, 0]]);
                return (e::SAMPLES_TARGET, false);
            }
            13 if has_target => {
                let mut params = [0u32; 16];
                params[..4].copy_from_slice(&[4, 4, 2, 1 + g.below(9) as u32]);
                abi::draw::encode(s, pipeline::CIRCLE_FILL, 0, 0, 0, 0, &[abi::draw::Instance { rect: [0, 0, 8, 8], params }]);
                return (abi::draw::PARAMETERS, false);
            }
            _ => {}
        }
    }
}

pub fn fuzz_streams() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let f = FuzzSet {
        target: target(format::B8G8R8A8, 32, 32)?,
        rgba: target(format::R8G8B8A8, 8, 8)?,
        texture: gpu::texture_create(format::B8G8R8A8, 16, 16).map_err(|e| fail_msg(format!("texture: {}", errno(e))))?,
        buffer: gpu::buffer_create(256).map_err(|e| fail_msg(format!("buffer: {}", errno(e))))?,
    };
    let rects: Vec<u8> = (0..16u32).flat_map(|i| [i, i, i + 4, i + 3]).flat_map(|v| v.to_le_bytes()).collect();
    let (r, _) = gpu::upload(ctx, f.buffer, [0, 0, 256, 1], &rects, 256, abi::upload_source::R8, abi::purpose::FIXTURE);
    ensure(r == 0, || format!("UPLOAD into the buffer returned {}", errno(r)))?;
    let (r, _) = gpu::upload(ctx, f.texture, [0, 0, 16, 16], &[0x5Au8; 16 * 16 * 4], 64, abi::upload_source::B8G8R8A8, abi::purpose::FIXTURE);
    ensure(r == 0, || format!("UPLOAD into the texture returned {}", errno(r)))?;
    let rects_drawn = [([4, 4, 20, 12], BLUE)];
    frame(ctx, &scene(f.target, GREY, &rects_drawn))?;
    frame(ctx, &scene(f.rgba, RED, &[]))?;
    let before = counters(ctx)?;
    let mut g = Rng(0x6A09_E667_F3BC_C908);
    const STREAMS: u32 = 10_000;
    let start = now_ns();
    for i in 0..STREAMS {
        let mut s = Vec::new();
        let mut has_target = false;
        let prefix = g.below(6) as u32;
        for _ in 0..prefix {
            has_target = valid_command(&mut g, &f, has_target, &mut s);
        }
        let index = {
            let mut n = 0u32;
            let mut at = 0usize;
            while at < s.len() {
                at += s[at + 1] as usize;
                n += 1;
            }
            n
        };
        let (code, last) = malformed(&mut g, &f, has_target, &mut s);
        if !last {
            for _ in 0..g.below(3) {
                valid_command(&mut g, &f, true, &mut s);
            }
        }
        let (r, a) = gpu::submit(ctx, &s);
        ensure(r == -EINVAL && a.error_index == index && a.error == code && a.fence == 0,
            || format!("stream {i} (seed 0x6A09E667F3BCC908, {} words) returned {} at command {} with code {}, expected EINVAL at command {index} with code {code}: {:x?}",
                s.len(), errno(r), a.error_index, a.error, s))?;
    }
    value("fuzz-ms", ms_since(start) as i64, "ms", None);
    let after = counters(ctx)?;
    ensure(after.submissions == before.submissions && after.state == context_state::LIVE && after.device_errors == 0,
        || format!("after the fuzzed streams: {} submissions (from {}), state {}, {} device errors",
            after.submissions, before.submissions, after.state, after.device_errors))?;
    expect(f.target, &scene_canvas(32, 32, GREY, &rects_drawn), "the target after the fuzzed streams")?;
    let next = [([0, 0, 16, 16], RED), ([10, 10, 32, 30], GREEN)];
    frame(ctx, &scene(f.target, TEAL, &next))?;
    expect(f.target, &scene_canvas(32, 32, TEAL, &next), "the next valid frame")?;
    pass()
}

/// One unit of `bounded-validation`'s mix, about 1 KiB: a draw, a buffer draw at a falling
/// offset, a copy, a sampled draw, a shape, a coverage table of its own, and scissor fill.
fn validation_unit(s: &mut Vec<u32>, f: &FuzzSet, buffer_slots: u32, i: u32, table: Option<(u64, u32, u32)>) {
    let start = s.len();
    encode::begin(s, f.target);
    encode::draw(s, pipeline::SOLID, RED, &[[0, 0, 2, 2], [1, 1, 3, 3]]);
    // 32-byte steps, falling: each buffer draw is its own snapshot range, named out of order.
    encode::draw_buffer(s, pipeline::SOLID, BLUE, f.buffer, 32 * (buffer_slots - 1 - i % buffer_slots), 1);
    encode::copy(s, f.texture, [0, 0, 2, 2], f.target, [4, 4]);
    encode::draw_sampled(s, pipeline::COPY, f.texture, &[[6, 6, 8, 8, 0, 0]]);
    let mut params = [0u32; 16];
    params[..3].copy_from_slice(&[4, 4, 2]);
    abi::draw::encode(s, pipeline::CIRCLE_FILL, GREEN, 0, 0, 0, &[abi::draw::Instance { rect: [0, 0, 4, 4], params }]);
    if let Some((buffer, data_bytes, offset)) = table {
        let mut params = [0u32; 16];
        params[..5].copy_from_slice(&[0, 0, 1, 1, offset]);
        abi::draw::encode(s, pipeline::COVERAGE_SPANS, GREEN, 0, buffer, data_bytes, &[abi::draw::Instance { rect: [0, 0, 1, 1], params }]);
    }
    while (s.len() - start) < 500 {
        s.extend_from_slice(&[stream::cmd::SCISSOR, 6, 0, 0, 8, 8]);
    }
}

/// A stream of about `bytes`, its units (2 KiB each) then one unknown command: refused at
/// its last command.
fn validation_stream(f: &FuzzSet, bytes: usize, slots: u32, tables: u64) -> Vec<u32> {
    let mut s = Vec::with_capacity(bytes / 4 + 1024);
    // Each unit's table is nine offsets valued 0 (one row's eight sub-scanlines and its end),
    // 36 bytes; the snapshot covers the stream's tables, so it grows with the stream.
    let data = (36 * (bytes / 2000 + 1)) as u32;
    let mut i = 0;
    while s.len() * 4 + 2100 < bytes {
        validation_unit(&mut s, f, slots, i, Some((tables, data, 36 * i)));
        i += 1;
    }
    s.extend_from_slice(&[0x55AA, 2]);
    s
}

/// The least of three refusals' times, in microseconds, requiring code 2 at `index`.
fn refusal_us(ctx: u64, s: &[u32], index: u32) -> R<u64> {
    let mut best = u64::MAX;
    for _ in 0..3 {
        let start = now_ns();
        let (r, a) = gpu::submit(ctx, s);
        let us = now_ns().saturating_sub(start) / 1_000;
        ensure(r == -EINVAL && a.error == stream::error::UNKNOWN_COMMAND && a.error_index == index,
            || format!("a {}-byte stream returned {} at command {} code {}, expected EINVAL code 2 at its last command {index}", s.len() * 4, errno(r), a.error_index, a.error))?;
        best = best.min(us);
    }
    Ok(best)
}

fn commands(s: &[u32]) -> u32 {
    let (mut n, mut at) = (0u32, 0usize);
    while at < s.len() {
        at += s[at + 1].max(2) as usize;
        n += 1;
    }
    n
}

pub fn bounded_validation() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let f = FuzzSet {
        target: target(format::B8G8R8A8, 8, 8)?,
        rgba: 0,
        texture: gpu::texture_create(format::B8G8R8A8, 8, 8).map_err(|e| fail_msg(format!("texture: {}", errno(e))))?,
        buffer: gpu::buffer_create(16 << 20).map_err(|e| fail_msg(format!("buffer: {}", errno(e))))?,
    };
    let (r, _) = gpu::upload(ctx, f.texture, [0, 0, 8, 8], &[0x33u8; 256], 32, abi::upload_source::B8G8R8A8, abi::purpose::FIXTURE);
    ensure(r == 0, || format!("UPLOAD returned {}", errno(r)))?;
    // A fresh buffer's bytes are 0: they read as empty rectangles for the buffer draws and
    // as crossing tables without crossings.
    let slots = (16u32 << 20) / 32;
    let tables = gpu::buffer_create(4 << 20).map_err(|e| fail_msg(format!("table buffer: {}", errno(e))))?;
    frame(ctx, &scene(f.target, GREY, &[]))?;
    let small_s = validation_stream(&f, 1 << 20, slots, tables);
    let large_s = validation_stream(&f, 16 << 20, slots, tables);
    let small_us = refusal_us(ctx, &small_s, commands(&small_s) - 1)?;
    let large_us = refusal_us(ctx, &large_s, commands(&large_s) - 1)?;
    let small_per = small_us as f64 / small_s.len() as f64;
    let large_per = large_us as f64 / large_s.len() as f64;
    value("small-us", small_us as i64, "us", None);
    value("large-us", large_us as i64, "us", None);
    value("per-word-ratio-x100", (100.0 * large_per / small_per) as i64, "", Some((0, 200)));
    ensure(large_per <= 2.0 * small_per,
        || format!("16 MiB took {large_us} us ({large_per:.4} us a word), 1 MiB {small_us} us ({small_per:.4} us a word): more than twice the time per word"))?;
    // The 80 MiB maximum, refused at its last command within 1 s.
    let mut max = filler(f.target, 8, 8, (80 << 20) - 8);
    max.extend_from_slice(&[0x55AA, 2]);
    let start = now_ns();
    let (r, a) = gpu::submit(ctx, &max);
    let ms = ms_since(start);
    value("max-ms", ms as i64, "ms", Some((0, 1_000)));
    ensure(r == -EINVAL && a.error == stream::error::UNKNOWN_COMMAND && a.error_index == commands(&max) - 1,
        || format!("the 80 MiB stream returned {} at command {} code {}", errno(r), a.error_index, a.error))?;
    ensure(ms <= 1_000, || format!("the 80 MiB stream was refused after {ms} ms"))?;
    drop(max);
    // Tables that overlap in one buffer cannot make validation read it once per instance:
    // their total past data_bytes is refused with code 8 at the first table past it.
    let mut s = Vec::new();
    encode::begin(&mut s, f.target);
    let data = 1u32 << 20;
    let instances: Vec<abi::draw::Instance> = (0..1000u32).map(|i| {
        let mut params = [0u32; 16];
        params[..5].copy_from_slice(&[0, 0, 1, 16_000, 4 * i]);
        abi::draw::Instance { rect: [0, 0, 1, 1], params }
    }).collect();
    abi::draw::encode(&mut s, pipeline::COVERAGE_SPANS, GREEN, 0, tables, data, &instances);
    let start = now_ns();
    let (r, a) = gpu::submit(ctx, &s);
    let us = now_ns().saturating_sub(start) / 1_000;
    value("overlap-us", us as i64, "us", None);
    ensure(r == -EINVAL && a.error == stream::error::BAD_RANGE && a.error_index == 1,
        || format!("1,000 overlapping coverage tables returned {} at command {} code {}", errno(r), a.error_index, a.error))?;
    pass()
}

pub fn efault_submit() -> CaseResult {
    let _case = begin()?;
    let (ctx, t, want) = drawn(format::B8G8R8A8, 8, 6, GREY)?;
    let before = counters(ctx)?;
    let page = memory::mmap(core::ptr::null_mut(), 8192, memory::PROT_READ | memory::PROT_WRITE,
        memory::MAP_PRIVATE | memory::MAP_ANONYMOUS, -1, 0).map_err(|e| fail_msg(format!("mmap: {e:?}")))?;
    // A stream whose first page is mapped and whose second is not.
    let s = scene(t, RED, &[]);
    let words = 1024 + s.len();
    let base = page as *mut u32;
    for i in 0..1024 {
        // SAFETY: the first page is mapped and writable.
        unsafe { base.add(i).write(if i % 6 == 0 { stream::cmd::SCISSOR } else if i % 6 == 1 { 6 } else { 0 }) };
    }
    memory::munmap(unsafe { page.add(4096) }, 4096).map_err(|e| fail_msg(format!("munmap: {e:?}")))?;
    let unmapped = memory::mmap(core::ptr::null_mut(), 4096, memory::PROT_READ | memory::PROT_WRITE,
        memory::MAP_PRIVATE | memory::MAP_ANONYMOUS, -1, 0).map_err(|e| fail_msg(format!("mmap: {e:?}")))?;
    memory::munmap(unmapped, 4096).map_err(|e| fail_msg(format!("munmap: {e:?}")))?;
    for (what, addr, len) in [("a stream at an unmapped address", unmapped as u64, (s.len() * 4) as u64),
                              ("a stream running into an unmapped page", page as u64, (words * 4) as u64)] {
        let (r, a) = gpu::submit_raw(ctx, addr, len);
        ensure(r == -EFAULT && a.fence == 0, || format!("{what} returned {} (fence {:#x})", errno(r), a.fence))?;
    }
    // The argument itself unmapped.
    // SAFETY: SUBMIT's argument at an unmapped address; the kernel must refuse it.
    let r = unsafe { gpu::call(abi::op::SUBMIT, unmapped, core::mem::size_of::<abi::Submit>()) };
    ensure(r == -EFAULT, || format!("a SUBMIT argument at an unmapped address returned {}", errno(r)))?;
    let after = counters(ctx)?;
    ensure(after.submissions == before.submissions && after.draws == before.draws, || "a faulted SUBMIT was counted".to_string())?;
    expect(t, &want, "the target after the faulted submissions")?;
    let f = submit(ctx, &scene(t, BLUE, &[]))?;
    ensure(f & 0xFFFF_FFFF == 2, || format!("the next submission's fence is {:#x}: a faulted SUBMIT used a fence", f))?;
    wait_ok(ctx, f, 2_000)?;
    let _ = memory::munmap(page, 4096);
    pass()
}

pub fn efault_readonly() -> CaseResult {
    let _case = begin()?;
    let (ctx, t, want) = drawn(format::B8G8R8A8, 8, 6, GREY)?;
    let (_, first) = read(t, 8, 6)?;
    let readbacks = counters(0)?.readbacks;
    let page = memory::mmap(core::ptr::null_mut(), 8192, memory::PROT_READ | memory::PROT_WRITE,
        memory::MAP_PRIVATE | memory::MAP_ANONYMOUS, -1, 0).map_err(|e| fail_msg(format!("mmap: {e:?}")))?;
    // SAFETY: both pages are mapped and writable before the second is made read-only.
    unsafe { core::ptr::write_bytes(page, 0x6C, 8192) };
    memory::mprotect(unsafe { page.add(4096) }, 4096, memory::PROT_READ).map_err(|e| fail_msg(format!("mprotect: {e:?}")))?;
    let mut a = abi::Readback { target: t, x: 0, y: 0, width: 8, height: 6, pixels: page as u64, pixels_len: 8 * 6 * 4,
        stride: 32, record: page as u64 + 4096, ..abi::Readback::ZERO };
    let r = gpu::call_with(abi::op::READBACK, &mut a);
    ensure(r == -EFAULT && a.reason == 0, || format!("a readback record in read-only memory returned {} reason {}", errno(r), a.reason))?;
    // SAFETY: both pages are still mapped and readable.
    let bytes = unsafe { core::slice::from_raw_parts(page, 8192) };
    ensure(bytes.iter().all(|&b| b == 0x6C), || "the refused readback wrote the caller's memory".to_string())?;
    ensure(counters(0)?.readbacks == readbacks, || "the refused readback was counted".to_string())?;
    let (_, next) = read(t, 8, 6)?;
    ensure(u64_at(&next, record::SERIAL) == u64_at(&first, record::SERIAL) + 1, || "the refused readback used an observation serial".to_string())?;
    expect(t, &want, "the target after the refused readback")?;
    let _ = ctx;
    let _ = memory::munmap(page, 8192);
    pass()
}

/// Make each op fail in the ways it can, `rounds` times over: the number of calls.
fn failing_calls(ctx: u64, t: u64, rounds: u32) -> R<u64> {
    let mut calls = 0u64;
    let mut px = vec![0u8; 8 * 6 * 4];
    let mut rec = [0u8; record::SIZE];
    for i in 0..rounds {
        let mut check = |what: &str, r: i64| -> R<()> {
            calls += 1;
            ensure(r < 0, || format!("{what} succeeded in the quiet-errors round {i}"))
        };
        // SAFETY: each call passes a null or live argument of the length given.
        unsafe {
            check("an unknown op", gpu::call(14 + i % 3, core::ptr::null_mut(), 0))?;
            check("VERSION with a length", gpu::call(abi::op::VERSION, core::ptr::null_mut(), 8))?;
            check("CAPS of the wrong size", gpu::call(abi::op::CAPS, rec.as_mut_ptr(), 8))?;
            check("MODES", gpu::call(abi::op::MODES, core::ptr::null_mut(), 0))?;
            check("SET_MODE", gpu::call(abi::op::SET_MODE, core::ptr::null_mut(), 0))?;
            check("SUBMIT with an unmapped argument", gpu::call(abi::op::SUBMIT, 8 as *mut u8, core::mem::size_of::<abi::Submit>()))?;
        }
        let mut cc = abi::ContextCreate { flags: 1, ..abi::ContextCreate::ZERO };
        check("CONTEXT_CREATE with flags", gpu::call_with(abi::op::CONTEXT_CREATE, &mut cc))?;
        check("CONTEXT_DESTROY of a guess", gpu::context_destroy(0x1234_5678 + i as u64))?;
        check("RESOURCE_DESTROY of a guess", gpu::resource_destroy(0x1234_5678 + i as u64))?;
        check("RESOURCE_CREATE of no kind", gpu::image_create(9, 0, 1, 1).err().unwrap_or(0))?;
        check("RESOURCE_CREATE past the maximum", gpu::target_create(format::B8G8R8A8, 1 << 20, 1).err().unwrap_or(0))?;
        check("SUBMIT of an empty stream", gpu::submit(ctx, &[]).0)?;
        check("SUBMIT of an unknown command", gpu::submit(ctx, &[0x99, 2]).0)?;
        check("SUBMIT before BEGIN", gpu::submit(ctx, &[stream::cmd::CLEAR, 3, 0]).0)?;
        check("SUBMIT past the work cap", {
            let mut s = Vec::new();
            encode::begin(&mut s, t);
            let rects = vec![[0, 0, 8, 6]; 4_000];
            let mut params = [0u32; 16];
            params[..3].copy_from_slice(&[0, 6, 0]);
            let inst: Vec<abi::draw::Instance> = rects.iter().map(|r| abi::draw::Instance { rect: *r, params }).collect();
            abi::draw::encode(&mut s, pipeline::GRADIENT_V, 0, 0, 0, 0, &inst);
            gpu::submit(ctx, &s).0
        })?;
        check("SUBMIT on a guessed context", gpu::submit(0x55 + i as u64, &[stream::cmd::CLEAR, 3, 0]).0)?;
        check("WAIT on a fence never issued", gpu::wait(ctx, 1 << 40, 0).0)?;
        check("WAIT on a guessed context", gpu::wait(0x77, 1, 0).0)?;
        check("READBACK out of the target", gpu::readback(t, [4, 4, 8, 8], &mut px, 32, &mut rec))?;
        check("READBACK at a short stride", gpu::readback(t, [0, 0, 8, 6], &mut px, 4, &mut rec))?;
        check("READBACK of a guess", gpu::readback(0x99, [0, 0, 8, 6], &mut px, 32, &mut rec))?;
        check("UPLOAD of no purpose", gpu::upload(ctx, t, [0, 0, 1, 1], &[0; 4], 4, 0, 9).0)?;
        check("UPLOAD into a target", gpu::upload(ctx, t, [0, 0, 1, 1], &[0; 4], 4, 0, abi::purpose::IMAGE).0)?;
        check("PRESENT by a process that is not the display owner", gpu::present(ctx, t).0)?;
        check("COUNTERS of a guess", gpu::counters(0x4242).0)?;
    }
    Ok(calls)
}

pub fn quiet_errors() -> CaseResult {
    let _case = begin()?;
    let (ctx, t, _) = drawn(format::B8G8R8A8, 8, 6, GREY)?;
    // The positive control: point 3's 33 bytes reach this thread's count.
    let before = kernel_output()?;
    arm(EMIT_DIAGNOSTIC, 1)?;
    ensure(gpu::version() == 1, || "VERSION".to_string())?;
    let emitted = kernel_output()? - before;
    ensure(emitted == DIAGNOSTIC_BYTES, || format!("with emit-diagnostic armed a GPU call added {emitted} bytes to this thread's kernel output, expected {DIAGNOSTIC_BYTES}"))?;
    // Not the display owner: PRESENT and SET_MODE refuse.
    let _ = graphics::give_back_display();
    let start = kernel_output()?;
    let mut calls = failing_calls(ctx, t, 40)?;
    // Device-side errors: a forced device error, a stale fence and a skipped transfer.
    for point in [test_point::DEVICE_ERROR, test_point::STALE_FENCE] {
        arm(point, 0)?;
        let f = submit(ctx, &scene(t, RED, &[]))?;
        let (r, _) = gpu::wait(ctx, f, 2_000_000_000);
        ensure(r == -EIO, || format!("the forced fault {point} gave WAIT {}", errno(r)))?;
        calls += 1;
    }
    frame(ctx, &scene(t, RED, &[]))?;
    ensure(gpu::tc_arm(test_point::SKIP_TRANSFER, t, 0) == 0, || "arming skip-transfer".to_string())?;
    let mut px = vec![0u8; 8 * 6 * 4];
    let mut rec = [0u8; record::SIZE];
    ensure(gpu::readback(t, [0, 0, 8, 6], &mut px, 32, &mut rec) == -EIO, || "the skipped transfer was not refused".to_string())?;
    calls += 1;
    value("failing-calls", calls as i64, "", Some((1_000, i64::MAX)));
    ensure(calls >= 1_000, || format!("only {calls} failing calls were made"))?;
    let added = kernel_output()? - start;
    ensure(added == 0, || format!("{calls} failing GPU calls added {added} bytes to this thread's kernel output"))?;
    pass()
}

pub fn unknown_op() -> CaseResult {
    let _case = begin()?;
    let mut buf = [0u8; 1024];
    for op in [14u64, 15, 16, 31, 64, 255, 0xFFFF, u32::MAX as u64, 1 << 32, (1 << 32) | abi::op::VERSION as u64, u64::MAX] {
        for (arg, len) in [(core::ptr::null_mut(), 0usize), (buf.as_mut_ptr(), buf.len()), (buf.as_mut_ptr(), 8)] {
            // SAFETY: a null argument with length 0, or a live 1,024-byte buffer.
            let r = unsafe { libbreenix::syscall::raw::syscall3(abi::SYS_GPU, op, arg as u64, len as u64) as i64 };
            ensure(r == -EINVAL, || format!("op {op:#x} with length {len} returned {}, expected EINVAL", errno(r)))?;
        }
    }
    ensure(buf.iter().all(|&b| b == 0), || "an unknown op wrote the caller's buffer".to_string())?;
    ensure(gpu::version() == 1, || "VERSION after the unknown ops".to_string())?;
    pass()
}

fn tc_raw(op: u32, arg: *mut u8, len: usize) -> i64 {
    // SAFETY: a null argument or a live buffer of `len` bytes.
    unsafe { gpu::test_control(op, arg, len) }
}

pub fn test_hooks_absent() -> CaseResult {
    let _case = begin()?;
    let kid = spawn(|_| {
        // SAFETY: setuid takes a user id.
        let r = unsafe { libbreenix::syscall::raw::syscall1(SYS_SETUID, NOBODY) as i64 };
        need(r == 0, || format!("setuid({NOBODY}) returned {}", errno(r)))?;
        let eperm = -abi::errno::EPERM;
        need(tc_raw(abi::test_control::STATE, core::ptr::null_mut(), 0) == eperm, || "STATE was not refused to a non-root process".to_string())?;
        need(tc_raw(abi::test_control::DISARM_ALL, core::ptr::null_mut(), 0) == eperm, || "DISARM_ALL was not refused".to_string())?;
        for point in test_point::FIRST..=test_point::LAST {
            let r = gpu::tc_arm(point, 0, 0);
            need(r == eperm, || format!("ARM of point {point} by a non-root process returned {}", errno(r)))?;
            let r = gpu::tc_fired(point);
            need(r == Err(eperm), || format!("FIRED of point {point} by a non-root process returned {r:?}"))?;
            // Malformed arguments are refused for the caller before they are decoded.
            let mut bad = [0u8; 24];
            bad[..4].copy_from_slice(&point.to_le_bytes());
            bad[4] = 1;
            for (arg, len) in [(bad.as_mut_ptr(), 24usize), (bad.as_mut_ptr(), 3), (8 as *mut u8, 24)] {
                let r = tc_raw(abi::test_control::ARM, arg, len);
                need(r == eperm, || format!("a malformed ARM of point {point} (length {len}) returned {}, expected EPERM before decoding", errno(r)))?;
            }
        }
        let r = tc_raw(4, core::ptr::null_mut(), 0);
        need(r == eperm, || format!("op 4 by a non-root process returned {}", errno(r)))
    })?;
    kid.done()?;
    // The suite's root process may arm and read each point; 32 is not defined.
    for point in test_point::FIRST..=test_point::LAST {
        let before = fired(point);
        ensure(before != u64::MAX, || format!("FIRED of point {point} by root was refused"))?;
        ensure(gpu::tc_arm(point, 0, 0) == 0, || format!("ARM of point {point} by root was refused"))?;
    }
    ensure(gpu::tc_disarm_all() == 0, || "DISARM_ALL by root".to_string())?;
    ensure(gpu::tc_arm(test_point::LAST + 1, 0, 0) == -EINVAL, || "ARM of point 32 was accepted".to_string())?;
    pass()
}

/// A child set up to block in one op once told `G`; it reports `B` just before the op.
/// The WAIT child's submission is the one whose withheld completion starts the stall. The
/// children draw while their catalogues warm (ruling 33): the stall takes most of the case's
/// time.
fn blocker(which: u8) -> R<Kid> {
    spawn(move |line| {
        match which {
            b'c' => {
                line.send(b'R', &[])?;
                line.recv(b'G')?;
                line.send(b'B', &[])?;
                let r = gpu::context_create();
                Err(format!("CONTEXT_CREATE returned {r:?} during the stall"))
            }
            b'w' => {
                let c = gpu::context_create().map_err(|e| format!("CONTEXT_CREATE: {}", errno(e)))?;
                let t = child(target(format::B8G8R8A8, 8, 8))?;
                line.send(b'R', &[])?;
                line.recv(b'G')?;
                let (r, s) = gpu::submit(c, &scene(t, RED, &[]));
                need(r == 0, || format!("SUBMIT returned {}", errno(r)))?;
                line.send(b'B', &[])?;
                let (r, _) = gpu::wait(c, s.fence, 10_000_000_000);
                Err(format!("WAIT returned {} during the stall", errno(r)))
            }
            b'r' => {
                let c = gpu::context_create().map_err(|e| format!("CONTEXT_CREATE: {}", errno(e)))?;
                let t = child(target(format::B8G8R8A8, 64, 64))?;
                child(frame(c, &scene(t, RED, &[])).map(drop))?;
                line.send(b'R', &[])?;
                line.recv(b'G')?;
                let mut px = vec![0u8; 64 * 64 * 4];
                let mut rec = [0u8; record::SIZE];
                line.send(b'B', &[])?;
                let r = gpu::readback(t, [0, 0, 64, 64], &mut px, 256, &mut rec);
                Err(format!("READBACK returned {} during the stall", errno(r)))
            }
            _ => {
                let c = gpu::context_create().map_err(|e| format!("CONTEXT_CREATE: {}", errno(e)))?;
                let t = child(scanout_target(format::B8G8R8A8, 64, 64))?;
                child(frame(c, &scene(t, RED, &[])).map(drop))?;
                graphics::take_over_display().map_err(|e| format!("take display: {e:?}"))?;
                line.send(b'R', &[])?;
                line.recv(b'G')?;
                line.send(b'B', &[])?;
                let (r, _) = gpu::present(c, t);
                Err(format!("PRESENT returned {} during the stall", errno(r)))
            }
        }
    })
}

pub fn killable_wait() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let mut kids = Vec::new();
    for (which, name) in [(b'w', "WAIT"), (b'c', "CONTEXT_CREATE"), (b'r', "READBACK"), (b'p', "PRESENT")] {
        let kid = blocker(which)?;
        kid.recv(b'R')?;
        kids.push((kid, name, which));
    }
    // Withhold completions from the next one on (the WAIT child's submission): the device's
    // answers stay untaken until the watchdog resets it, so each child's op blocks.
    let fired_before = fired(test_point::WITHHOLD_FENCE);
    arm(test_point::WITHHOLD_FENCE, test_point::WITHHOLD_ALL)?;
    let stall = now_ns();
    kids[0].0.send(b'G', &[])?;
    kids[0].0.recv(b'B')?;
    while fired(test_point::WITHHOLD_FENCE) == fired_before {
        ensure(ms_since(stall) < 500, || "withhold-fence did not fire within 500 ms".to_string())?;
        let _ = time::sleep_ms(2);
    }
    for (kid, _, _) in &kids[1..] {
        kid.send(b'G', &[])?;
        kid.recv(b'B')?;
    }
    for (kid, name, which) in &mut kids {
        // After B there is only the named GPU call. Observe an actual blocked
        // scheduler state, rather than assuming the child ran during a delay.
        loop {
            let stat = std::fs::read_to_string(format!("/proc/{}/stat", kid.pid))
                .map_err(|e| fail_msg(format!("reading the {name} child's state: {e}")))?;
            let state = stat.rsplit_once(") ").and_then(|(_, tail)| tail.split_whitespace().next());
            if state == Some("S") { break; }
            ensure(ms_since(stall) < W_MS, || format!("the {name} child did not block before the watchdog"))?;
            let _ = time::sleep_ms(1);
        }
        let start = now_ns();
        kid.kill()?;
        let status = kid.reap()?;
        let ms = ms_since(start);
        let label = match which { b'w' => "wait-exit-ms", b'c' => "create-exit-ms", b'r' => "readback-exit-ms", _ => "present-exit-ms" };
        value(label, ms as i64, "ms", Some((0, K_MS as i64)));
        ensure(process::wifsignaled(status) && process::wtermsig(status) == SIGKILL,
            || format!("the child blocked in {name} ended with status {status:#x}, not by SIGKILL"))?;
        ensure(ms <= K_MS, || format!("the child blocked in {name} took {ms} ms to exit after SIGKILL"))?;
    }
    ensure(ms_since(stall) < W_MS, || format!("the kills ended {} ms into the stall, after the watchdog could have ended the waits", ms_since(stall)))?;
    // The stall ends with the device's reset (section 1.4.1, within 2W + 3K + R of the
    // stalled submission); then new work renders exactly.
    loop {
        let k = counters(ctx)?;
        if k.state == context_state::RESET {
            break;
        }
        ensure(ms_since(stall) < 2 * W_MS + 3 * K_MS + 100 + 200, || format!("{} ms into the stall this process's context is in state {}, not reset", ms_since(stall), k.state))?;
        let _ = time::sleep_ms(20);
    }
    value("reset-ms", ms_since(stall) as i64, "ms", None);
    let start = now_ns();
    let made = gpu::context_create();
    let ms = ms_since(start);
    value("recreate-ms", ms as i64, "ms", None);
    let c2 = made.map_err(|e| fail_msg(format!("CONTEXT_CREATE after the reset returned {} after {ms} ms", errno(e))))?;
    // A context draws while its catalogue warms (ruling 33); this case's time is the stall's.
    let t2 = target(format::B8G8R8A8, 9, 7)?;
    frame(c2, &scene(t2, GREEN, &[([1, 1, 3, 3], RED)]))?;
    expect(t2, &scene_canvas(9, 7, GREEN, &[([1, 1, 3, 3], RED)]), "a new context after the reset")?;
    pass()
}

pub fn client_cannot_scanout() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = scanout_target(format::B8G8R8A8, 16, 12)?;
    let rects = [([3, 3, 9, 9], RED)];
    frame(ctx, &scene(t, BLUE, &rects))?;
    graphics::take_over_display().map_err(|e| fail_msg(format!("take display: {e:?}")))?;
    let (r, p) = gpu::present(ctx, t);
    ensure(r == 0, || format!("the owner's PRESENT returned {}", errno(r)))?;
    let kid = spawn(|_| {
        let c = child(context())?;
        let own = child(scanout_target(format::B8G8R8A8, 16, 12))?;
        child(frame(c, &scene(own, GREEN, &[])).map(drop))?;
        let eperm = -abi::errno::EPERM;
        need(gpu::set_mode(0) == eperm, || "a client's valid SET_MODE was not refused with EPERM".to_string())?;
        let (r, a) = gpu::present(c, own);
        need(r == eperm && a.fence == 0, || format!("a client's PRESENT returned {} with fence {:#x}", errno(r), a.fence))?;
        let mut junk = [0xEEu8; 64];
        // SAFETY: a null argument, or a live 64-byte buffer.
        for (arg, len) in [(core::ptr::null_mut(), 0usize), (junk.as_mut_ptr(), 64), (8 as *mut u8, 16)] {
            let r = unsafe { gpu::call(abi::op::SET_MODE, arg, len) };
            need(r == eperm, || format!("a client's SET_MODE (length {len}) returned {}, expected EPERM", errno(r)))?;
        }
        need(junk.iter().all(|&b| b == 0xEE), || "the refused SET_MODE wrote its argument".to_string())?;
        let mut px = vec![0u8; 16 * 12 * 4];
        let mut rec = [0u8; record::SIZE];
        let r = gpu::readback(own, [0, 0, 16, 12], &mut px, 64, &mut rec);
        need(r == 0 && u32_at(&rec, record::FLAGS) & 1 == 0, || format!("the client's own target read {} with flags {}", errno(r), u32_at(&rec, record::FLAGS)))
    })?;
    kid.done()?;
    let rec = expect(t, &scene_canvas(16, 12, BLUE, &rects), "the owner's scanout after the client")?;
    ensure(u32_at(&rec, record::FLAGS) & 1 == 1 && u64_at(&rec, record::PRESENT_FENCE) == p.fence,
        || "the client changed the scanout binding".to_string())?;
    // SAFETY: SET_MODE with no argument.
    let r = unsafe { gpu::call(abi::op::SET_MODE, core::ptr::null_mut(), 0) };
    ensure(r == -EINVAL, || format!("the owner's SET_MODE(NULL, 0) returned {}, expected EINVAL", errno(r)))?;
    pass()
}

pub fn irq_masked_bound() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let small_t = target(format::B8G8R8A8, 8, 8)?;
    arm(IRQ_HOLD, 0)?;
    frame(ctx, &scene(small_t, RED, &[]))?;
    let held = trace_counter("gpu_irq_masked_max_us")?;
    value("irq-hold-gauge", held as i64, "us", Some((2_000, i64::MAX)));
    ensure(held >= 2_000, || format!("with irq-hold armed the gauge reads {held} us; the 2 ms hold reads at least 2,000"))?;
    ensure(gpu::tc_disarm_all() == 0, || "DISARM_ALL".to_string())?;
    let reset = trace_counter("gpu_irq_masked_max_us")?;
    ensure(reset < 2_000, || format!("after DISARM_ALL the gauge reads {reset} us; it restarts at 0"))?;
    // A full-screen workload over the GPU paths: uploads, draws, copies, blends, readback
    // and present at 2560x1600.
    let (w, h) = (2560u32, 1600u32);
    let t = scanout_target(format::B8G8R8A8, w, h)?;
    let tex = gpu::texture_create(format::B8G8R8A8, w, h).map_err(|e| fail_msg(format!("texture: {}", errno(e))))?;
    let pixels = vec![0x7Fu8; (w * h * 4) as usize];
    let (r, _) = gpu::upload(ctx, tex, [0, 0, w, h], &pixels, w * 4, abi::upload_source::B8G8R8A8, abi::purpose::IMAGE);
    ensure(r == 0, || format!("the full-screen UPLOAD returned {}", errno(r)))?;
    drop(pixels);
    graphics::take_over_display().map_err(|e| fail_msg(format!("take display: {e:?}")))?;
    for i in 0..10u32 {
        let mut s = Vec::new();
        encode::begin(&mut s, t);
        encode::clear(&mut s, GREY.wrapping_add(i));
        let rects: Vec<[i32; 4]> = (0..2_000).map(|k| { let x = (k * 37 % 2500) as i32; let y = (k * 53 % 1550) as i32; [x, y, x + 60, y + 50] }).collect();
        encode::draw(&mut s, pipeline::SOLID, RED, &rects);
        encode::copy(&mut s, tex, [0, 0, 1280, 800], t, [640, 400]);
        let mut params = [0u32; 16];
        params[..4].copy_from_slice(&[0, 0, 0, 0]);
        abi::draw::encode(&mut s, pipeline::OVER, 0, tex, 0, 0, &[abi::draw::Instance { rect: [0, 0, w as i32, h as i32], params }]);
        frame(ctx, &s)?;
        let (r, _) = gpu::present(ctx, t);
        ensure(r == 0, || format!("PRESENT returned {}", errno(r)))?;
    }
    let mut px = vec![0u8; (w * h * 4) as usize];
    let mut rec = [0u8; record::SIZE];
    let r = gpu::readback(t, [0, 0, w, h], &mut px, w * 4, &mut rec);
    ensure(r == 0, || format!("the full-screen READBACK returned {}", errno(r)))?;
    let bound = trace_counter("gpu_irq_masked_max_us")?;
    value("irq-masked-max", bound as i64, "us", Some((0, 500)));
    ensure(bound <= 500, || format!("over the full-screen workload the longest IRQ-masked interval in the GPU paths was {bound} us; the bound is 500"))
}
