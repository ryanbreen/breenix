//! G0 host-stack spike (throwaway, never for main): drives QEMU's virtio-gpu-gl-pci with
//! polled VirGL commands, renders the profile workload into a B8G8R8A8_UNORM target and
//! checks every pixel of the target read back from the device with TRANSFER_FROM_HOST_3D.
//!
//! Regions of the 256x64 target (rows are the readback's rows):
//!   texfetch  x 0..64,  y 0..16   integer texel fetch (TXF) from an R8_UNORM and a B8G8R8A8_UNORM texture
//!   integer   x 0..256, y 16..32  32-bit integer routines and a two-word multiply from 16x16 partial products
//!   blend     x 0..64,  y 32..48  over(src, dst) in integers, dst read from a GPU copy of the target
//!   unorm     x 0..256, y 48..64  every 8-bit value k in every channel, written as k * (1/255)

use super::pci_transport::VirtioPciDevice;
use super::virgl::{format as vfmt, pipe, swizzle, CommandBuffer};
use crate::serial_println;

const W: u32 = 256;
const H: u32 = 64;
const TW: u32 = 64;
const TH: u32 = 16;

const CTX: u32 = 1;
const RES_RT: u32 = 10;
const RES_T8: u32 = 11;
const RES_TB: u32 = 12;
const RES_COPY: u32 = 13;
const RES_VB: u32 = 14;
const VB_BYTES: u32 = 4096;

const VIRTIO_F_VERSION_1: u64 = 1 << 32;
const VIRTIO_GPU_F_VIRGL: u64 = 1 << 0;

const CMD_GET_CAPSET_INFO: u32 = 0x0108;
const CMD_GET_CAPSET: u32 = 0x0109;
const CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;
const CMD_CTX_CREATE: u32 = 0x0200;
const CMD_CTX_ATTACH_RESOURCE: u32 = 0x0202;
const CMD_RESOURCE_CREATE_3D: u32 = 0x0204;
const CMD_TRANSFER_TO_HOST_3D: u32 = 0x0205;
const CMD_TRANSFER_FROM_HOST_3D: u32 = 0x0206;
const CMD_SUBMIT_3D: u32 = 0x0207;
const RESP_OK_NODATA: u32 = 0x1100;
const RESP_OK_CAPSET_INFO: u32 = 0x1102;
const RESP_OK_CAPSET: u32 = 0x1103;
const FLAG_FENCE: u32 = 1;

const QSIZE: u16 = 16;
const CAPS_RENDERER_OFFSET: usize = 696;

#[repr(C, align(4096))]
struct Page<const N: usize>([u8; N]);

// Split virtqueue: descriptors at 0, available ring at 256, used ring at 4096.
const AVAIL: usize = 256;
const USED: usize = 4096;
static mut QUEUE: Page<8192> = Page([0; 8192]);
static mut CMD: Page<32768> = Page([0; 32768]);
static mut RESP: Page<4096> = Page([0; 4096]);
static mut RT_BACKING: Page<{ (W * H * 4) as usize }> = Page([0; (W * H * 4) as usize]);
static mut T8_BACKING: Page<{ (TW * TH) as usize }> = Page([0; (TW * TH) as usize]);
static mut TB_BACKING: Page<{ (TW * TH * 4) as usize }> = Page([0; (TW * TH * 4) as usize]);
static mut COPY_BACKING: Page<{ (TW * TH * 4) as usize }> = Page([0; (TW * TH * 4) as usize]);
static mut VB_BACKING: Page<{ VB_BYTES as usize }> = Page([0; VB_BYTES as usize]);
static mut VIRGL: CommandBuffer = CommandBuffer::new();

fn phys(addr: u64) -> u64 {
    const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;
    if addr >= HHDM_BASE {
        addr - HHDM_BASE
    } else {
        addr
    }
}

fn cache_clean(ptr: *const u8, len: usize) {
    let start = ptr as usize & !63;
    let end = (ptr as usize + len + 63) & !63;
    for a in (start..end).step_by(64) {
        unsafe { core::arch::asm!("dc cvac, {}", in(reg) a, options(nostack)) };
    }
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}

fn cache_invalidate(ptr: *const u8, len: usize) {
    let start = ptr as usize & !63;
    let end = (ptr as usize + len + 63) & !63;
    for a in (start..end).step_by(64) {
        unsafe { core::arch::asm!("dc civac, {}", in(reg) a, options(nostack)) };
    }
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}

