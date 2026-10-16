//! Scratch spike for the `gpu` effort's G0 on Parallels (branch scratch/gpu-spike-parallels,
//! never merged).
//!
//! Runs once at boot, after VirGL init and the timer, in its own VirGL context (2) beside the
//! production compositor's context (1), with only its own resources attached to it. It drives the
//! deterministic rendering profile through SUBMIT_3D, reads every target back with a fenced
//! TRANSFER_FROM_HOST_3D into poisoned guest backing, and compares every pixel with a formula
//! computed here from integers. Every line it prints starts with `GPU_SPIKE`.
//!
//! Sources: the VirtIO 1.3 specification (virtio-gpu commands and structures), the VirGL protocol
//! as interface facts (command numbers and field layouts, already encoded in `virgl.rs`), and the
//! TGSI documentation for the shader text. Expectations are Breenix's own formulas.

use super::{
    cmd, dimensions, dma_cache_clean, dma_cache_invalidate, get_capset, get_capset_info,
    send_owned_command_expect_ok, set_scanout_resource, virgl_attach_backing_from_pages,
    virgl_ctx_attach_resource_cmd, virgl_ctx_create_cmd, virgl_resource_create_3d_cmd,
    virgl_submit_3d_cmd, with_device_state, VirtioGpuCtrlHdr, VirtioGpuResourceFlush,
    VirtioGpuTransferHost3d, PCI_RESP_BUF, RESOURCE_3D_ID, VIRTIO_GPU_FLAG_FENCE,
};
use crate::drivers::virtio::virgl::{
    format as vfmt, pipe, swizzle, CommandBuffer, OBJ_BLEND, OBJ_DSA, OBJ_RASTERIZER,
    OBJ_VERTEX_ELEMENTS,
};
use crate::serial_println;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const CTX: u32 = 2;
const CTX_FOREIGN: u32 = 3;
const CB_CAPACITY: usize = 3072;

// Resources (device-global ids, attached to CTX only).
const RES_A: u32 = 300;
const RES_T8: u32 = 301;
const RES_TB: u32 = 302;
const RES_COPY_A: u32 = 303;
const RES_VB: u32 = 304;
const RES_B: u32 = 305;
const RES_TR: u32 = 306;
const RES_COPY_B: u32 = 307;
const RES_P: u32 = 308;
const RES_G: u32 = 309;
const VB_BYTES: u32 = 16384;
/// Row pitch of the main R8 texture's backing: Parallels drops 3D transfers at a 64-byte pitch.
const T8_PITCH: u32 = 256;

// Object handles in CTX's sub-context 1 (one handle space for all object types).
const S_A: u32 = 1;
const S_B: u32 = 2;
const S_P: u32 = 3;
const S_G: u32 = 4;
const H_BLEND: u32 = 10;
const H_DSA: u32 = 11;
const H_RAST: u32 = 12;
const H_RAST_SCISSOR: u32 = 13;
const H_VE: u32 = 14;
const H_VS: u32 = 20;
const H_FS_TEXFETCH: u32 = 21;
const H_FS_INTEGER: u32 = 22;
const H_FS_BACKGROUND: u32 = 23;
const H_FS_BLEND: u32 = 24;
const H_FS_UNORM: u32 = 25;
const H_FS_SOLID: u32 = 26;
const H_FS_TRIPRED: u32 = 27;
const H_FS_PROBE_TXF: u32 = 28;
const H_FS_PROBE_TEX: u32 = 29;
const V_T8: u32 = 30;
const V_TB: u32 = 31;
const V_COPY_A: u32 = 32;
const V_TR: u32 = 33;
const V_COPY_B: u32 = 34;
const H_SAMPLER: u32 = 40;

const AW: u32 = 256;
const AH: u32 = 256;
const BW: u32 = 256;
const BH: u32 = 64;
const TW: u32 = 64;
const TH: u32 = 16;
const GW: u32 = 2560;
const GH: u32 = 1600;

const CLR_A: [u32; 4] = [37, 99, 201, 123];
const CLR_B: [u32; 4] = [201, 37, 123, 99];
const CLR_P: [u32; 4] = [16, 32, 48, 255];
const CLR_G: [u32; 4] = [3, 5, 7, 9];
const POISON: [u8; 4] = [0xA5, 0x5A, 0xC3, 0x3C];

/// Solid rectangles (x, y, w, h, rgba) drawn in this order in scene A's band 96..128.
const RECTS: [(u32, u32, u32, u32, [u32; 4]); 8] = [
    (1, 97, 1, 1, [255, 0, 0, 255]),
    (3, 99, 5, 2, [0, 255, 0, 255]),
    (10, 100, 37, 23, [12, 34, 56, 78]),
    (30, 110, 40, 10, [200, 100, 50, 255]),
    (80, 96, 1, 32, [1, 2, 3, 4]),
    (90, 127, 160, 1, [250, 251, 252, 253]),
    (255, 96, 1, 32, [9, 8, 7, 6]),
    (100, 104, 64, 16, [0, 0, 0, 0]),
];
/// The scissored draw: a full-width quad over 128..144, scissor [16, 200) x [130, 140).
const SCISSOR: (u32, u32, u32, u32) = (16, 130, 200, 140);
const C_SCISSOR: [u32; 4] = [77, 88, 99, 111];
/// Device-rasterized triangles (pixel-space vertices) and their colours.
const T1: [(u32, u32); 3] = [(100, 66), (180, 70), (120, 95)];
const T2: [(u32, u32); 3] = [(190, 65), (250, 65), (220, 95)];
const C_T1: [u32; 4] = [5, 150, 250, 255];
const C_T2: [u32; 4] = [250, 150, 5, 255];
/// The shader-predicate triangle in doubled coordinates (pixel (x, y) centre is (2x+1, 2y+1)).
const TP: [(i64, i64); 3] = [(16, 132), (180, 140), (60, 190)];
/// The planted device-side corruption: one pixel in scene A's cleared area.
const CORRUPT_AT: (u32, u32) = (200, 200);
const C_CORRUPT: [u32; 4] = [1, 2, 3, 4];

// ----------------------------------------------------------------------------- time

fn counter() -> (u64, u64) {
    let (cnt, frq): (u64, u64);
    unsafe {
        core::arch::asm!("isb; mrs {}, cntvct_el0", out(reg) cnt, options(nostack));
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) frq, options(nostack));
    }
    (cnt, frq)
}

fn now_us() -> u64 {
    let (c, f) = counter();
    ((c as u128) * 1_000_000 / (f as u128)) as u64
}

static NEVER: crate::task::completion::Completion = crate::task::completion::Completion::new();

/// Sleeps `ms` milliseconds on a completion nobody completes (spike thread only).
fn sleep_ms(ms: u64) {
    let _ = NEVER.wait_timeout_uninterruptible(1, ms * 1_000_000);
}

/// Waits `ms` milliseconds in the boot thread, in WFI when the CPU 0 timer is ticking.
fn idle_ms(ms: u64) -> &'static str {
    use crate::arch_impl::aarch64::timer_interrupt::TIMER_TICK_COUNT;
    let start = now_us();
    let ticks = TIMER_TICK_COUNT[0].load(Ordering::Relaxed);
    while now_us() - start < 50_000 && TIMER_TICK_COUNT[0].load(Ordering::Relaxed) == ticks {
        core::hint::spin_loop();
    }
    let ticking = TIMER_TICK_COUNT[0].load(Ordering::Relaxed) != ticks;
    while now_us() - start < ms * 1000 {
        if ticking {
            unsafe { core::arch::asm!("wfi", options(nostack, preserves_flags)) };
        } else {
            core::hint::spin_loop();
        }
    }
    if ticking {
        "wfi"
    } else {
        "spin"
    }
}

// ----------------------------------------------------------------------------- backing

/// Guest backing in physically contiguous frames, reached through the kernel's physical map.
struct Backing {
    ptr: *mut u8,
    len: usize,
    pages: Vec<u64>,
}

fn backing(len: usize) -> Result<Backing, &'static str> {
    let count = len.div_ceil(4096);
    let mut out = alloc::vec![None; count];
    let first = crate::memory::frame_allocator::allocate_contiguous_frames(count, &mut out)
        .ok_or("no contiguous frames for spike backing")?;
    let phys = first.start_address().as_u64();
    let ptr = super::phys_to_kern_virt(phys) as *mut u8;
    let pages = (0..count as u64).map(|i| phys + i * 4096).collect();
    Ok(Backing { ptr, len, pages })
}

impl Backing {
    fn fill(&self, pattern: [u8; 4]) {
        for i in (0..self.len).step_by(4) {
            unsafe { core::ptr::copy_nonoverlapping(pattern.as_ptr(), self.ptr.add(i), 4) };
        }
        dma_cache_clean(self.ptr, self.len);
    }

    fn bytes(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.ptr, self.len) }
    }

    fn poisoned(&self) -> usize {
        dma_cache_invalidate(self.ptr, self.len);
        self.bytes().chunks_exact(4).filter(|p| *p == POISON).count()
    }
}

// ----------------------------------------------------------------------------- device

fn create(
    id: u32,
    target: u32,
    fmt: u32,
    bind: u32,
    w: u32,
    h: u32,
    b: Option<&Backing>,
) -> Result<(), &'static str> {
    with_device_state(|s| {
        virgl_resource_create_3d_cmd(s, id, target, fmt, bind, w, h, 1, 1)?;
        if let Some(b) = b {
            virgl_attach_backing_from_pages(s, id, &b.pages, b.len)?;
        }
        virgl_ctx_attach_resource_cmd(s, CTX, id)
    })
}

/// A fenced 3D transfer of the box (0, 0, w, h); returns (fence, response fence, response flags).
fn transfer(dir: u32, id: u32, w: u32, h: u32, stride: u32) -> Result<(u64, u64, u32), &'static str> {
    with_device_state(|s| {
        let fence = s.next_fence_id;
        s.next_fence_id += 1;
        let c = VirtioGpuTransferHost3d {
            hdr: VirtioGpuCtrlHdr {
                type_: dir,
                flags: VIRTIO_GPU_FLAG_FENCE,
                fence_id: fence,
                ctx_id: CTX,
                padding: 0,
            },
            box_x: 0,
            box_y: 0,
            box_z: 0,
            box_w: w,
            box_h: h,
            box_d: 1,
            offset: 0,
            resource_id: id,
            level: 0,
            stride,
            layer_stride: 0,
        };
        let r = send_owned_command_expect_ok(s, &c)?;
        Ok((fence, r.fence_id, r.flags))
    })
}

