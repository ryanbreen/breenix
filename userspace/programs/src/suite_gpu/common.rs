//! What the `gpu` suite's cases share: setup, the stream builders, readback, and the
//! suite's own model of what a frame must read back as (`docs/gui/gpu.md` section 1.10:
//! expectations are computed here, never taken from the kernel).

use libbreenix::gpu::{self, abi};
use libbreenix::suite::{CaseError, CaseResult};

pub use abi::errno::{EAGAIN, EDQUOT, EFAULT, EINTR, EINVAL, EIO, ENODEV, ENOSYS, ETIMEDOUT};
pub use abi::stream::encode;
pub use abi::{capability, context_state, fence_status, format, limits, pipeline, record, stream, test_point};

/// The errno a raw return carries, by name.
pub fn errno(ret: i64) -> String {
    let name = match -ret {
        0 => return format!("{ret}"),
        EINTR => "EINTR",
        EIO => "EIO",
        EAGAIN => "EAGAIN",
        abi::errno::ENOMEM => "ENOMEM",
        EFAULT => "EFAULT",
        ENODEV => "ENODEV",
        EINVAL => "EINVAL",
        ENOSYS => "ENOSYS",
        ETIMEDOUT => "ETIMEDOUT",
        EDQUOT => "EDQUOT",
        _ => return format!("errno {}", -ret),
    };
    name.to_string()
}

/// A colour as the stream and the canonical readback carry it: bytes B, G, R, A.
pub const fn bgra(b: u8, g: u8, r: u8, a: u8) -> u32 {
    b as u32 | (g as u32) << 8 | (r as u32) << 16 | (a as u32) << 24
}

/// CLOCK_MONOTONIC in nanoseconds.
pub fn now_ns() -> u64 {
    libbreenix::time::now_monotonic().map_or(0, |t| t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64)
}

pub fn ms_since(start: u64) -> u64 {
    now_ns().saturating_sub(start) / 1_000_000
}

/// Disarms every test-control point when the case ends.
pub struct Case;

impl Drop for Case {
    fn drop(&mut self) {
        let _ = gpu::tc_disarm_all();
    }
}

/// Start a case that needs the GPU core: no point left armed by an earlier case, syscall
/// 416 answering, and the device admitted to the profile.
pub fn begin() -> Result<Case, CaseError> {
    let _ = gpu::tc_disarm_all();
    let v = gpu::version();
    if v != abi::INTERFACE_VERSION {
        return Err(fail_msg(format!("syscall 416 VERSION returned {}, expected 1", errno(v))));
    }
    let (r, c) = gpu::caps();
    if r != 0 {
        return Err(fail_msg(format!("CAPS returned {}", errno(r))));
    }
    if c.status != abi::status::ADMITTED {
        return Err(fail_msg(format!("the device is not admitted: status {} ({})", c.status, text(&c.reason))));
    }
    Ok(Case)
}

pub fn fail_msg(msg: String) -> CaseError {
    CaseError::Fail(msg)
}

/// `Err` with `msg` unless `cond`.
pub fn ensure(cond: bool, msg: impl FnOnce() -> String) -> Result<(), CaseError> {
    if cond {
        Ok(())
    } else {
        Err(fail_msg(msg()))
    }
}