fn counter() -> (u64, u64) {
    let (cnt, frq): (u64, u64);
    unsafe {
        core::arch::asm!("isb; mrs {}, cntvct_el0", out(reg) cnt, options(nostack));
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) frq, options(nostack));
    }
    (cnt, frq)
}

struct Gpu {
    dev: VirtioPciDevice,
    avail_idx: u16,
    last_used: u16,
    next_fence: u64,
}

fn queue_put<T: Copy>(offset: usize, value: T) {
    unsafe { core::ptr::write_volatile(((&raw mut QUEUE.0) as *mut u8).add(offset) as *mut T, value) };
}

fn queue_get<T: Copy>(offset: usize) -> T {
    unsafe { core::ptr::read_volatile(((&raw const QUEUE.0) as *const u8).add(offset) as *const T) }
}

fn queue_desc(i: usize, addr: u64, len: u32, flags: u16, next: u16) {
    queue_put(i * 16, addr);
    queue_put(i * 16 + 8, len);
    queue_put(i * 16 + 12, flags);
    queue_put(i * 16 + 14, next);
}

fn put<T: Copy>(offset: usize, value: T) {
    unsafe {
        let base = (&raw mut CMD.0) as *mut u8;
        core::ptr::write_unaligned(base.add(offset) as *mut T, value);
    }
}

fn hdr(cmd: u32, flags: u32, fence: u64, ctx: u32) {
    put(0, cmd);
    put(4, flags);
    put(8, fence);
    put(16, ctx);
    put(20, 0u32);
}

fn resp_u32(offset: usize) -> u32 {
    unsafe { core::ptr::read_unaligned(((&raw const RESP.0) as *const u8).add(offset) as *const u32) }
}

impl Gpu {
    /// Sends CMD[..len] and waits for the device to write the response into RESP.
    fn send(&mut self, len: usize, resp_len: usize) -> Result<u32, &'static str> {
        unsafe {
            let q = (&raw const QUEUE.0) as *const u8;
            let resp = (&raw mut RESP.0) as *mut u8;
            core::ptr::write_bytes(resp, 0xEE, resp_len);
            cache_clean(resp, resp_len);
            cache_clean((&raw const CMD.0) as *const u8, len);
            queue_desc(0, phys((&raw const CMD.0) as u64), len as u32, 1, 1);
            queue_desc(1, phys(resp as u64), resp_len as u32, 2, 0);
            queue_put(AVAIL + 4 + 2 * (self.avail_idx % QSIZE) as usize, 0u16);
            core::arch::asm!("dmb ish", options(nostack, preserves_flags));
            self.avail_idx = self.avail_idx.wrapping_add(1);
            queue_put(AVAIL + 2, self.avail_idx);
            cache_clean(q, 4096);
            self.dev.notify_queue(0);
            let (start, frq) = counter();
            loop {
                cache_invalidate(q.add(USED), 8);
                let used: u16 = queue_get(USED + 2);
                if used != self.last_used {
                    self.last_used = used;
                    break;
                }
                if counter().0 - start > frq * 10 {
                    return Err("device did not answer within 10 s");
                }
                core::hint::spin_loop();
            }
            cache_invalidate(resp, resp_len);
        }
        Ok(resp_u32(0))
    }

    fn expect_ok(&mut self, len: usize, what: &'static str) -> Result<(), &'static str> {
        let resp = self.send(len, 24)?;
        if resp != RESP_OK_NODATA {
            serial_println!("GPU_SPIKE {} answered {:#x}", what, resp);
            return Err(what);
        }
        Ok(())
    }

    fn create_3d(&mut self, id: u32, target: u32, fmt: u32, bind: u32, w: u32, h: u32) -> Result<(), &'static str> {
        hdr(CMD_RESOURCE_CREATE_3D, 0, 0, CTX);
        for (i, v) in [id, target, fmt, bind, w, h, 1, 1, 0, 0, 0, 0].iter().enumerate() {
            put(24 + i * 4, *v);
        }
        self.expect_ok(72, "RESOURCE_CREATE_3D")?;
        hdr(CMD_CTX_ATTACH_RESOURCE, 0, 0, CTX);
        put(24, id);
        put(28, 0u32);
        self.expect_ok(32, "CTX_ATTACH_RESOURCE")
    }

    fn attach(&mut self, id: u32, addr: u64, len: usize) -> Result<(), &'static str> {
        hdr(CMD_RESOURCE_ATTACH_BACKING, 0, 0, 0);
        put(24, id);
        put(28, 1u32);
        put(32, phys(addr));
        put(40, len as u32);
        put(44, 0u32);
        self.expect_ok(48, "RESOURCE_ATTACH_BACKING")
    }

    fn transfer(&mut self, cmd: u32, id: u32, w: u32, h: u32, stride: u32) -> Result<(), &'static str> {
        let fence = self.next_fence;
        self.next_fence += 1;
        hdr(cmd, FLAG_FENCE, fence, CTX);
        for (i, v) in [0u32, 0, 0, w, h, 1].iter().enumerate() {
            put(24 + i * 4, *v);
        }
        put(48, 0u64);
        put(56, id);
        put(60, 0u32);
        put(64, stride);
        put(68, 0u32);
        self.expect_ok(72, "TRANSFER_3D")
    }

    fn submit(&mut self, cmds: &[u32]) -> Result<(), &'static str> {
        let fence = self.next_fence;
        self.next_fence += 1;
        hdr(CMD_SUBMIT_3D, FLAG_FENCE, fence, CTX);
        put(24, (cmds.len() * 4) as u32);
        put(28, 0u32);
        for (i, v) in cmds.iter().enumerate() {
            put(32 + i * 4, *v);
        }
        self.expect_ok(32 + cmds.len() * 4, "SUBMIT_3D")
    }
}