/// A fenced SUBMIT_3D in `ctx`; returns (fence, response fence).
fn submit(ctx: u32, cb: &CommandBuffer) -> Result<(u64, u64), &'static str> {
    if cb.len() >= CB_CAPACITY {
        return Err("VirGL command buffer overflow");
    }
    with_device_state(|s| virgl_submit_3d_cmd(s, ctx, cb.as_slice()))
        .map(|r| (r.submit_id, r.resp_fence))
}

fn flush(id: u32, w: u32, h: u32) -> Result<(), &'static str> {
    with_device_state(|s| {
        let c = VirtioGpuResourceFlush {
            hdr: VirtioGpuCtrlHdr {
                type_: cmd::RESOURCE_FLUSH,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            r_x: 0,
            r_y: 0,
            r_width: w,
            r_height: h,
            resource_id: id,
            padding: 0,
        };
        send_owned_command_expect_ok(s, &c).map(|_| ())
    })
}

// ----------------------------------------------------------------------------- formulas

fn t8(x: u32, y: u32) -> u32 {
    (x * 4 + y * 13) & 255
}

/// The colour textures' channels as (r, g, b, a).
fn tb(x: u32, y: u32) -> [u32; 4] {
    [((x * 3) ^ (y * 5)) & 255, (x + y * 29) & 255, (x * 7 + y) & 255, (255 - x - y * 3) & 255]
}

fn texfetch_px(x: u32, y: u32) -> [u32; 4] {
    let c = tb(x, y);
    [c[0], c[1], t8(x, y), c[3]]
}

/// 32 x 32 -> 64-bit multiply from 16 x 16-bit partial products: (low word, high word).
fn mul64(a: u32, b: u32) -> (u32, u32) {
    let (al, ah, bl, bh) = (a & 0xFFFF, a >> 16, b & 0xFFFF, b >> 16);
    let (ll, lh, hl, hh) = (al * bl, al * bh, ah * bl, ah * bh);
    let lo1 = ll.wrapping_add(lh << 16);
    let c1 = (lo1 < ll) as u32;
    let lo2 = lo1.wrapping_add(hl << 16);
    let c2 = (lo2 < lo1) as u32;
    let hi = hh
        .wrapping_add(lh >> 16)
        .wrapping_add(hl >> 16)
        .wrapping_add(c1)
        .wrapping_add(c2);
    (lo2, hi)
}

fn integer_px(x: u32, y: u32) -> [u32; 4] {
    let a = x
        .wrapping_mul(2654435761)
        .wrapping_add(y.wrapping_mul(2246822519))
        .wrapping_add(374761393);
    let b = (x ^ (y << 8)).wrapping_mul(3266489917).wrapping_add(668265263);
    let (lo, hi) = mul64(a, b);
    debug_assert_eq!(((a as u64) * (b as u64)) as u32, lo);
    [lo & 255, (lo >> 16) & 255, (hi >> 8) & 255, hi >> 24]
}

fn background_px(x: u32, y: u32) -> [u32; 4] {
    [(x * 5 + y * 11) & 255, (x * 9) & 255, (y * 17) & 255, 255]
}

fn blend_px(x: u32, y: u32) -> [u32; 4] {
    let s = [(x * 13) & 255, (y * 31 + x) & 255, (x ^ (y * 7)) & 255, (x * 4 + y * 9) & 255];
    let d = background_px(x, y);
    let a = s[3];
    let mut out = [0u32; 4];
    for c in 0..3 {
        out[c] = (s[c] * a + d[c] * (255 - a) + 127) / 255;
    }
    out[3] = a + (d[3] * (255 - a) + 127) / 255;
    out
}

fn unorm_px(x: u32) -> [u32; 4] {
    [x & 255, (255u32.wrapping_sub(x)) & 255, (x * 7 + 3) & 255, (x * 37 + 11) & 255]
}

/// Edge function of edge (a -> b) at p; positive on the interior side of TP, T1 and T2.
fn edge(a: (i64, i64), b: (i64, i64), p: (i64, i64)) -> i64 {
    (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)
}

/// The shader predicate triangle: Some(colour) inside (all edge functions >= 0), else None.
fn tri_pred_px(x: u32, y: u32) -> Option<[u32; 4]> {
    let p = (2 * x as i64 + 1, 2 * y as i64 + 1);
    let e = [edge(TP[0], TP[1], p), edge(TP[1], TP[2], p), edge(TP[2], TP[0], p)];
    if e.iter().any(|&v| v < 0) {
        return None;
    }
    let w = |v: i64| (v as i32) as u32;
    Some([
        w(e[0] >> 2) & 255,
        w(e[1] >> 3) & 255,
        w(e[2]) & 255,
        (3 * p.0 as u32 + p.1 as u32) & 255,
    ])
}

/// Device coverage under the top-left rule (D3D's; GL leaves ties to the implementation).
/// Returns (covered, centre lies on an edge).
fn hw_cover(t: [(u32, u32); 3], x: u32, y: u32) -> (bool, bool) {
    let v = t.map(|(a, b)| (2 * a as i64, 2 * b as i64));
    let p = (2 * x as i64 + 1, 2 * y as i64 + 1);
    let mut tie = false;
    for i in 0..3 {
        let (a, b) = (v[i], v[(i + 1) % 3]);
        let e = edge(a, b, p);
        if e < 0 {
            return (false, false);
        }
        if e == 0 {
            tie = true;
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let top_or_left = dy < 0 || (dy == 0 && dx > 0);
            if !top_or_left {
                return (false, true);
            }
        }
    }
    (true, tie)
}

fn scene_a_px(x: u32, y: u32, corrupt: bool) -> Option<[u32; 4]> {
    if corrupt && (x, y) == CORRUPT_AT {
        return Some(C_CORRUPT);
    }
    let v = match y {
        0..16 if x < TW => texfetch_px(x, y),
        0..16 if x < 192 => return None,
        16..32 => integer_px(x, y - 16),
        32..48 if x < TW => blend_px(x, y - 32),
        48..64 => unorm_px(x),
        64..96 if x < 96 => tri_pred_px(x, y).unwrap_or(CLR_A),
        64..96 => return None,
        96..128 => {
            let mut c = CLR_A;
            for (rx, ry, rw, rh, col) in RECTS {
                if x >= rx && x < rx + rw && y >= ry && y < ry + rh {
                    c = col;
                }
            }
            c
        }
        128..144 => {
            let (x0, y0, x1, y1) = SCISSOR;
            if x >= x0 && x < x1 && y >= y0 && y < y1 {
                C_SCISSOR
            } else {
                CLR_A
            }
        }
        _ => CLR_A,
    };
    Some(v)
}

fn scene_b_px(x: u32, y: u32) -> Option<[u32; 4]> {
    Some(match y {
        0..16 if x < TW => texfetch_px(x, y),
        16..32 => integer_px(x, y - 16),
        32..48 if x < TW => blend_px(x, y - 32),
        48..64 => unorm_px(x),
        _ => CLR_B,
    })
}

fn banner(i: u32) -> [u32; 4] {
    [(i * 37 + 20) & 255, (i * 91 + 70) & 255, (255 - i * 13) & 255, 255]
}

fn present_px(x: u32, y: u32, w: u32, h: u32) -> Option<[u32; 4]> {
    if x >= 32 && x < w - 32 && y >= 32 && y < h - 32 {
        return Some(integer_px(x, y.wrapping_sub(16)));
    }
    Some(match y {
        0..16 => unorm_px(x),
        16..32 if x % 80 < 72 => banner(x / 80),
        _ => CLR_P,
    })
}

fn upload_px(x: u32, y: u32) -> [u32; 4] {
    [x & 255, y & 255, ((x >> 8) ^ (y >> 8) ^ 0x5A) & 255, (x + y) & 255]
}

// ----------------------------------------------------------------------------- judging

const FNV_INIT: u64 = 0xcbf2_9ce4_8422_2325;

fn fnv(h: u64, b: u8) -> u64 {
    (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3)
}

struct Verdict {
    judged: u64,
    mismatched: u64,
    poisoned: u64,
    first: Option<(u32, u32, [u32; 4], [u32; 4])>,
    fnv_got: u64,
    fnv_want: u64,
}

/// Compares the backing (rows of `w` pixels in `order`, BGRA if `bgra`) with `want` over the
/// rectangle [x0, x1) x [y0, y1); pixels where `want` is None are not judged.
fn judge(
    b: &Backing,
    w: u32,
    bgra: bool,
    rect: (u32, u32, u32, u32),
    want: &dyn Fn(u32, u32) -> Option<[u32; 4]>,
) -> Verdict {
    dma_cache_invalidate(b.ptr, b.len);
    let px = b.bytes();
    let mut v = Verdict {
        judged: 0,
        mismatched: 0,
        poisoned: 0,
        first: None,
        fnv_got: FNV_INIT,
        fnv_want: FNV_INIT,
    };
    let (x0, y0, x1, y1) = rect;
    for y in y0..y1 {
        for x in x0..x1 {
            let Some(e) = want(x, y) else { continue };
            let o = ((y * w + x) * 4) as usize;
            let raw = [px[o], px[o + 1], px[o + 2], px[o + 3]];
            let got = if bgra {
                [raw[2] as u32, raw[1] as u32, raw[0] as u32, raw[3] as u32]
            } else {
                [raw[0] as u32, raw[1] as u32, raw[2] as u32, raw[3] as u32]
            };
            v.judged += 1;
            for c in 0..4 {
                v.fnv_got = fnv(v.fnv_got, got[c] as u8);
                v.fnv_want = fnv(v.fnv_want, e[c] as u8);
            }
            if raw == POISON {
                v.poisoned += 1;
            }
            if got != e {
                v.mismatched += 1;
                if v.first.is_none() {
                    v.first = Some((x, y, got, e));
                }
            }
        }
    }
    v
}