/// A NUL-padded string field as text.
pub fn text(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

pub fn caps() -> Result<abi::Caps, CaseError> {
    let (r, c) = gpu::caps();
    ensure(r == 0, || format!("CAPS returned {}", errno(r)))?;
    Ok(c)
}

pub fn context() -> Result<u64, CaseError> {
    gpu::context_create().map_err(|e| fail_msg(format!("CONTEXT_CREATE returned {}", errno(e))))
}

pub fn target(fmt: u32, w: u32, h: u32) -> Result<u64, CaseError> {
    gpu::target_create(fmt, w, h).map_err(|e| fail_msg(format!("RESOURCE_CREATE of a {w}x{h} target returned {}", errno(e))))
}

/// Submit and return the fence.
pub fn submit(ctx: u64, s: &[u32]) -> Result<u64, CaseError> {
    let (r, a) = gpu::submit(ctx, s);
    ensure(r == 0, || format!("SUBMIT returned {} (command {} error {})", errno(r), a.error_index, a.error))?;
    Ok(a.fence)
}

/// Wait for a fence that must complete OK within `ms`.
pub fn wait_ok(ctx: u64, fence: u64, ms: u64) -> Result<(), CaseError> {
    let (r, a) = gpu::wait(ctx, fence, ms * 1_000_000);
    ensure(r == 0, || format!("WAIT returned {} (status {} answer {:#x})", errno(r), a.status, a.answer))
}

/// Submit a frame and wait for it.
pub fn frame(ctx: u64, s: &[u32]) -> Result<u64, CaseError> {
    let f = submit(ctx, s)?;
    wait_ok(ctx, f, 2_000)?;
    Ok(f)
}

/// A whole-target readback: the canonical BGRA bytes and record v2.
pub fn read(t: u64, w: u32, h: u32) -> Result<(Vec<u8>, [u8; record::SIZE]), CaseError> {
    let mut px = vec![0u8; (w * h * 4) as usize];
    let mut rec = [0u8; record::SIZE];
    let r = gpu::readback(t, [0, 0, w, h], &mut px, w * 4, &mut rec);
    ensure(r == 0, || format!("READBACK returned {} (unwritten {})", errno(r), u32_at(&rec, record::UNWRITTEN)))?;
    Ok((px, rec))
}

pub fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

pub fn u64_at(b: &[u8], at: usize) -> u64 {
    u32_at(b, at) as u64 | (u32_at(b, at + 4) as u64) << 32
}

/// The suite's own picture of a target: what each pixel must read back as.
pub struct Canvas {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

impl Canvas {
    pub fn new(w: u32, h: u32) -> Canvas {
        Canvas { w, h, px: vec![0; (w * h * 4) as usize] }
    }

    pub fn clear(&mut self, colour: u32) {
        let c = colour.to_le_bytes();
        for p in self.px.chunks_exact_mut(4) {
            p.copy_from_slice(&c);
        }
    }

    /// Fill the half-open rectangle `[x0, x1) x [y0, y1)`, clipped to the canvas.
    pub fn rect(&mut self, r: [i32; 4], colour: u32) {
        let c = colour.to_le_bytes();
        let x0 = r[0].clamp(0, self.w as i32) as u32;
        let y0 = r[1].clamp(0, self.h as i32) as u32;
        let x1 = r[2].clamp(0, self.w as i32) as u32;
        let y1 = r[3].clamp(0, self.h as i32) as u32;
        for y in y0..y1 {
            for x in x0..x1 {
                let at = ((y * self.w + x) * 4) as usize;
                self.px[at..at + 4].copy_from_slice(&c);
            }
        }
    }

    /// `Ok` when `got` equals the canvas byte for byte, or the first pixel that differs.
    pub fn matches(&self, got: &[u8], what: &str) -> Result<(), CaseError> {
        if got.len() != self.px.len() {
            return Err(fail_msg(format!("{what}: read {} bytes, expected {}", got.len(), self.px.len())));
        }
        match got.chunks_exact(4).zip(self.px.chunks_exact(4)).position(|(a, b)| a != b) {
            None => Ok(()),
            Some(i) => {
                let (x, y) = (i as u32 % self.w, i as u32 / self.w);
                let g = &got[i * 4..i * 4 + 4];
                let e = &self.px[i * 4..i * 4 + 4];
                Err(fail_msg(format!("{what}: pixel ({x},{y}) read {g:02x?}, expected {e:02x?}")))
            }
        }
    }
}

/// Read target `t` back and compare it with `want`.
pub fn expect(t: u64, want: &Canvas, what: &str) -> Result<[u8; record::SIZE], CaseError> {
    let (px, rec) = read(t, want.w, want.h)?;
    want.matches(&px, what)?;
    Ok(rec)
}

/// A frame for target `t`: clear to `bg`, then each rectangle in its colour.
pub fn scene(t: u64, bg: u32, rects: &[([i32; 4], u32)]) -> Vec<u32> {
    let mut s = Vec::new();
    encode::begin(&mut s, t);
    encode::clear(&mut s, bg);
    for (r, c) in rects {
        encode::draw(&mut s, pipeline::SOLID, *c, &[*r]);
    }
    s
}

/// The canvas `scene` must produce.
pub fn scene_canvas(w: u32, h: u32, bg: u32, rects: &[([i32; 4], u32)]) -> Canvas {
    let mut c = Canvas::new(w, h);
    c.clear(bg);
    for (r, col) in rects {
        c.rect(*r, *col);
    }
    c
}

/// Arm a test-control point for any target.
pub fn arm(point: u32, param: u64) -> Result<(), CaseError> {
    let r = gpu::tc_arm(point, 0, param);
    ensure(r == 0, || format!("arming test-control point {point} returned {}", errno(r)))
}

pub fn fired(point: u32) -> u64 {
    gpu::tc_fired(point).unwrap_or(u64::MAX)
}

pub fn counters(ctx: u64) -> Result<abi::Counters, CaseError> {
    let (r, c) = gpu::counters(ctx);
    ensure(r == 0, || format!("COUNTERS returned {}", errno(r)))?;
    Ok(c)
}

/// Pass when everything before it did.
pub fn pass() -> CaseResult {
    Ok(())
}