// ----------------------------------------------------------------------------- formulas

fn t8(x: u32, y: u32) -> u32 {
    (x * 4 + y * 13) & 255
}

/// The B8G8R8A8 texture's channels as (r, g, b, a).
fn tb(x: u32, y: u32) -> [u32; 4] {
    [(x * 3) ^ (y * 5) & 255, (x + y * 29) & 255, (x * 7 + y) & 255, (255 - x - y * 3) & 255]
}

fn mul64(a: u32, b: u32) -> (u32, u32) {
    let (al, ah, bl, bh) = (a & 0xFFFF, a >> 16, b & 0xFFFF, b >> 16);
    let (ll, lh, hl, hh) = (al * bl, al * bh, ah * bl, ah * bh);
    let lo1 = ll.wrapping_add(lh << 16);
    let c1 = (lo1 < ll) as u32;
    let lo2 = lo1.wrapping_add(hl << 16);
    let c2 = (lo2 < lo1) as u32;
    let hi = hh.wrapping_add(lh >> 16).wrapping_add(hl >> 16).wrapping_add(c1).wrapping_add(c2);
    (lo2, hi)
}

fn integer_px(x: u32, y: u32) -> [u32; 4] {
    let a = x.wrapping_mul(2654435761).wrapping_add(y.wrapping_mul(2246822519)).wrapping_add(374761393);
    let b = (x ^ (y << 8)).wrapping_mul(3266489917).wrapping_add(668265263);
    let (lo, hi) = mul64(a, b);
    [lo & 255, (lo >> 16) & 255, (hi >> 8) & 255, hi >> 24]
}

fn background_px(x: u32, y: u32) -> [u32; 4] {
    [(x * 5 + y * 11) & 255, (x * 9) & 255, (y * 17) & 255, 255]
}