fn report(name: &str, v: &Verdict, pass: bool) {
    serial_println!(
        "GPU_SPIKE check={} judged={} mismatched={} poisoned={} fnv_got={:#018x} fnv_want={:#018x} verdict={}",
        name,
        v.judged,
        v.mismatched,
        v.poisoned,
        v.fnv_got,
        v.fnv_want,
        if pass { "PASS" } else { "FAIL" }
    );
    if let Some((x, y, got, want)) = v.first {
        serial_println!(
            "GPU_SPIKE check={} first_mismatch x={} y={} got_rgba={:?} want_rgba={:?}",
            name,
            x,
            y,
            got,
            want
        );
    }
}

static PASSED: AtomicU32 = AtomicU32::new(0);
static FAILED: AtomicU32 = AtomicU32::new(0);
static T0_US: AtomicU64 = AtomicU64::new(0);
/// Scene B's backing, for the isolation check in the spike thread.
static B_PTR: AtomicU64 = AtomicU64::new(0);
static B_LEN: AtomicU64 = AtomicU64::new(0);
static B_FNV: AtomicU64 = AtomicU64::new(0);

struct Tally;

impl Tally {
    fn record(&mut self, name: &str, v: &Verdict, pass: bool) {
        report(name, v, pass);
        self.pass(pass);
    }