fn blend_px(x: u32, y: u32) -> [u32; 4] {
    let s = [(x * 13) & 255, (y * 31 + x) & 255, x ^ (y * 7) & 255, (x * 4 + y * 9) & 255];
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
    [x, 255 - x, (x * 7 + 3) & 255, (x * 37 + 11) & 255]
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

// ----------------------------------------------------------------------------- the run

/// Appends a pixel-aligned rectangle as a 4-vertex strip at vertex `first` of the vertex buffer.
fn quad(cb: &mut CommandBuffer, first: u32, x0: u32, y0: u32, x1: u32, y1: u32) {
    let cx = |x: u32| (x as f32 / (W as f32 / 2.0) - 1.0).to_bits();
    let cy = |y: u32| (1.0 - y as f32 / (H as f32 / 2.0)).to_bits();
    let one = 1.0f32.to_bits();
    let verts = [
        cx(x0), cy(y0), 0, one, cx(x0), cy(y1), 0, one, cx(x1), cy(y0), 0, one, cx(x1), cy(y1), 0, one,
    ];
    cb.resource_inline_write(RES_VB, first * 16, 64, &verts);
    cb.draw_vbo(first, 4, pipe::PRIM_TRIANGLE_STRIP, first + 3);
}

/// Resource copy region (VIRGL_CCMD_RESOURCE_COPY_REGION, 13 dwords).
fn copy_region(cb: &mut CommandBuffer, dst: u32, src: u32, sx: u32, sy: u32, w: u32, h: u32) {
    let words = [dst, 0, 0, 0, 0, src, 0, sx, sy, 0, w, h, 1];
    cb.raw(17 | (13 << 16), &words);
}

pub fn run() {
    match run_inner() {
        Ok(()) => {}
        Err(e) => serial_println!("GPU_SPIKE DONE passed=0 failed=1 error={}", e),
    }
}

fn run_inner() -> Result<(), &'static str> {
    let pci = crate::drivers::pci::find_device(0x1AF4, 0x1050).ok_or("no virtio-gpu PCI device")?;
    let mut dev = VirtioPciDevice::probe(pci).ok_or("no modern virtio capabilities")?;
    let offered = dev.read_device_features();
    dev.init(VIRTIO_F_VERSION_1 | VIRTIO_GPU_F_VIRGL)?;
    let virgl = dev.device_features() & VIRTIO_GPU_F_VIRGL != 0;
    serial_println!("GPU_SPIKE device features={:#x} virgl={}", offered, virgl);
    if !virgl {
        return Err("VIRGL not negotiated");
    }
    dev.select_queue(0);
    if dev.get_queue_num_max() < QSIZE as u32 {
        return Err("control queue too small");
    }
    dev.set_queue_num(QSIZE as u32);
    let queue = unsafe { (&raw const QUEUE.0) as *const u8 };
    let q = phys(queue as u64);
    cache_clean(queue, 8192);
    dev.set_queue_desc(q);
    dev.set_queue_avail(q + AVAIL as u64);
    dev.set_queue_used(q + USED as u64);
    dev.set_queue_ready(true);
    dev.driver_ok();
    let num_capsets = dev.read_config_u32(12);
    let mut gpu = Gpu { dev, avail_idx: 0, last_used: 0, next_fence: 1 };

    // The VirGL 2 capset: caps v2 carries the host's renderer string.
    let mut found = None;
    for index in 0..num_capsets {
        hdr(CMD_GET_CAPSET_INFO, 0, 0, 0);
        put(24, index);
        put(28, 0u32);
        if gpu.send(32, 40)? != RESP_OK_CAPSET_INFO {
            return Err("GET_CAPSET_INFO failed");
        }
        let (id, ver, size) = (resp_u32(24), resp_u32(28), resp_u32(32));
        serial_println!("GPU_SPIKE capset index={} id={} version={} size={}", index, id, ver, size);
        if id == 2 {
            found = Some((id, ver, size));
        }
    }
    let (cap_id, cap_ver, cap_size) = found.ok_or("no VirGL 2 capset")?;
    hdr(CMD_GET_CAPSET, 0, 0, 0);
    put(24, cap_id);
    put(28, cap_ver);
    let resp_len = core::cmp::min(24 + cap_size as usize, 4096);
    if gpu.send(32, resp_len)? != RESP_OK_CAPSET {
        return Err("GET_CAPSET failed");
    }
    if cap_size as usize >= CAPS_RENDERER_OFFSET + 64 {
        let mut name = [0u8; 64];
        unsafe {
            let src = ((&raw const RESP.0) as *const u8).add(24 + CAPS_RENDERER_OFFSET);
            core::ptr::copy_nonoverlapping(src, name.as_mut_ptr(), 64);
        }
        let len = name.iter().position(|&b| b == 0).unwrap_or(64);
        let s = core::str::from_utf8(&name[..len]).unwrap_or("?");
        serial_println!("VIRGL_CAPSET_RENDERER={}", s);
    }

    hdr(CMD_CTX_CREATE, 0, 0, CTX);
    put(24, 9u32);
    put(28, 0u32);
    for (i, b) in b"gpu-spike".iter().enumerate() {
        put(32 + i, *b);
    }
    gpu.expect_ok(96, "CTX_CREATE")?;

    let rt_bind = pipe::BIND_RENDER_TARGET | pipe::BIND_SAMPLER_VIEW;
    gpu.create_3d(RES_RT, pipe::TEXTURE_2D, vfmt::B8G8R8A8_UNORM, rt_bind, W, H)?;
    gpu.create_3d(RES_T8, pipe::TEXTURE_2D, vfmt::R8_UNORM, pipe::BIND_SAMPLER_VIEW, TW, TH)?;
    gpu.create_3d(RES_TB, pipe::TEXTURE_2D, vfmt::B8G8R8A8_UNORM, pipe::BIND_SAMPLER_VIEW, TW, TH)?;
    gpu.create_3d(RES_COPY, pipe::TEXTURE_2D, vfmt::B8G8R8A8_UNORM, rt_bind, TW, TH)?;
    gpu.create_3d(RES_VB, pipe::BUFFER, vfmt::R8_UNORM, pipe::BIND_VERTEX_BUFFER, VB_BYTES, 1)?;
    unsafe {
        gpu.attach(RES_RT, (&raw const RT_BACKING) as u64, (W * H * 4) as usize)?;
        gpu.attach(RES_T8, (&raw const T8_BACKING) as u64, (TW * TH) as usize)?;
        gpu.attach(RES_TB, (&raw const TB_BACKING) as u64, (TW * TH * 4) as usize)?;
        gpu.attach(RES_COPY, (&raw const COPY_BACKING) as u64, (TW * TH * 4) as usize)?;
        gpu.attach(RES_VB, (&raw const VB_BACKING) as u64, VB_BYTES as usize)?;

        // Texture uploads.
        let t8p = (&raw mut T8_BACKING.0) as *mut u8;
        let tbp = (&raw mut TB_BACKING.0) as *mut u8;
        for y in 0..TH {
            for x in 0..TW {
                *t8p.add((y * TW + x) as usize) = t8(x, y) as u8;
                let [r, g, b, a] = tb(x, y);
                let o = ((y * TW + x) * 4) as usize;
                *tbp.add(o) = b as u8;
                *tbp.add(o + 1) = g as u8;
                *tbp.add(o + 2) = r as u8;
                *tbp.add(o + 3) = a as u8;
            }
        }
        cache_clean(t8p, (TW * TH) as usize);
        cache_clean(tbp, (TW * TH * 4) as usize);
    }
    gpu.transfer(CMD_TRANSFER_TO_HOST_3D, RES_T8, TW, TH, TW)?;
    gpu.transfer(CMD_TRANSFER_TO_HOST_3D, RES_TB, TW, TH, TW * 4)?;

    let cb = unsafe { &mut *(&raw mut VIRGL) };
    cb.clear();
    cb.create_sub_ctx(1);
    cb.set_sub_ctx(1);
    cb.create_surface(1, RES_RT, vfmt::B8G8R8A8_UNORM, 0, 0);
    // Blending off, all channels written, no dithering (profile item 1).
    cb.raw(1 | (1 << 8) | (11 << 16), &[1, 0, 0, 0xF << 27, 0, 0, 0, 0, 0, 0, 0]);
    cb.create_dsa_default(1);
    cb.create_rasterizer(1, 1 << 29);
    cb.create_vertex_elements(1, &[(0, 0, 0, vfmt::R32G32B32A32_FLOAT)]);
    cb.create_shader(2, pipe::SHADER_VERTEX, 64, VS);
    cb.create_shader(3, pipe::SHADER_FRAGMENT, 1024, FS_TEXFETCH);
    cb.create_shader(4, pipe::SHADER_FRAGMENT, 1024, FS_INTEGER);
    cb.create_shader(5, pipe::SHADER_FRAGMENT, 1024, FS_BACKGROUND);
    cb.create_shader(6, pipe::SHADER_FRAGMENT, 1024, FS_BLEND);
    cb.create_shader(7, pipe::SHADER_FRAGMENT, 1024, FS_UNORM);
    cb.create_sampler_view(20, RES_T8, vfmt::R8_UNORM, pipe::TEXTURE_2D, 0, 0, 0, 0, swizzle::IDENTITY);
    cb.create_sampler_view(21, RES_TB, vfmt::B8G8R8A8_UNORM, pipe::TEXTURE_2D, 0, 0, 0, 0, swizzle::IDENTITY);
    cb.create_sampler_view(22, RES_COPY, vfmt::B8G8R8A8_UNORM, pipe::TEXTURE_2D, 0, 0, 0, 0, swizzle::IDENTITY);
    cb.create_sampler_state(
        30,
        pipe::TEX_WRAP_CLAMP_TO_EDGE,
        pipe::TEX_WRAP_CLAMP_TO_EDGE,
        pipe::TEX_WRAP_CLAMP_TO_EDGE,
        pipe::TEX_FILTER_NEAREST,
        pipe::TEX_MIPFILTER_NONE,
        pipe::TEX_FILTER_NEAREST,
    );
    cb.bind_object(1, super::virgl::OBJ_BLEND);
    cb.bind_object(1, super::virgl::OBJ_DSA);
    cb.bind_object(1, super::virgl::OBJ_RASTERIZER);
    cb.bind_object(1, super::virgl::OBJ_VERTEX_ELEMENTS);
    cb.bind_shader(2, pipe::SHADER_VERTEX);
    cb.set_framebuffer_state(0, &[1]);
    cb.set_viewport(W as f32, H as f32);
    cb.set_vertex_buffers(&[(16, 0, RES_VB)]);
    cb.clear_color(0.0, 0.0, 0.0, 0.0);

    cb.bind_shader(3, pipe::SHADER_FRAGMENT);
    cb.set_sampler_views(pipe::SHADER_FRAGMENT, 0, &[20, 21]);
    cb.bind_sampler_states(pipe::SHADER_FRAGMENT, 0, &[30, 30]);
    quad(cb, 0, 0, 0, TW, TH);

    cb.bind_shader(4, pipe::SHADER_FRAGMENT);
    quad(cb, 4, 0, 16, W, 32);

    cb.bind_shader(5, pipe::SHADER_FRAGMENT);
    quad(cb, 8, 0, 32, TW, 48);
    copy_region(cb, RES_COPY, RES_RT, 0, 32, TW, TH);

    cb.bind_shader(6, pipe::SHADER_FRAGMENT);
    cb.set_sampler_views(pipe::SHADER_FRAGMENT, 0, &[22]);
    cb.bind_sampler_states(pipe::SHADER_FRAGMENT, 0, &[30]);
    quad(cb, 12, 0, 32, TW, 48);

    cb.bind_shader(7, pipe::SHADER_FRAGMENT);
    quad(cb, 16, 0, 48, W, 64);
    serial_println!("GPU_SPIKE submitting {} dwords", cb.len());
    if cb.len() >= 3000 {
        return Err("VirGL command buffer full");
    }
    gpu.submit(cb.as_slice())?;

    // Poison the backing, read the target back, and judge every pixel.
    unsafe {
        let p = (&raw mut RT_BACKING.0) as *mut u8;
        core::ptr::write_bytes(p, 0xA5, (W * H * 4) as usize);
        cache_clean(p, (W * H * 4) as usize);
    }
    gpu.transfer(CMD_TRANSFER_FROM_HOST_3D, RES_RT, W, H, W * 4)?;
    let px = unsafe {
        let p = (&raw const RT_BACKING.0) as *const u8;
        cache_invalidate(p, (W * H * 4) as usize);
        core::slice::from_raw_parts(p, (W * H * 4) as usize)
    };
    let got = |x: u32, y: u32| -> [u32; 4] {
        let o = ((y * W + x) * 4) as usize;
        [px[o + 2] as u32, px[o + 1] as u32, px[o] as u32, px[o + 3] as u32]
    };
    let regions: [(&str, u32, u32, u32, u32); 4] =
        [("texfetch", 0, 0, TW, 16), ("integer", 0, 16, W, 32), ("blend", 0, 32, TW, 48), ("unorm", 0, 48, W, 64)];
    let mut passed = 0;
    let mut failed = 0;
    for (name, x0, y0, x1, y1) in regions {
        let mut bad = 0u32;
        let mut first: Option<(u32, u32, [u32; 4], [u32; 4])> = None;
        for y in y0..y1 {
            for x in x0..x1 {
                let want = match name {
                    "texfetch" => {
                        let c = tb(x, y);
                        [c[0], c[1], t8(x, y), c[3]]
                    }
                    "integer" => integer_px(x, y - 16),
                    "blend" => blend_px(x, y - 32),
                    _ => unorm_px(x),
                };
                let have = got(x, y);
                if have != want {
                    bad += 1;
                    if first.is_none() {
                        first = Some((x, y, have, want));
                    }
                }
            }
        }
        let total = (x1 - x0) * (y1 - y0);
        match first {
            None => {
                passed += 1;
                serial_println!("GPU_SPIKE {} PASS pixels={}", name, total);
            }
            Some((x, y, have, want)) => {
                failed += 1;
                serial_println!(
                    "GPU_SPIKE {} FAIL mismatches={}/{} first=({},{}) got_rgba={:?} want_rgba={:?}",
                    name, bad, total, x, y, have, want
                );
            }
        }
    }
    // Every 8-bit value appears in every channel of the unorm region (each channel is a permutation of x).
    serial_println!("GPU_SPIKE DONE passed={} failed={}", passed, failed);
    Ok(())
}