    fn pass(&mut self, pass: bool) {
        if pass {
            PASSED.fetch_add(1, Ordering::Relaxed);
        } else {
            FAILED.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn fail(&mut self, name: &str, why: &str) {
        serial_println!("GPU_SPIKE check={} verdict=FAIL error={}", name, why);
        self.pass(false);
    }
}

fn elapsed(what: &str) {
    serial_println!("GPU_SPIKE elapsed_ms={} at={}", (now_us() - T0_US.load(Ordering::Relaxed)) / 1000, what);
}

// ----------------------------------------------------------------------------- shaders

const VS: &[u8] = b"VERT\nDCL IN[0]\nDCL OUT[0], POSITION\n  0: MOV OUT[0], IN[0]\n  1: END\n";

const FS_TEXFETCH: &[u8] = b"FRAG
DCL IN[0], POSITION, LINEAR
DCL OUT[0], COLOR
DCL SAMP[0]
DCL SAMP[1]
DCL SVIEW[0], 2D, FLOAT
DCL SVIEW[1], 2D, FLOAT
DCL TEMP[0..2]
IMM[0] UINT32 {0, 0, 0, 0}
  0: F2U TEMP[0].xy, IN[0].xyyy
  1: MOV TEMP[0].zw, IMM[0].xxxx
  2: TXF TEMP[1], TEMP[0], SAMP[0], 2D
  3: TXF TEMP[2], TEMP[0], SAMP[1], 2D
  4: MOV OUT[0].xyw, TEMP[2].xyyw
  5: MOV OUT[0].z, TEMP[1].xxxx
  6: END
";

// TEMP[0].xy = (x, y - 16); a in TEMP[1].x, b in TEMP[1].y; halves and partial products after.
const FS_INTEGER: &[u8] = b"FRAG
DCL IN[0], POSITION, LINEAR
DCL OUT[0], COLOR
DCL TEMP[0..7]
IMM[0] UINT32 {2654435761, 2246822519, 374761393, 4294967280}
IMM[1] UINT32 {3266489917, 668265263, 65535, 16}
IMM[2] UINT32 {8, 24, 255, 1}
IMM[3] FLT32 {0.00392156886, 0.0, 0.0, 0.0}
  0: F2U TEMP[0].xy, IN[0].xyyy
  1: UADD TEMP[0].y, TEMP[0].yyyy, IMM[0].wwww
  2: UMUL TEMP[1].x, TEMP[0].yyyy, IMM[0].yyyy
  3: UMAD TEMP[1].x, TEMP[0].xxxx, IMM[0].xxxx, TEMP[1].xxxx
  4: UADD TEMP[1].x, TEMP[1].xxxx, IMM[0].zzzz
  5: SHL TEMP[1].y, TEMP[0].yyyy, IMM[2].xxxx
  6: XOR TEMP[1].y, TEMP[0].xxxx, TEMP[1].yyyy
  7: UMAD TEMP[1].y, TEMP[1].yyyy, IMM[1].xxxx, IMM[1].yyyy
  8: AND TEMP[2].x, TEMP[1].xxxx, IMM[1].zzzz
  9: USHR TEMP[2].y, TEMP[1].xxxx, IMM[1].wwww
 10: AND TEMP[2].z, TEMP[1].yyyy, IMM[1].zzzz
 11: USHR TEMP[2].w, TEMP[1].yyyy, IMM[1].wwww
 12: UMUL TEMP[3].x, TEMP[2].xxxx, TEMP[2].zzzz
 13: UMUL TEMP[3].y, TEMP[2].xxxx, TEMP[2].wwww
 14: UMUL TEMP[3].z, TEMP[2].yyyy, TEMP[2].zzzz
 15: UMUL TEMP[3].w, TEMP[2].yyyy, TEMP[2].wwww
 16: SHL TEMP[4].x, TEMP[3].yyyy, IMM[1].wwww
 17: UADD TEMP[4].y, TEMP[3].xxxx, TEMP[4].xxxx
 18: USLT TEMP[4].z, TEMP[4].yyyy, TEMP[3].xxxx
 19: AND TEMP[4].z, TEMP[4].zzzz, IMM[2].wwww
 20: SHL TEMP[4].x, TEMP[3].zzzz, IMM[1].wwww
 21: UADD TEMP[4].w, TEMP[4].yyyy, TEMP[4].xxxx
 22: USLT TEMP[5].x, TEMP[4].wwww, TEMP[4].yyyy
 23: AND TEMP[5].x, TEMP[5].xxxx, IMM[2].wwww
 24: USHR TEMP[5].y, TEMP[3].yyyy, IMM[1].wwww
 25: USHR TEMP[5].z, TEMP[3].zzzz, IMM[1].wwww
 26: UADD TEMP[5].w, TEMP[3].wwww, TEMP[5].yyyy
 27: UADD TEMP[5].w, TEMP[5].wwww, TEMP[5].zzzz
 28: UADD TEMP[5].w, TEMP[5].wwww, TEMP[4].zzzz
 29: UADD TEMP[5].w, TEMP[5].wwww, TEMP[5].xxxx
 30: AND TEMP[6].x, TEMP[4].wwww, IMM[2].zzzz
 31: USHR TEMP[6].y, TEMP[4].wwww, IMM[1].wwww
 32: AND TEMP[6].y, TEMP[6].yyyy, IMM[2].zzzz
 33: USHR TEMP[6].z, TEMP[5].wwww, IMM[2].xxxx
 34: AND TEMP[6].z, TEMP[6].zzzz, IMM[2].zzzz
 35: USHR TEMP[6].w, TEMP[5].wwww, IMM[2].yyyy
 36: U2F TEMP[7], TEMP[6]
 37: MUL OUT[0], TEMP[7], IMM[3].xxxx
 38: END
";

const FS_BACKGROUND: &[u8] = b"FRAG
DCL IN[0], POSITION, LINEAR
DCL OUT[0], COLOR
DCL TEMP[0..2]
IMM[0] UINT32 {5, 11, 9, 17}
IMM[1] UINT32 {255, 4294967264, 0, 0}
IMM[2] FLT32 {0.00392156886, 0.0, 0.0, 0.0}
  0: F2U TEMP[0].xy, IN[0].xyyy
  1: UADD TEMP[0].y, TEMP[0].yyyy, IMM[1].yyyy
  2: UMUL TEMP[1].x, TEMP[0].yyyy, IMM[0].yyyy
  3: UMAD TEMP[1].x, TEMP[0].xxxx, IMM[0].xxxx, TEMP[1].xxxx
  4: UMUL TEMP[1].y, TEMP[0].xxxx, IMM[0].zzzz
  5: UMUL TEMP[1].z, TEMP[0].yyyy, IMM[0].wwww
  6: AND TEMP[1].xyz, TEMP[1].xyzz, IMM[1].xxxx
  7: UADD TEMP[1].w, IMM[1].xxxx, IMM[1].zzzz
  8: U2F TEMP[2], TEMP[1]
  9: MUL OUT[0], TEMP[2], IMM[2].xxxx
 10: END
";

// SAMP[0] is the GPU copy of the background; integer over(src, dst) with rounding division by 255.
const FS_BLEND: &[u8] = b"FRAG
DCL IN[0], POSITION, LINEAR
DCL OUT[0], COLOR
DCL SAMP[0]
DCL SVIEW[0], 2D, FLOAT
DCL TEMP[0..6]
IMM[0] UINT32 {13, 31, 7, 4}
IMM[1] UINT32 {9, 255, 127, 4294967264}
IMM[2] UINT32 {0, 0, 0, 0}
IMM[3] FLT32 {255.0, 0.5, 0.00392156886, 0.0}
  0: F2U TEMP[0].xy, IN[0].xyyy
  1: UADD TEMP[0].y, TEMP[0].yyyy, IMM[1].wwww
  2: MOV TEMP[0].zw, IMM[2].xxxx
  3: TXF TEMP[1], TEMP[0], SAMP[0], 2D
  4: MAD TEMP[1], TEMP[1], IMM[3].xxxx, IMM[3].yyyy
  5: F2U TEMP[1], TEMP[1]
  6: UMUL TEMP[2].x, TEMP[0].xxxx, IMM[0].xxxx
  7: UMAD TEMP[2].y, TEMP[0].yyyy, IMM[0].yyyy, TEMP[0].xxxx
  8: UMUL TEMP[2].z, TEMP[0].yyyy, IMM[0].zzzz
  9: XOR TEMP[2].z, TEMP[0].xxxx, TEMP[2].zzzz
 10: UMUL TEMP[2].w, TEMP[0].xxxx, IMM[0].wwww
 11: UMAD TEMP[2].w, TEMP[0].yyyy, IMM[1].xxxx, TEMP[2].wwww
 12: AND TEMP[2], TEMP[2], IMM[1].yyyy
 13: INEG TEMP[3].x, TEMP[2].wwww
 14: UADD TEMP[3].x, TEMP[3].xxxx, IMM[1].yyyy
 15: UMUL TEMP[4], TEMP[1], TEMP[3].xxxx
 16: UMAD TEMP[5].xyz, TEMP[2].xyzz, TEMP[2].wwww, TEMP[4].xyzz
 17: UADD TEMP[5].w, TEMP[4].wwww, IMM[2].xxxx
 18: UADD TEMP[5], TEMP[5], IMM[1].zzzz
 19: UDIV TEMP[5], TEMP[5], IMM[1].yyyy
 20: UADD TEMP[5].w, TEMP[5].wwww, TEMP[2].wwww
 21: U2F TEMP[6], TEMP[5]
 22: MUL OUT[0], TEMP[6], IMM[3].zzzz
 23: END
";

const FS_UNORM: &[u8] = b"FRAG
DCL IN[0], POSITION, LINEAR
DCL OUT[0], COLOR
DCL TEMP[0..2]
IMM[0] UINT32 {255, 7, 3, 37}
IMM[1] UINT32 {11, 0, 0, 0}
IMM[2] FLT32 {0.00392156886, 0.0, 0.0, 0.0}
  0: F2U TEMP[0].x, IN[0].xxxx
  1: UADD TEMP[1].x, TEMP[0].xxxx, IMM[1].yyyy
  2: INEG TEMP[1].y, TEMP[0].xxxx
  3: UADD TEMP[1].y, TEMP[1].yyyy, IMM[0].xxxx
  4: UMAD TEMP[1].z, TEMP[0].xxxx, IMM[0].yyyy, IMM[0].zzzz
  5: UMAD TEMP[1].w, TEMP[0].xxxx, IMM[0].wwww, IMM[1].xxxx
  6: AND TEMP[1], TEMP[1], IMM[0].xxxx
  7: U2F TEMP[2], TEMP[1]
  8: MUL OUT[0], TEMP[2], IMM[2].xxxx
  9: END
";

const FS_SOLID: &[u8] = b"FRAG
PROPERTY FS_COLOR0_WRITES_ALL_CBUFS 1
DCL OUT[0], COLOR
DCL CONST[0]
  0: MOV OUT[0], CONST[0]
  1: END
";

// Doubled centre (X, Y) = (2x + 1, 2y + 1); edge functions of TP in TEMP[3].xyz; discard
// unless all are >= 0; colour = (E0 >> 2, E1 >> 3, E2, 3X + Y), each & 255.
const FS_TRIPRED: &[u8] = b"FRAG
DCL IN[0], POSITION, LINEAR
DCL OUT[0], COLOR
DCL TEMP[0..5]
IMM[0] UINT32 {2, 1, 255, 0}
IMM[1] INT32 {-132, -140, -190, 0}
IMM[2] INT32 {-16, -180, -60, 0}
IMM[3] INT32 {164, -120, -44, 0}
IMM[4] INT32 {-8, -50, 58, 0}
IMM[5] FLT32 {1.0, -1.0, 0.00392156886, 0.0}
IMM[6] UINT32 {3, 0, 0, 0}
  0: F2U TEMP[0].xy, IN[0].xyyy
  1: UMAD TEMP[0].xy, TEMP[0].xyyy, IMM[0].xxxx, IMM[0].yyyy
  2: UADD TEMP[1].xyz, TEMP[0].yyyy, IMM[1].xyzz
  3: UADD TEMP[2].xyz, TEMP[0].xxxx, IMM[2].xyzz
  4: UMUL TEMP[3].xyz, TEMP[1].xyzz, IMM[3].xyzz
  5: UMAD TEMP[3].xyz, TEMP[2].xyzz, IMM[4].xyzz, TEMP[3].xyzz
  6: ISGE TEMP[4].xyz, TEMP[3].xyzz, IMM[0].wwww
  7: AND TEMP[4].x, TEMP[4].xxxx, TEMP[4].yyyy
  8: AND TEMP[4].x, TEMP[4].xxxx, TEMP[4].zzzz
  9: UCMP TEMP[5].x, TEMP[4].xxxx, IMM[5].xxxx, IMM[5].yyyy
 10: KILL_IF TEMP[5].xxxx
 11: ISHR TEMP[2].x, TEMP[3].xxxx, IMM[0].xxxx
 12: ISHR TEMP[2].y, TEMP[3].yyyy, IMM[6].xxxx
 13: MOV TEMP[2].z, TEMP[3].zzzz
 14: UMAD TEMP[2].w, TEMP[0].xxxx, IMM[6].xxxx, TEMP[0].yyyy
 15: AND TEMP[2], TEMP[2], IMM[0].zzzz
 16: U2F TEMP[1], TEMP[2]
 17: MUL OUT[0], TEMP[1], IMM[5].zzzz
 18: END
";

// Diagnostics for the R8 texture: all four TXF channels at x - 64, and a nearest TEX at x - 128.
const FS_PROBE_TXF: &[u8] = b"FRAG
DCL IN[0], POSITION, LINEAR
DCL OUT[0], COLOR
DCL SAMP[0]
DCL SVIEW[0], 2D, FLOAT
DCL TEMP[0..1]
IMM[0] UINT32 {4294967232, 0, 0, 0}
  0: F2U TEMP[0].xy, IN[0].xyyy
  1: UADD TEMP[0].x, TEMP[0].xxxx, IMM[0].xxxx
  2: MOV TEMP[0].zw, IMM[0].yyyy
  3: TXF TEMP[1], TEMP[0], SAMP[0], 2D
  4: MOV OUT[0], TEMP[1]
  5: END
";

const FS_PROBE_TEX: &[u8] = b"FRAG
DCL IN[0], POSITION, LINEAR
DCL OUT[0], COLOR
DCL SAMP[0]
DCL SVIEW[0], 2D, FLOAT
DCL TEMP[0]
IMM[0] FLT32 {-128.0, 0.015625, 0.0625, 0.0}
  0: ADD TEMP[0].x, IN[0].xxxx, IMM[0].xxxx
  1: MUL TEMP[0].x, TEMP[0].xxxx, IMM[0].yyyy
  2: MUL TEMP[0].y, IN[0].yyyy, IMM[0].zzzz
  3: TEX OUT[0], TEMP[0], SAMP[0], 2D
  4: END
";

const FS_PROBE_TXF0: &[u8] = b"FRAG
DCL IN[0], POSITION, LINEAR
DCL OUT[0], COLOR
DCL SAMP[0]
DCL SVIEW[0], 2D, FLOAT
DCL TEMP[0..1]
IMM[0] UINT32 {0, 0, 0, 0}
  0: F2U TEMP[0].xy, IN[0].xyyy
  1: MOV TEMP[0].zw, IMM[0].xxxx
  2: TXF TEMP[1], TEMP[0], SAMP[0], 2D
  3: MOV OUT[0], TEMP[1]
  4: END
";

// ----------------------------------------------------------------------------- encoding

fn unorm(c: [u32; 4]) -> [f32; 4] {
    c.map(|k| k as f32 / 255.0)
}

fn solid(cb: &mut CommandBuffer, c: [u32; 4]) {
    let f = unorm(c);
    cb.set_constant_buffer(pipe::SHADER_FRAGMENT, 0, &f.map(f32::to_bits));
}

fn clear(cb: &mut CommandBuffer, c: [u32; 4]) {
    let f = unorm(c);
    cb.clear_color(f[0], f[1], f[2], f[3]);
}

/// Vertex-buffer slots (4 vertices, 64 bytes each) used by one submission's draws.
struct Verts {
    tw: f32,
    th: f32,
    slot: u32,
}

impl Verts {
    fn new(tw: u32, th: u32) -> Self {
        Self { tw: tw as f32, th: th as f32, slot: 0 }
    }

    fn put(&mut self, cb: &mut CommandBuffer, pts: &[(u32, u32)]) -> u32 {
        let first = self.slot * 4;
        let one = 1.0f32.to_bits();
        let mut words = [0u32; 16];
        for (i, &(x, y)) in pts.iter().enumerate() {
            words[i * 4] = (x as f32 / (self.tw / 2.0) - 1.0).to_bits();
            words[i * 4 + 1] = (1.0 - y as f32 / (self.th / 2.0)).to_bits();
            words[i * 4 + 3] = one;
        }
        cb.resource_inline_write(RES_VB, first * 16, 64, &words);
        self.slot += 1;
        first
    }

    fn quad(&mut self, cb: &mut CommandBuffer, x0: u32, y0: u32, x1: u32, y1: u32) {
        let first = self.put(cb, &[(x0, y0), (x0, y1), (x1, y0), (x1, y1)]);
        cb.draw_vbo(first, 4, pipe::PRIM_TRIANGLE_STRIP, first + 3);
    }

    fn tri(&mut self, cb: &mut CommandBuffer, t: [(u32, u32); 3]) {
        let first = self.put(cb, &t);
        cb.draw_vbo(first, 3, pipe::PRIM_TRIANGLES, first + 2);
    }
}

/// VIRGL_CCMD_RESOURCE_COPY_REGION: dst res/level/x/y/z, src res/level/x/y/z, w/h/d.
fn copy_region(cb: &mut CommandBuffer, dst: u32, src: u32, sx: u32, sy: u32, w: u32, h: u32) {
    cb.raw(17 | (13 << 16), &[dst, 0, 0, 0, 0, src, 0, sx, sy, 0, w, h, 1]);
}

fn target(cb: &mut CommandBuffer, surface: u32, w: u32, h: u32) {
    cb.set_framebuffer_state(0, &[surface]);
    cb.set_viewport(w as f32, h as f32);
}

/// Scene A, or B with `rgba`: the four profile regions in rows 0..64.
fn profile_regions(cb: &mut CommandBuffer, v: &mut Verts, rgba: bool, suppress_texfetch: bool) {
    let (tex, copy, copy_view, rt) = if rgba {
        (V_TR, RES_COPY_B, V_COPY_B, RES_B)
    } else {
        (V_TB, RES_COPY_A, V_COPY_A, RES_A)
    };
    let width = if rgba { BW } else { AW };
    if !suppress_texfetch {
        cb.bind_shader(H_FS_TEXFETCH, pipe::SHADER_FRAGMENT);
        cb.set_sampler_views(pipe::SHADER_FRAGMENT, 0, &[V_T8, tex]);
        cb.bind_sampler_states(pipe::SHADER_FRAGMENT, 0, &[H_SAMPLER, H_SAMPLER]);
        v.quad(cb, 0, 0, TW, TH);
    }
    cb.bind_shader(H_FS_INTEGER, pipe::SHADER_FRAGMENT);
    v.quad(cb, 0, 16, width, 32);
    cb.bind_shader(H_FS_BACKGROUND, pipe::SHADER_FRAGMENT);
    v.quad(cb, 0, 32, TW, 48);
    copy_region(cb, copy, rt, 0, 32, TW, TH);
    cb.bind_shader(H_FS_BLEND, pipe::SHADER_FRAGMENT);
    cb.set_sampler_views(pipe::SHADER_FRAGMENT, 0, &[copy_view]);
    cb.bind_sampler_states(pipe::SHADER_FRAGMENT, 0, &[H_SAMPLER]);
    v.quad(cb, 0, 32, TW, 48);
    cb.bind_shader(H_FS_UNORM, pipe::SHADER_FRAGMENT);
    v.quad(cb, 0, 48, width, 64);
}

#[derive(Clone, Copy, PartialEq)]
enum Fault {
    None,
    CorruptPixel,
    SuppressDraw,
}

fn scene_a(fault: Fault) -> Box<CommandBuffer> {
    let mut cb = Box::new(CommandBuffer::new());
    let mut v = Verts::new(AW, AH);
    cb.set_sub_ctx(1);
    target(&mut cb, S_A, AW, AH);
    cb.bind_object(H_RAST, OBJ_RASTERIZER);
    clear(&mut cb, CLR_A);
    profile_regions(&mut cb, &mut v, false, fault == Fault::SuppressDraw);
    cb.bind_shader(H_FS_PROBE_TXF, pipe::SHADER_FRAGMENT);
    cb.set_sampler_views(pipe::SHADER_FRAGMENT, 0, &[V_T8]);
    cb.bind_sampler_states(pipe::SHADER_FRAGMENT, 0, &[H_SAMPLER]);
    v.quad(&mut cb, 64, 0, 128, TH);
    cb.bind_shader(H_FS_PROBE_TEX, pipe::SHADER_FRAGMENT);
    v.quad(&mut cb, 128, 0, 192, TH);
    cb.bind_shader(H_FS_TRIPRED, pipe::SHADER_FRAGMENT);
    v.quad(&mut cb, 0, 64, 96, 96);
    cb.bind_shader(H_FS_SOLID, pipe::SHADER_FRAGMENT);
    solid(&mut cb, C_T1);
    v.tri(&mut cb, T1);
    solid(&mut cb, C_T2);
    v.tri(&mut cb, T2);
    for (x, y, w, h, c) in RECTS {
        solid(&mut cb, c);
        v.quad(&mut cb, x, y, x + w, y + h);
    }
    cb.bind_object(H_RAST_SCISSOR, OBJ_RASTERIZER);
    let (sx0, sy0, sx1, sy1) = SCISSOR;
    cb.set_scissor_state(sx0, sy0, sx1, sy1);
    solid(&mut cb, C_SCISSOR);
    v.quad(&mut cb, 0, 128, AW, 144);
    cb.bind_object(H_RAST, OBJ_RASTERIZER);
    if fault == Fault::CorruptPixel {
        solid(&mut cb, C_CORRUPT);
        v.quad(&mut cb, CORRUPT_AT.0, CORRUPT_AT.1, CORRUPT_AT.0 + 1, CORRUPT_AT.1 + 1);
    }
    cb
}

fn scene_b() -> Box<CommandBuffer> {
    let mut cb = Box::new(CommandBuffer::new());
    let mut v = Verts::new(BW, BH);
    cb.set_sub_ctx(1);
    target(&mut cb, S_B, BW, BH);
    cb.bind_object(H_RAST, OBJ_RASTERIZER);
    clear(&mut cb, CLR_B);
    profile_regions(&mut cb, &mut v, true, false);
    cb
}

fn scene_p(w: u32, h: u32) -> Box<CommandBuffer> {
    let mut cb = Box::new(CommandBuffer::new());
    let mut v = Verts::new(w, h);
    cb.set_sub_ctx(1);
    target(&mut cb, S_P, w, h);
    cb.bind_object(H_RAST, OBJ_RASTERIZER);
    clear(&mut cb, CLR_P);
    cb.bind_shader(H_FS_UNORM, pipe::SHADER_FRAGMENT);
    v.quad(&mut cb, 0, 0, w, 16);
    cb.bind_shader(H_FS_SOLID, pipe::SHADER_FRAGMENT);
    let mut i = 0;
    while i * 80 < w {
        solid(&mut cb, banner(i));
        v.quad(&mut cb, i * 80, 16, (i * 80 + 72).min(w), 32);
        i += 1;
    }
    cb.bind_shader(H_FS_INTEGER, pipe::SHADER_FRAGMENT);
    v.quad(&mut cb, 32, 32, w - 32, h - 32);
    cb
}

fn scene_g(draws: u32) -> Box<CommandBuffer> {
    let mut cb = Box::new(CommandBuffer::new());
    let mut v = Verts::new(GW, GH);
    cb.set_sub_ctx(1);
    target(&mut cb, S_G, GW, GH);
    cb.bind_object(H_RAST, OBJ_RASTERIZER);
    clear(&mut cb, CLR_G);
    cb.bind_shader(H_FS_INTEGER, pipe::SHADER_FRAGMENT);
    for _ in 0..draws {
        v.quad(&mut cb, 0, 0, GW, GH);
    }
    cb
}

fn setup_objects() -> Box<CommandBuffer> {
    let mut cb = Box::new(CommandBuffer::new());
    cb.create_sub_ctx(1);
    cb.set_sub_ctx(1);
    cb.create_surface(S_A, RES_A, vfmt::B8G8R8A8_UNORM, 0, 0);
    cb.create_surface(S_B, RES_B, vfmt::R8G8B8A8_UNORM, 0, 0);
    // Blending off, all channels written, dither off (profile items 1 and 7).
    cb.raw(1 | (1 << 8) | (11 << 16), &[H_BLEND, 0, 0, 0xF << 27, 0, 0, 0, 0, 0, 0, 0]);
    cb.create_dsa_default(H_DSA);
    // Half-pixel centres (bit 29); the scissored variant adds the scissor enable (bit 14).
    cb.create_rasterizer(H_RAST, 1 << 29);
    cb.create_rasterizer(H_RAST_SCISSOR, (1 << 29) | (1 << 14));
    cb.create_vertex_elements(H_VE, &[(0, 0, 0, vfmt::R32G32B32A32_FLOAT)]);
    cb.create_sampler_view(V_T8, RES_T8, vfmt::R8_UNORM, pipe::TEXTURE_2D, 0, 0, 0, 0, swizzle::IDENTITY);
    cb.create_sampler_view(V_TB, RES_TB, vfmt::B8G8R8A8_UNORM, pipe::TEXTURE_2D, 0, 0, 0, 0, swizzle::IDENTITY);
    cb.create_sampler_view(V_COPY_A, RES_COPY_A, vfmt::B8G8R8A8_UNORM, pipe::TEXTURE_2D, 0, 0, 0, 0, swizzle::IDENTITY);
    cb.create_sampler_view(V_TR, RES_TR, vfmt::R8G8B8A8_UNORM, pipe::TEXTURE_2D, 0, 0, 0, 0, swizzle::IDENTITY);
    cb.create_sampler_view(V_COPY_B, RES_COPY_B, vfmt::R8G8B8A8_UNORM, pipe::TEXTURE_2D, 0, 0, 0, 0, swizzle::IDENTITY);
    cb.create_sampler_state(
        H_SAMPLER,
        pipe::TEX_WRAP_CLAMP_TO_EDGE,
        pipe::TEX_WRAP_CLAMP_TO_EDGE,
        pipe::TEX_WRAP_CLAMP_TO_EDGE,
        pipe::TEX_FILTER_NEAREST,
        pipe::TEX_MIPFILTER_NONE,
        pipe::TEX_FILTER_NEAREST,
    );
    cb.bind_object(H_BLEND, OBJ_BLEND);
    cb.bind_object(H_DSA, OBJ_DSA);
    cb.bind_object(H_RAST, OBJ_RASTERIZER);
    cb.bind_object(H_VE, OBJ_VERTEX_ELEMENTS);
    cb.set_min_samples(1);
    cb.set_vertex_buffers(&[(16, 0, RES_VB)]);
    target(&mut cb, S_A, AW, AH);
    cb
}

/// Creates a surface for a resource made after the setup submission.
fn surface(handle: u32, res: u32) -> Result<(), &'static str> {
    let mut cb = Box::new(CommandBuffer::new());
    cb.set_sub_ctx(1);
    cb.create_surface(handle, res, vfmt::B8G8R8A8_UNORM, 0, 0);
    submit(CTX, &cb).map(|_| ())
}

fn setup_shaders() -> Box<CommandBuffer> {
    let mut cb = Box::new(CommandBuffer::new());
    cb.set_sub_ctx(1);
    cb.create_shader(H_VS, pipe::SHADER_VERTEX, 300, VS);
    cb.create_shader(H_FS_TEXFETCH, pipe::SHADER_FRAGMENT, 1024, FS_TEXFETCH);
    cb.create_shader(H_FS_INTEGER, pipe::SHADER_FRAGMENT, 1024, FS_INTEGER);
    cb.create_shader(H_FS_BACKGROUND, pipe::SHADER_FRAGMENT, 1024, FS_BACKGROUND);
    cb.create_shader(H_FS_BLEND, pipe::SHADER_FRAGMENT, 1024, FS_BLEND);
    cb.create_shader(H_FS_UNORM, pipe::SHADER_FRAGMENT, 1024, FS_UNORM);
    cb.create_shader(H_FS_SOLID, pipe::SHADER_FRAGMENT, 300, FS_SOLID);
    cb.create_shader(H_FS_TRIPRED, pipe::SHADER_FRAGMENT, 1024, FS_TRIPRED);
    cb.create_shader(H_FS_PROBE_TXF, pipe::SHADER_FRAGMENT, 300, FS_PROBE_TXF);
    cb.create_shader(H_FS_PROBE_TEX, pipe::SHADER_FRAGMENT, 300, FS_PROBE_TEX);
    cb.bind_shader(H_VS, pipe::SHADER_VERTEX);
    cb
}

// ----------------------------------------------------------------------------- the run

/// Runs the spike once when VirGL owns the display.
pub fn run() {
    T0_US.store(now_us(), Ordering::Relaxed);
    serial_println!("GPU_SPIKE begin target=parallels ctx={}", CTX);
    let mut tally = Tally;
    match run_inner(&mut tally) {
        Ok(()) => {
            elapsed("boot-part-end");
            if crate::task::kthread::kthread_run(late, "gpu-spike").is_err() {
                tally.fail("spawn", "kthread_run failed");
                done();
            }
        }
        Err(e) => {
            tally.fail("run", e);
            done();
        }
    }
}

fn done() {
    serial_println!(
        "GPU_SPIKE DONE passed={} failed={}",
        PASSED.load(Ordering::Relaxed),
        FAILED.load(Ordering::Relaxed)
    );
}

/// The spike thread: 2560x1600, the attestation window and isolation, with real sleeps.
fn late() {
    let mut tally = Tally;
    sleep_ms(2000);
    elapsed("late-begin");
    if let Err(e) = r8_variants() {
        serial_println!("GPU_SPIKE info=r8-variants error={}", e);
    }
    pitch_probes();
    if let Err(e) = big(&mut tally) {
        tally.fail("2560x1600", e);
    }
    if let Err(e) = isolation(&mut tally) {
        tally.fail("isolation", e);
    }
    done();
}

fn capsets() {
    for index in 0..4 {
        let Ok((id, ver, size)) = get_capset_info(index) else { break };
        serial_println!("GPU_SPIKE capset index={} id={} max_version={} max_size={}", index, id, ver, size);
        if id != 2 && id != 1 {
            continue;
        }
        let Ok(resp) = get_capset(id, ver) else { continue };
        if resp != cmd::RESP_OK_CAPSET {
            serial_println!("GPU_SPIKE capset id={} GET_CAPSET resp={:#x}", id, resp);
            continue;
        }
        let resp = unsafe { &*(&raw const PCI_RESP_BUF) };
        let caps = &resp.data[24..];
        let word = |o: usize| u32::from_le_bytes([caps[o], caps[o + 1], caps[o + 2], caps[o + 3]]);
        let bit = |base: usize, fmt: u32| (word(base + 4 * (fmt as usize / 32)) >> (fmt % 32)) & 1;
        let (sampler, render) = (4, 68);
        serial_println!(
            "GPU_SPIKE caps id={} max_version={} glsl_level={} bset={:#x} max_render_targets={} \
             sampler(B8G8R8A8={} B8G8R8X8={} R8={} R8G8B8A8={}) render(B8G8R8A8={} B8G8R8X8={} R8={} R8G8B8A8={})",
            id,
            word(0),
            word(264),
            word(260),
            word(280),
            bit(sampler, 1),
            bit(sampler, 2),
            bit(sampler, 64),
            bit(sampler, 67),
            bit(render, 1),
            bit(render, 2),
            bit(render, 64),
            bit(render, 67)
        );
        const RENDERER: usize = 696;
        if id == 2 && size as usize >= RENDERER + 64 && RENDERER + 64 <= caps.len() {
            let name = &caps[RENDERER..RENDERER + 64];
            let len = name.iter().position(|&b| b == 0).unwrap_or(64);
            serial_println!(
                "GPU_SPIKE caps renderer=\"{}\"",
                core::str::from_utf8(&name[..len]).unwrap_or("?")
            );
        }
    }
}

fn answered(name: &str, sent: u64, got: u64) -> bool {
    let ok = got >= sent;
    if !ok {
        serial_println!("GPU_SPIKE {} fence={} answered_fence={} NOT ANSWERED", name, sent, got);
    }
    ok
}

/// Poisons `b`, submits `cb`, reads the target back (unless `skip_transfer`) and judges it.
fn render_and_judge(
    name: &str,
    cb: &CommandBuffer,
    res: u32,
    b: &Backing,
    w: u32,
    h: u32,
    bgra: bool,
    skip_transfer: bool,
    want: &dyn Fn(u32, u32) -> Option<[u32; 4]>,
) -> Result<Verdict, &'static str> {
    b.fill(POISON);
    let t0 = now_us();
    let (sf, sa) = submit(CTX, cb)?;
    let t1 = now_us();
    let control = b.poisoned();
    let mut t2 = t1;
    let mut fences_ok = answered(name, sf, sa);
    if !skip_transfer {
        let (tf, ta, flags) = transfer(cmd::TRANSFER_FROM_HOST_3D, res, w, h, w * 4)?;
        t2 = now_us();
        fences_ok &= answered(name, tf, ta) && flags & VIRTIO_GPU_FLAG_FENCE != 0;
    }
    serial_println!(
        "GPU_SPIKE {} submit_dwords={} submit_us={} readback_us={} backing_poisoned_after_render={}/{} fences_answered={}",
        name,
        cb.len(),
        t1 - t0,
        t2 - t1,
        control,
        w * h,
        fences_ok
    );
    if !fences_ok {
        return Err("fence not answered");
    }
    Ok(judge(b, w, bgra, (0, 0, w, h), want))
}

fn run_inner(tally: &mut Tally) -> Result<(), &'static str> {
    let (dw, dh) = dimensions().ok_or("no display dimensions")?;
    serial_println!("GPU_SPIKE display={}x{}", dw, dh);
    capsets();

    let t = now_us();
    with_device_state(|s| virgl_ctx_create_cmd(s, CTX, b"gpu-spike"))?;
    serial_println!("GPU_SPIKE ctx_create ctx={} us={}", CTX, now_us() - t);

    let ba = backing((AW * AH * 4) as usize)?;
    let bt8 = backing((T8_PITCH * TH) as usize)?;
    let btb = backing((TW * TH * 4) as usize)?;
    let bca = backing((TW * TH * 4) as usize)?;
    let bb = backing((BW * BH * 4) as usize)?;
    let btr = backing((TW * TH * 4) as usize)?;
    let bcb = backing((TW * TH * 4) as usize)?;
    let rt = pipe::BIND_RENDER_TARGET | pipe::BIND_SAMPLER_VIEW;
    let tex = pipe::BIND_SAMPLER_VIEW;
    let tex2d = pipe::TEXTURE_2D;
    create(RES_A, tex2d, vfmt::B8G8R8A8_UNORM, rt, AW, AH, Some(&ba))?;
    create(RES_T8, tex2d, vfmt::R8_UNORM, tex, TW, TH, Some(&bt8))?;
    create(RES_TB, tex2d, vfmt::B8G8R8A8_UNORM, tex, TW, TH, Some(&btb))?;
    create(RES_COPY_A, tex2d, vfmt::B8G8R8A8_UNORM, rt, TW, TH, Some(&bca))?;
    create(RES_VB, pipe::BUFFER, vfmt::R8_UNORM, pipe::BIND_VERTEX_BUFFER, VB_BYTES, 1, None)?;
    create(RES_B, tex2d, vfmt::R8G8B8A8_UNORM, rt, BW, BH, Some(&bb))?;
    B_PTR.store(bb.ptr as u64, Ordering::Relaxed);
    B_LEN.store(bb.len as u64, Ordering::Relaxed);
    create(RES_TR, tex2d, vfmt::R8G8B8A8_UNORM, tex, TW, TH, Some(&btr))?;
    create(RES_COPY_B, tex2d, vfmt::R8G8B8A8_UNORM, rt, TW, TH, Some(&bcb))?;
    serial_println!("GPU_SPIKE resources created and attached to ctx {} only", CTX);

    // Texture uploads: R8, B8G8R8A8 and R8G8B8A8 carry the same channel values.
    unsafe {
        for y in 0..TH {
            for x in 0..TW {
                let i = (y * TW + x) as usize;
                *bt8.ptr.add((y * T8_PITCH + x) as usize) = t8(x, y) as u8;
                let [r, g, b, a] = tb(x, y).map(|c| c as u8);
                core::ptr::copy_nonoverlapping([b, g, r, a].as_ptr(), btb.ptr.add(i * 4), 4);
                core::ptr::copy_nonoverlapping([r, g, b, a].as_ptr(), btr.ptr.add(i * 4), 4);
            }
        }
    }
    for b in [&bt8, &btb, &btr] {
        dma_cache_clean(b.ptr, b.len);
    }
    let t = now_us();
    transfer(cmd::TRANSFER_TO_HOST_3D, RES_T8, TW, TH, T8_PITCH)?;
    transfer(cmd::TRANSFER_TO_HOST_3D, RES_TB, TW, TH, TW * 4)?;
    transfer(cmd::TRANSFER_TO_HOST_3D, RES_TR, TW, TH, TW * 4)?;
    serial_println!("GPU_SPIKE uploads textures=3 us={}", now_us() - t);
    bt8.fill(POISON);
    let (rf, ra, _) = transfer(cmd::TRANSFER_FROM_HOST_3D, RES_T8, TW, TH, T8_PITCH)?;
    dma_cache_invalidate(bt8.ptr, bt8.len);
    let raw = bt8.bytes();
    let bad = (0..TW * TH)
        .filter(|&i| raw[((i / TW) * T8_PITCH + i % TW) as usize] as u32 != t8(i % TW, i / TW))
        .count();
    serial_println!(
        "GPU_SPIKE check=r8-texture-readback bytes={} mismatched={} answered={} first16={:?} verdict={}",
        TW * TH,
        bad,
        ra >= rf,
        &raw[..16],
        if bad == 0 && ra >= rf { "PASS" } else { "FAIL" }
    );
    tally.pass(bad == 0 && ra >= rf);

    let objects = setup_objects();
    let shaders = setup_shaders();
    serial_println!("GPU_SPIKE setup dwords objects={} shaders={}", objects.len(), shaders.len());
    submit(CTX, &objects)?;
    submit(CTX, &shaders)?;

    // Scene A: B8G8R8A8, every profile region, judged exactly.
    let want_a = |x: u32, y: u32| scene_a_px(x, y, false);
    let a = scene_a(Fault::None);
    let v = render_and_judge("sceneA", &a, RES_A, &ba, AW, AH, true, false, &want_a)?;
    tally.record("sceneA-bgra8-all-regions", &v, v.mismatched == 0 && v.poisoned == 0);
    for (name, rect) in [
        ("sceneA/clear", (192, 0, 256, 16)),
        ("sceneA/texfetch-r8-bgra", (0, 0, 64, 16)),
        ("sceneA/integer-two-word-mul", (0, 16, 256, 32)),
        ("sceneA/blend-dest-copy", (0, 32, 64, 48)),
        ("sceneA/unorm-256", (0, 48, 256, 64)),
        ("sceneA/triangle-predicate", (0, 64, 96, 96)),
        ("sceneA/rects-order-alpha", (0, 96, 256, 128)),
        ("sceneA/scissor", (0, 128, 256, 144)),
        ("sceneA/clear-rest", (0, 144, 256, 256)),
    ] {
        let r = judge(&ba, AW, true, rect, &want_a);
        report(name, &r, r.mismatched == 0 && r.poisoned == 0);
    }
    for (name, x0) in [("r8-probe-txf-rgba", 64u32), ("r8-probe-tex-nearest-rgba", 128)] {
        let r = judge(&ba, AW, true, (x0, 0, x0 + TW, TH), &|x, y| Some([t8(x - x0, y), 0, 0, 255]));
        report(name, &r, r.mismatched == 0);
        let px = ba.bytes();
        let at = |x: u32, y: u32| {
            let o = ((y * AW + x) * 4) as usize;
            [px[o + 2], px[o + 1], px[o], px[o + 3]]
        };
        serial_println!(
            "GPU_SPIKE info={} samples (dx,dy,t8)->rgba: (1,0,{})->{:?} (2,0,{})->{:?} (6,3,{})->{:?} (63,15,{})->{:?}",
            name,
            t8(1, 0),
            at(x0 + 1, 0),
            t8(2, 0),
            at(x0 + 2, 0),
            t8(6, 3),
            at(x0 + 6, 3),
            t8(63, 15),
            at(x0 + 63, 15)
        );
    }
    let clean: Vec<u8> = ba.bytes().to_vec();
    let diffs = |b: &Backing| -> Vec<(u32, u32)> {
        b.bytes()
            .chunks_exact(4)
            .zip(clean.chunks_exact(4))
            .enumerate()
            .filter(|(_, (p, q))| p != q)
            .map(|(i, _)| (i as u32 % AW, i as u32 / AW))
            .collect()
    };

    // Device-rasterized non-axis-aligned triangles: the profile makes no claim; recorded only.
    let mut ties = 0u64;
    let hw = judge(&ba, AW, true, (96, 64, 256, 96), &|x, y| {
        let (c1, _) = hw_cover(T1, x, y);
        let (c2, _) = hw_cover(T2, x, y);
        Some(if c1 {
            C_T1
        } else if c2 {
            C_T2
        } else {
            CLR_A
        })
    });
    for y in 64..96 {
        for x in 96..256 {
            if hw_cover(T1, x, y).1 || hw_cover(T2, x, y).1 {
                ties += 1;
            }
        }
    }
    serial_println!(
        "GPU_SPIKE info=hw-triangles-top-left-rule judged={} mismatched={} centres_on_edges={}",
        hw.judged,
        hw.mismatched,
        ties
    );
    if let Some((x, y, got, want)) = hw.first {
        serial_println!("GPU_SPIKE info=hw-triangles first_mismatch x={} y={} got={:?} want={:?}", x, y, got, want);
    }

    // Determinism: the same stream again must give identical bytes.
    let v2 = render_and_judge("sceneA-again", &a, RES_A, &ba, AW, AH, true, false, &want_a)?;
    let d2 = diffs(&ba);
    serial_println!("GPU_SPIKE sceneA-again pixels_differing_from_first_run={}", d2.len());
    tally.record("sceneA-determinism", &v2, d2.is_empty() && v2.fnv_got == v.fnv_got);

    // Scene B: R8G8B8A8 target and texture, same formulas, RGBA byte order.
    let b = scene_b();
    let vb = render_and_judge("sceneB", &b, RES_B, &bb, BW, BH, false, false, &scene_b_px)?;
    tally.record("sceneB-rgba8-all-regions", &vb, vb.mismatched == 0 && vb.poisoned == 0);
    B_FNV.store(vb.fnv_got, Ordering::Relaxed);

    // Planted faults: each must be caught by the exact comparison.
    let fc = render_and_judge("fault-corrupt-pixel", &scene_a(Fault::CorruptPixel), RES_A, &ba, AW, AH, true, false, &want_a)?;
    let dc = diffs(&ba);
    let caught = fc.mismatched == v.mismatched + 1 && dc == [CORRUPT_AT];
    serial_println!("GPU_SPIKE fault-corrupt-pixel pixels_differing_from_clean={} at={:?}", dc.len(), dc.first());
    tally.record("fault-corrupt-pixel-caught", &fc, caught);
    let fs = render_and_judge("fault-suppress-draw", &scene_a(Fault::SuppressDraw), RES_A, &ba, AW, AH, true, false, &want_a)?;
    let ds = diffs(&ba);
    let inside = ds.iter().all(|&(x, y)| x < TW && y < TH);
    serial_println!("GPU_SPIKE fault-suppress-draw pixels_differing_from_clean={} all_in_texfetch_region={}", ds.len(), inside);
    tally.record("fault-suppress-draw-caught", &fs, fs.mismatched > v.mismatched && !ds.is_empty() && inside);
    let fk = render_and_judge("fault-skip-transfer", &a, RES_A, &ba, AW, AH, true, true, &want_a)?;
    tally.record("fault-skip-transfer-caught", &fk, fk.poisoned == fk.judged && fk.judged > 0);

    elapsed("before-present");
    if let Err(e) = present(tally, dw, dh) {
        tally.fail("present", e);
    }

    // Submit latency: fenced submissions of one SET_SUB_CTX.
    let mut nop = Box::new(CommandBuffer::new());
    nop.set_sub_ctx(1);
    let mut lat: Vec<u64> = Vec::with_capacity(200);
    for _ in 0..200 {
        let t = now_us();
        let (f, af) = submit(CTX, &nop)?;
        lat.push(now_us() - t);
        if af < f {
            return Err("empty submission fence not answered");
        }
    }
    lat.sort_unstable();
    serial_println!(
        "GPU_SPIKE time=submit-latency n=200 min_us={} median_us={} p99_us={} max_us={}",
        lat[0],
        lat[100],
        lat[198],
        lat[199]
    );

    Ok(())
}

fn isolation(tally: &mut Tally) -> Result<(), &'static str> {
    let bb = Backing {
        ptr: B_PTR.load(Ordering::Relaxed) as *mut u8,
        len: B_LEN.load(Ordering::Relaxed) as usize,
        pages: Vec::new(),
    };
    // Isolation: context 3 draws to scene B's target, which is not attached to it.
    with_device_state(|s| virgl_ctx_create_cmd(s, CTX_FOREIGN, b"gpu-spike-foreign"))?;
    let mut foreign = Box::new(CommandBuffer::new());
    foreign.create_sub_ctx(1);
    foreign.set_sub_ctx(1);
    foreign.create_surface(1, RES_B, vfmt::R8G8B8A8_UNORM, 0, 0);
    foreign.set_framebuffer_state(0, &[1]);
    foreign.clear_color(1.0, 0.0, 1.0, 1.0);
    let fr = submit(CTX_FOREIGN, &foreign);
    serial_println!("GPU_SPIKE isolation foreign_submit result={:?}", fr.map(|_| "OK"));
    bb.fill(POISON);
    transfer(cmd::TRANSFER_FROM_HOST_3D, RES_B, BW, BH, BW * 4)?;
    let vi = judge(&bb, BW, false, (0, 0, BW, BH), &scene_b_px);
    let unchanged = vi.fnv_got == B_FNV.load(Ordering::Relaxed) && vi.poisoned == 0;
    serial_println!("GPU_SPIKE isolation sceneB_bytes_unchanged={}", unchanged);
    tally.record("isolation-foreign-context-cannot-draw", &vi, unchanged);

    Ok(())
}

fn present(tally: &mut Tally, dw: u32, dh: u32) -> Result<(), &'static str> {
    // Present: render a display-size target, bind it to scanout 0, hold, read back, restore.
    let bp = backing((dw * dh * 4) as usize)?;
    let rt = pipe::BIND_RENDER_TARGET | pipe::BIND_SAMPLER_VIEW | pipe::BIND_SCANOUT;
    create(RES_P, pipe::TEXTURE_2D, vfmt::B8G8R8A8_UNORM, rt, dw, dh, Some(&bp))?;
    surface(S_P, RES_P)?;
    let p = scene_p(dw, dh);
    let want_p = |x: u32, y: u32| present_px(x, y, dw, dh);
    let vp = render_and_judge("present", &p, RES_P, &bp, dw, dh, true, false, &want_p)?;
    let t = now_us();
    let bound = with_device_state(|s| set_scanout_resource(s, RES_P)).and_then(|_| flush(RES_P, dw, dh));
    serial_println!("GPU_SPIKE present set_scanout+flush ok={} us={} hold_begin", bound.is_ok(), now_us() - t);
    let how = idle_ms(2000);
    serial_println!("GPU_SPIKE present hold_end idle={}", how);
    bp.fill(POISON);
    let again = transfer(cmd::TRANSFER_FROM_HOST_3D, RES_P, dw, dh, dw * 4);
    let vp2 = judge(&bp, dw, true, (0, 0, dw, dh), &want_p);
    let restored = with_device_state(|s| set_scanout_resource(s, RESOURCE_3D_ID)).and_then(|_| flush(RESOURCE_3D_ID, dw, dh));
    serial_println!("GPU_SPIKE present restored_production_scanout ok={}", restored.is_ok());
    tally.record("present-target-exact", &vp, vp.mismatched == 0);
    tally.record("present-readback-while-scanned-out", &vp2, bound.is_ok() && again.is_ok() && vp2.mismatched == 0);

    Ok(())
}

fn big(tally: &mut Tally) -> Result<(), &'static str> {
    // 2560x1600: upload, read the upload back, render, read back; timed.
    let bg = backing((GW * GH * 4) as usize)?;
    let rt = pipe::BIND_RENDER_TARGET | pipe::BIND_SAMPLER_VIEW;
    create(RES_G, pipe::TEXTURE_2D, vfmt::B8G8R8A8_UNORM, rt, GW, GH, Some(&bg))?;
    surface(S_G, RES_G)?;
    let mut ok = true;
    unsafe {
        for y in 0..GH {
            for x in 0..GW {
                let [r, g, b, a] = upload_px(x, y).map(|c| c as u8);
                core::ptr::copy_nonoverlapping([b, g, r, a].as_ptr(), bg.ptr.add(((y * GW + x) * 4) as usize), 4);
            }
        }
    }
    dma_cache_clean(bg.ptr, bg.len);
    let t = now_us();
    let (uf, ua, _) = transfer(cmd::TRANSFER_TO_HOST_3D, RES_G, GW, GH, GW * 4)?;
    let upload_us = now_us() - t;
    ok &= answered("upload-2560", uf, ua);
    bg.fill(POISON);
    let t = now_us();
    let (rf, ra, _) = transfer(cmd::TRANSFER_FROM_HOST_3D, RES_G, GW, GH, GW * 4)?;
    let readback_us = now_us() - t;
    ok &= answered("readback-2560", rf, ra);
    serial_println!(
        "GPU_SPIKE time=2560x1600 upload_bytes={} upload_us={} readback_us={}",
        bg.len,
        upload_us,
        readback_us
    );
    let vu = judge(&bg, GW, true, (0, 0, GW, GH), &|x, y| Some(upload_px(x, y)));
    tally.record("upload-2560-read-back-exact", &vu, ok && vu.mismatched == 0);
    let g = scene_g(1);
    let vg = render_and_judge("render-2560", &g, RES_G, &bg, GW, GH, true, false, &|x, y| {
        Some(integer_px(x, y.wrapping_sub(16)))
    })?;
    tally.record("render-2560-integer-exact", &vg, vg.mismatched == 0 && vg.poisoned == 0);

    // Host GPU-time attestation window: idle, full-screen integer load, idle.
    serial_println!("GPU_SPIKE attest phase=idle1 begin");
    sleep_ms(8000);
    let load = scene_g(4);
    serial_println!("GPU_SPIKE attest phase=load begin");
    let t = now_us();
    let mut n = 0u64;
    while now_us() - t < 8_000_000 {
        submit(CTX, &load)?;
        n += 1;
    }
    let elapsed = now_us() - t;
    serial_println!(
        "GPU_SPIKE attest phase=load end submissions={} fragments={} us={}",
        n,
        n * 4 * (GW * GH) as u64,
        elapsed
    );
    serial_println!("GPU_SPIKE attest phase=idle2 begin");
    sleep_ms(8000);
    serial_println!("GPU_SPIKE attest phase=idle2 end");
    Ok(())
}

/// R8_UNORM variants: (name, width, row pitch, bind render target, upload: 0 transfer, 1 inline, 2 GPU clear).
const R8_VARIANTS: [(&str, u32, u32, bool, u32); 6] = [
    ("r8-64-pitch64-transfer", 64, 64, false, 0),
    ("r8-64-pitch256-transfer", 64, 256, false, 0),
    ("r8-256-pitch256-transfer", 256, 256, false, 0),
    ("r8-64-rendertarget-gpu-clear", 64, 64, true, 2),
    ("r8-64-pitch64-inline-write", 64, 64, false, 1),
    ("r8-64-rendertarget-transfer", 64, 64, true, 0),
];

/// Diagnostics, reported but not counted in the tally.
fn r8_variants() -> Result<(), &'static str> {
    const RES_PR: u32 = 316;
    const S_PR: u32 = 5;
    const H_FS_PROBE_TXF0: u32 = 60;
    let bpr = backing((TW * TH * 4) as usize)?;
    let rt = pipe::BIND_RENDER_TARGET | pipe::BIND_SAMPLER_VIEW;
    create(RES_PR, pipe::TEXTURE_2D, vfmt::B8G8R8A8_UNORM, rt, TW, TH, Some(&bpr))?;
    surface(S_PR, RES_PR)?;
    let mut cb = Box::new(CommandBuffer::new());
    cb.set_sub_ctx(1);
    cb.create_shader(H_FS_PROBE_TXF0, pipe::SHADER_FRAGMENT, 300, FS_PROBE_TXF0);
    submit(CTX, &cb)?;
    for (i, &(name, w, pitch, as_rt, upload)) in R8_VARIANTS.iter().enumerate() {
        let id = 310 + i as u32;
        let view = 50 + i as u32;
        let surf = 70 + i as u32;
        let b = backing((pitch * TH) as usize)?;
        let bind = if as_rt { rt } else { pipe::BIND_SAMPLER_VIEW };
        create(id, pipe::TEXTURE_2D, vfmt::R8_UNORM, bind, w, TH, Some(&b))?;
        let want = |x: u32, y: u32| if upload == 2 { 128 } else { t8(x, y) };
        unsafe {
            for y in 0..TH {
                for x in 0..w {
                    *b.ptr.add((y * pitch + x) as usize) = want(x, y) as u8;
                }
            }
        }
        dma_cache_clean(b.ptr, b.len);
        let mut cb = Box::new(CommandBuffer::new());
        cb.set_sub_ctx(1);
        cb.create_sampler_view(view, id, vfmt::R8_UNORM, pipe::TEXTURE_2D, 0, 0, 0, 0, swizzle::IDENTITY);
        let up = match upload {
            0 => transfer(cmd::TRANSFER_TO_HOST_3D, id, w, TH, pitch).map(|(f, a, _)| a >= f),
            1 => {
                let mut words = alloc::vec![0u32; (w * TH / 4) as usize];
                for y in 0..TH {
                    for x in 0..w {
                        words[((y * w + x) / 4) as usize] |= t8(x, y) << (8 * (x % 4));
                    }
                }
                cb.resource_inline_write_2d(id, 0, 0, w, TH, w, &words);
                Ok(true)
            }
            _ => {
                cb.create_surface(surf, id, vfmt::R8_UNORM, 0, 0);
                target(&mut cb, surf, w, TH);
                cb.clear_color(128.0 / 255.0, 0.0, 0.0, 1.0);
                Ok(true)
            }
        };
        // Sample the variant by TXF into the probe target.
        let mut v = Verts::new(TW, TH);
        target(&mut cb, S_PR, TW, TH);
        cb.bind_object(H_RAST, OBJ_RASTERIZER);
        clear(&mut cb, CLR_A);
        cb.bind_shader(H_FS_PROBE_TXF0, pipe::SHADER_FRAGMENT);
        cb.set_sampler_views(pipe::SHADER_FRAGMENT, 0, &[view]);
        cb.bind_sampler_states(pipe::SHADER_FRAGMENT, 0, &[H_SAMPLER]);
        v.quad(&mut cb, 0, 0, TW, TH);
        let probe = render_and_judge(name, &cb, RES_PR, &bpr, TW, TH, true, false, &|x, y| Some([want(x, y), 0, 0, 255]))?;
        // Read the R8 resource itself back.
        b.fill(POISON);
        let (rf, ra, _) = transfer(cmd::TRANSFER_FROM_HOST_3D, id, w, TH, pitch)?;
        dma_cache_invalidate(b.ptr, b.len);
        let raw = b.bytes();
        let bad = (0..TH)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .filter(|&(x, y)| raw[(y * pitch + x) as usize] as u32 != want(x, y))
            .count();
        serial_println!(
            "GPU_SPIKE r8-variant={} upload_ok={:?} readback_answered={} readback_mismatched={}/{} first8={:?}",
            name,
            up,
            ra >= rf,
            bad,
            w * TH,
            &raw[..8]
        );
        report(name, &probe, probe.mismatched == 0 && bad == 0);
    }
    Ok(())
}

/// Transfer round trips at several row pitches: (name, format, bytes per pixel, width, pitch).
const PITCHES: [(&str, u32, u32, u32, u32); 7] = [
    ("r8-w64-pitch128", vfmt::R8_UNORM, 1, 64, 128),
    ("r8-w64-pitch320", vfmt::R8_UNORM, 1, 64, 320),
    ("r8-w64-pitch512", vfmt::R8_UNORM, 1, 64, 512),
    ("r8-w256-pitch256", vfmt::R8_UNORM, 1, 256, 256),
    ("bgra-w48-pitch192", vfmt::B8G8R8A8_UNORM, 4, 48, 192),
    ("bgra-w80-pitch320", vfmt::B8G8R8A8_UNORM, 4, 80, 320),
    ("bgra-w64-pitch256", vfmt::B8G8R8A8_UNORM, 4, 64, 256),
];

/// Records, without judging the run, which row pitches a 3D transfer honours in each direction.
fn pitch_probes() {
    for (i, &(name, fmt, bpp, w, pitch)) in PITCHES.iter().enumerate() {
        let id = 330 + i as u32;
        let result = (|| -> Result<(usize, u32), &'static str> {
            let b = backing((pitch * TH) as usize)?;
            create(id, pipe::TEXTURE_2D, fmt, pipe::BIND_SAMPLER_VIEW, w, TH, Some(&b))?;
            let byte = |x: u32, y: u32| ((x * 7 + y * 13 + 1) & 255) as u8;
            unsafe {
                for y in 0..TH {
                    for x in 0..w * bpp {
                        *b.ptr.add((y * pitch + x) as usize) = byte(x, y);
                    }
                }
            }
            dma_cache_clean(b.ptr, b.len);
            transfer(cmd::TRANSFER_TO_HOST_3D, id, w, TH, pitch)?;
            b.fill(POISON);
            transfer(cmd::TRANSFER_FROM_HOST_3D, id, w, TH, pitch)?;
            dma_cache_invalidate(b.ptr, b.len);
            let raw = b.bytes();
            let bad = (0..TH)
                .flat_map(|y| (0..w * bpp).map(move |x| (x, y)))
                .filter(|&(x, y)| raw[(y * pitch + x) as usize] != byte(x, y))
                .count();
            Ok((bad, w * bpp * TH))
        })();
        match result {
            Ok((bad, total)) => serial_println!(
                "GPU_SPIKE info=pitch-probe {} round_trip_mismatched_bytes={}/{} {}",
                name,
                bad,
                total,
                if bad == 0 { "HONOURED" } else { "DROPPED" }
            ),
            Err(e) => serial_println!("GPU_SPIKE info=pitch-probe {} error={}", name, e),
        }
    }
}
