//! Throwaway G0 DX feasibility probe; never merge this scratch branch.
//!
//! Interface facts: VMware's MIT-licensed device_include/{svga_reg.h,
//! svga3d_cmd.h,svga3d_dx.h,svga3d_types.h,svga3d_devcaps.h}; only those
//! published interface definitions were consulted, not driver implementations.
//! Shader tokens: Microsoft's DirectX-Headers D3D12TokenizedProgramFormat.hpp,
//! SM4 instruction/operand declarations. Programs below are handwritten.
//! CPU expectations below use independent integer formulas, never rendered bytes.

use super::{
    cache_clean_range, cache_invalidate_range, reg_read, reg_write, CmdEncoder, SvgaCbHeader,
};
use crate::serial_println;
use alloc::{
    alloc::{alloc_zeroed, dealloc, Layout},
    vec,
    vec::Vec,
};

const INVALID: u32 = u32::MAX;
const CID: u32 = 0;
const W: u32 = 256;
const H: u32 = 256;
const BGRA: u32 = 141;
const RGBA: u32 = 68;
const R8: u32 = 93;
const RT: u32 = 1 << 24;
const SR: u32 = 1 << 23;

type Result<T> = core::result::Result<T, &'static str>;

// All allocations stay alive until the VM stops: even a failed command may
// retain a device reference. No timeout ever frees memory still owned by DMA.
struct Dma {
    ptr: *mut u8,
    pa: u64,
    len: usize,
}
impl Dma {
    fn new(bytes: usize) -> Result<Self> {
        let len = (bytes + 4095) & !4095;
        let layout = Layout::from_size_align(len, 4096).map_err(|_| "DMA layout")?;
        let ptr = unsafe { alloc_zeroed(layout) };
        if ptr.is_null() {
            return Err("DMA allocation");
        }
        let pa = crate::platform_config::kernel_va_to_ipa(ptr as u64);
        let mem = Self { ptr, pa, len };
        mem.clean();
        Ok(mem)
    }
    fn clean(&self) {
        unsafe {
            cache_clean_range(self.ptr as u64, self.len);
            core::arch::asm!("dsb sy");
        }
    }
    fn invalidate(&self) {
        unsafe {
            cache_invalidate_range(self.ptr as u64, self.len);
            core::arch::asm!("dsb sy");
        }
    }
    fn write(&self, bytes: &[u8]) {
        assert!(bytes.len() <= self.len);
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), self.ptr, bytes.len());
        }
        self.clean();
    }
    fn poison(&self, value: u8) {
        unsafe {
            core::ptr::write_bytes(self.ptr, value, self.len);
        }
        self.clean();
    }
}

fn words(bytes: &[u32]) -> Vec<u8> {
    bytes.iter().flat_map(|x| x.to_le_bytes()).collect()
}
fn cmd(e: &mut CmdEncoder, id: u32, args: &[u32]) {
    e.cmd_header(id, (args.len() * 4) as u32);
    for x in args {
        e.push_u32(*x);
    }
}

fn submit(e: CmdEncoder, dx: bool, device: bool) -> Result<()> {
    let data = e.finish();
    let mem = Dma::new(64 + data.len())?;
    unsafe {
        let hdr = &mut *(mem.ptr as *mut SvgaCbHeader);
        hdr.flags = 1 | if dx { 2 } else { 0 };
        hdr.length = data.len() as u32;
        hdr.pa = mem.pa + 64;
        hdr.dx_context = CID;
        core::ptr::copy_nonoverlapping(data.as_ptr(), mem.ptr.add(64), data.len());
    }
    mem.clean();
    unsafe {
        reg_write(49, (mem.pa >> 32) as u32);
        reg_write(48, mem.pa as u32 | if device { 63 } else { 0 });
    }
    let start = ticks();
    loop {
        mem.invalidate();
        let status = unsafe { core::ptr::read_volatile(mem.ptr as *const u32) };
        if status != 0 {
            let off = unsafe { core::ptr::read_volatile(mem.ptr.add(4) as *const u32) };
            if status != 1 {
                let command = data
                    .get(off as usize..off as usize + 4)
                    .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                    .unwrap_or(0);
                serial_println!("GPU_SPIKE rejected_command={}", command);
                serial_println!(
                    "GPU_SPIKE CB status={} offset={} dx={} device={} bytes={}",
                    status,
                    off,
                    dx,
                    device,
                    data.len()
                );
            }
            unsafe {
                dealloc(mem.ptr, Layout::from_size_align(mem.len, 4096).unwrap());
            }
            return if status == 1 {
                Ok(())
            } else {
                Err("command rejected")
            };
        }
        if ticks().wrapping_sub(start) > frequency() * 3 {
            return Err("command timeout (DMA retained)");
        }
        core::hint::spin_loop();
    }
}
fn ticks() -> u64 {
    let x;
    unsafe {
        core::arch::asm!("mrs {}, cntvct_el0",out(reg)x);
    }
    x
}
fn frequency() -> u64 {
    let x;
    unsafe {
        core::arch::asm!("mrs {}, cntfrq_el0",out(reg)x);
    }
    x
}
fn devcap(n: u32) -> u32 {
    unsafe {
        reg_write(52, n);
        reg_read(52)
    }
}

struct Probe {
    next: u32,
    passed: u32,
    failed: u32,
    pixels: usize,
    fence: Option<(u32, Dma)>,
    sequence: u64,
    rtv: bool,
    srv: bool,
}
struct Surface {
    sid: u32,
    format: u32,
    dma: Dma,
    width: u32,
    height: u32,
}
impl Probe {
    fn mob(&mut self, size: usize) -> Result<(u32, Dma)> {
        let id = self.next;
        self.next += 1;
        let mem = Dma::new(size)?;
        let mut e = CmdEncoder::new();
        // Two indirections of 64-bit page numbers, each page holds 512 entries.
        let leaves = Dma::new(mem.len.div_ceil(4096) * 8)?;
        let root = Dma::new(leaves.len.div_ceil(4096) * 8)?;
        for i in 0..mem.len / 4096 {
            let pa = crate::platform_config::kernel_va_to_ipa(mem.ptr as u64 + (i * 4096) as u64);
            unsafe {
                core::ptr::write_volatile((leaves.ptr as *mut u64).add(i), pa >> 12);
            }
        }
        for i in 0..leaves.len / 4096 {
            let pa =
                crate::platform_config::kernel_va_to_ipa(leaves.ptr as u64 + (i * 4096) as u64);
            unsafe {
                core::ptr::write_volatile((root.ptr as *mut u64).add(i), pa >> 12);
            }
        }
        leaves.clean();
        root.clean();
        e.define_gb_mob64(id, 6, root.pa >> 12, mem.len as u32);
        submit(e, false, false)?;
        Ok((id, mem))
    }
    fn table(&mut self, ty: u32, size: usize) -> Result<()> {
        let mem = Dma::new(size)?;
        let mut e = CmdEncoder::new();
        e.set_otable_base64(ty, mem.pa >> 12, mem.len as u32, 0, 4);
        submit(e, false, false)
    }
    fn surface(&mut self, format: u32, flags: u32, width: u32, height: u32) -> Result<Surface> {
        let bytes = width as usize * height as usize * if format == R8 { 1 } else { 4 };
        let (id, dma) = self.mob(bytes)?;
        let mut e = CmdEncoder::new();
        // v1 expresses exactly the profile's one mip, one sample, no array.
        cmd(
            &mut e,
            1097,
            &[id, flags, format, 1, 0, 0, width, height, 1],
        );
        e.bind_gb_surface(id, id);
        submit(e, false, false)?;
        Ok(Surface {
            sid: id,
            format,
            dma,
            width,
            height,
        })
    }
    fn upload(&mut self, s: &Surface, bytes: &[u8]) -> Result<()> {
        s.dma.write(bytes);
        let mut e = CmdEncoder::new();
        e.update_gb_image(s.sid, 0, 0, s.width, s.height);
        submit(e, false, false)
    }
    fn shader(&mut self, id: u32, kind: u32, mut s: Shader) -> Result<()> {
        s.emit(62, &[]);
        s.code[1] = s.code.len() as u32;
        let bytecode_size = s.code.len() * 4;
        // VMware signature trailer follows the SM4 program (not a DXBC container).
        s.code.extend_from_slice(&[0x08a92d12, 1, 1, 0]);
        s.code
            .extend_from_slice(&[0, if kind == 1 { 0 } else { 1 }, 15, 0, 0]);
        s.code
            .extend_from_slice(&[0, if kind == 1 { 1 } else { 0 }, 15, 0, 0]);
        let data = words(&s.code);
        let (mob, mem) = self.mob(data.len())?;
        mem.write(&data);
        let mut e = CmdEncoder::new();
        cmd(&mut e, 1201, &[id, kind, bytecode_size as u32]);
        cmd(&mut e, 1203, &[CID, id, mob, 0]);
        submit(e, true, false)
    }
    fn target(&mut self, s: &Surface) -> Result<()> {
        let mut e = CmdEncoder::new();
        cmd(&mut e, 1149, &[0, 2, INVALID]);
        if self.rtv {
            cmd(&mut e, 1161, &[INVALID, INVALID]);
            cmd(&mut e, 1188, &[0]);
        }
        self.rtv = true;
        cmd(&mut e, 1187, &[0, s.sid, s.format, 3, 0, 0, 1]);
        cmd(&mut e, 1161, &[INVALID, 0]);
        submit(e, true, false)
    }
    fn texture(&mut self, s: &Surface) -> Result<()> {
        let mut e = CmdEncoder::new();
        if self.srv {
            cmd(&mut e, 1149, &[0, 2, INVALID]);
            cmd(&mut e, 1186, &[0]);
        }
        self.srv = true;
        cmd(&mut e, 1185, &[0, s.sid, s.format, 3, 0, 0, 1, 1]);
        cmd(&mut e, 1149, &[0, 2, 0]);
        submit(e, true, false)
    }
    fn clear(&self) -> Result<()> {
        let mut e = CmdEncoder::new();
        cmd(&mut e, 1176, &[0, 0, 0, 0, 0]);
        submit(e, true, false)
    }
    fn draw(&self, shader: u32) -> Result<()> {
        let mut e = CmdEncoder::new();
        cmd(&mut e, 1150, &[shader, 2]);
        cmd(&mut e, 1152, &[6, 0]);
        submit(e, true, false)
    }
    fn scissor(&self, enable: bool) -> Result<()> {
        let mut e = CmdEncoder::new();
        cmd(&mut e, 1164, &[u32::from(enable)]);
        if enable {
            cmd(&mut e, 1175, &[0, 23, 41, 191, 217]);
        }
        submit(e, true, false)
    }
    fn copy(&self, src: &Surface, dst: &Surface) -> Result<()> {
        let mut e = CmdEncoder::new();
        cmd(
            &mut e,
            1178,
            &[dst.sid, 0, src.sid, 0, 0, 0, 0, W, H, 1, 0, 0, 0],
        );
        submit(e, true, false)
    }
    fn read(&mut self, src: &Surface, read: &Surface, poison: u8) -> Result<Vec<u8>> {
        // GPU host copy first: never interpret the render target's guest backing.
        let mut e = CmdEncoder::new();
        cmd(
            &mut e,
            1042,
            &[src.sid, 0, 0, read.sid, 0, 0, 0, 0, 0, W, H, 1, 0, 0, 0],
        );
        submit(e, false, false)?;
        read.dma.poison(poison);
        let mut e = CmdEncoder::new();
        cmd(&mut e, 1104, &[read.sid]);
        submit(e, false, false)?;
        self.sequence += 1;
        let (id, mem) = self.fence.as_ref().ok_or("missing fence MOB")?;
        let mut e = CmdEncoder::new();
        cmd(
            &mut e,
            1216,
            &[self.sequence as u32, (self.sequence >> 32) as u32, *id, 0],
        );
        submit(e, true, false)?;
        let start = ticks();
        loop {
            mem.invalidate();
            if unsafe { core::ptr::read_volatile(mem.ptr as *const u64) } == self.sequence {
                break;
            }
            if ticks() - start > frequency() * 2 {
                return Err("GPU fence timeout");
            }
            core::hint::spin_loop();
        }
        read.dma.invalidate();
        let bytes = unsafe { core::slice::from_raw_parts(read.dma.ptr, (W * H * 4) as usize) };
        Ok(bytes.to_vec())
    }
    fn judge(
        &mut self,
        name: &str,
        src: &Surface,
        read: &Surface,
        expected: impl Fn(u32, u32) -> [u8; 4],
        fault: bool,
    ) -> Result<()> {
        let begin = ticks();
        let data = self.read(src, read, 0xa5)?;
        let confirmation = self.read(src, read, 0x5a)?;
        let mut mismatches = 0;
        let mut first = None;
        for y in 0..H {
            for x in 0..W {
                let mut want = expected(x, y);
                if src.format == BGRA {
                    want.swap(0, 2);
                }
                let offset = ((y * W + x) * 4) as usize;
                if data[offset..offset + 4] != want || confirmation[offset..offset + 4] != want {
                    mismatches += 1;
                    if first.is_none() {
                        let observed = if data[offset..offset + 4] != want {
                            &data[offset..offset + 4]
                        } else {
                            &confirmation[offset..offset + 4]
                        };
                        first = Some((
                            x,
                            y,
                            want,
                            [observed[0], observed[1], observed[2], observed[3]],
                        ));
                    }
                }
            }
        }
        self.pixels += (2 * W * H) as usize;
        let pass = if fault {
            mismatches > 0 && data == confirmation
        } else {
            mismatches == 0
        };
        if pass {
            self.passed += 1;
        } else {
            self.failed += 1;
        }
        serial_println!(
            "GPU_SPIKE CASE {} {} pixels={} observations=2 mismatches={} fault={} read_us={}",
            name,
            if pass { "PASS" } else { "FAIL" },
            W * H,
            mismatches,
            fault,
            (ticks() - begin) * 1_000_000 / frequency()
        );
        if let Some((x, y, want, got)) = first {
            serial_println!(
                "GPU_SPIKE first x={} y={} expected={:?} actual={:?}",
                x,
                y,
                want,
                got
            );
        }
        Ok(())
    }
}

// SM4 operands, described by Microsoft's token format: mask destinations,
// swizzle/select sources, 1D immediate indices, literal32 immediate vectors.
fn dst(kind: u32, n: u32, mask: u32) -> Vec<u32> {
    vec![2 | (mask << 4) | (kind << 12) | (1 << 20), n]
}
fn src(kind: u32, n: u32) -> Vec<u32> {
    vec![2 | 4 | (0xe4 << 4) | (kind << 12) | (1 << 20), n]
}
fn scalar(n: u32, c: u32) -> Vec<u32> {
    vec![2 | 8 | (c << 4) | (1 << 20), n]
}
fn imm(v: u32) -> Vec<u32> {
    vec![1 | (4 << 12), v]
}
fn literal(v: [u32; 4]) -> Vec<u32> {
    let mut x = vec![2 | (4 << 12)];
    x.extend_from_slice(&v);
    x
}
struct Shader {
    code: Vec<u32>,
}
impl Shader {
    fn emit(&mut self, op: u32, args: &[u32]) {
        self.code.push(op | (((args.len() + 1) as u32) << 24));
        self.code.extend_from_slice(args);
    }
    fn inst(&mut self, op: u32, args: &[Vec<u32>]) {
        let v: Vec<u32> = args.iter().flatten().copied().collect();
        self.emit(op, &v);
    }
    fn pixel(texture: bool) -> Self {
        let mut s = Self {
            code: vec![0x40, 0],
        };
        s.inst(100 | (4 << 11), &[dst(1, 0, 15), vec![1]]); // linear noperspective SV_Position
        s.inst(101, &[dst(2, 0, 15)]);
        s.emit(104, &[24]);
        if texture {
            s.inst(
                88 | (3 << 11),
                &[vec![(7 << 12) | (1 << 20), 0], vec![0x5555]],
            );
        }
        s.inst(28, &[dst(0, 0, 3), src(1, 0)]); // truncate positive half-integer fragment centres
        s.inst(54, &[dst(0, 0, 12), imm(0)]); // z and mip zero for ld
        s
    }
    fn output_bytes(&mut self, n: u32) {
        self.inst(86, &[dst(0, n, 15), src(0, n)]);
        self.inst(56, &[dst(2, 0, 15), src(0, n), imm(0x3b808081)]); // nearest binary32 1/255
    }
}
fn vertex() -> Shader {
    let mut s = Shader {
        code: vec![0x10040, 0],
    };
    s.inst(95, &[dst(1, 0, 15)]);
    s.inst(103, &[dst(2, 0, 15), vec![1]]);
    s.inst(54, &[dst(2, 0, 15), src(1, 0)]);
    s
}
fn colour(value: [u8; 4]) -> Shader {
    let mut s = Shader::pixel(false);
    s.inst(54, &[dst(0, 1, 15), literal(value.map(|x| x as u32))]);
    s.output_bytes(1);
    s
}
fn unorm() -> Shader {
    let mut s = Shader::pixel(false);
    s.inst(54, &[dst(0, 1, 15), scalar(0, 0)]);
    s.output_bytes(1);
    s
}
fn fetch(mask: bool) -> Shader {
    let mut s = Shader::pixel(true);
    s.inst(45, &[dst(0, 1, 15), src(0, 0), src(7, 0)]);
    if mask {
        s.inst(54, &[dst(0, 1, 15), scalar(1, 0)]);
    }
    s.inst(54, &[dst(2, 0, 15), src(0, 1)]);
    s
}
fn triangle() -> Shader {
    let mut s = colour([219, 37, 83, 255]);
    // Right triangle x>=32,y>=32,x+y<224; strict diagonal never samples an edge.
    s.inst(79, &[dst(0, 2, 1), scalar(0, 0), imm(32)]);
    s.inst(79, &[dst(0, 2, 2), scalar(0, 1), imm(32)]);
    s.inst(30, &[dst(0, 3, 1), scalar(0, 0), scalar(0, 1)]);
    s.inst(80, &[dst(0, 2, 4), scalar(3, 0), imm(224)]);
    s.inst(60, &[dst(0, 2, 1), scalar(2, 0), scalar(2, 1)]);
    s.inst(60, &[dst(0, 2, 1), scalar(2, 0), scalar(2, 2)]);
    s.inst(13 | (1 << 18), &[scalar(2, 0)]);
    s
}
fn over() -> Shader {
    let mut s = Shader::pixel(true);
    s.inst(45, &[dst(0, 1, 15), src(0, 0), src(7, 0)]);
    s.inst(56, &[dst(0, 1, 15), src(0, 1), imm(0x437f0000)]); // float255
    s.inst(0, &[dst(0, 1, 15), src(0, 1), imm(0x3f000000)]);
    s.inst(28, &[dst(0, 1, 15), src(0, 1)]);
    // src=(x,x,x,y) straight. Premultiply: (x*y+127)/255, then over copy.
    s.inst(35, &[dst(0, 2, 15), scalar(0, 0), scalar(0, 1), imm(127)]);
    s.inst(78, &[dst(0, 2, 15), vec![13 << 12], src(0, 2), imm(255)]);
    s.inst(54, &[dst(0, 2, 8), scalar(0, 1)]);
    s.inst(40, &[dst(0, 3, 1), scalar(0, 1)]);
    s.inst(30, &[dst(0, 3, 1), scalar(3, 0), imm(255)]);
    s.inst(35, &[dst(0, 1, 15), src(0, 1), scalar(3, 0), imm(127)]);
    s.inst(78, &[dst(0, 1, 15), vec![13 << 12], src(0, 1), imm(255)]);
    s.inst(30, &[dst(0, 1, 15), src(0, 1), src(0, 2)]);
    s.output_bytes(1);
    s
}
fn wide(high: bool) -> Shader {
    let mut s = Shader::pixel(false);
    // Varied full-width operands. Product via four 16x16 low-word products.
    s.inst(
        35,
        &[dst(0, 1, 1), scalar(0, 0), imm(0x01010101), imm(0xfedcba98)],
    );
    s.inst(
        35,
        &[dst(0, 1, 2), scalar(0, 1), imm(0x00100101), imm(0x87654321)],
    );
    s.inst(1, &[dst(0, 2, 1), scalar(1, 0), imm(65535)]);
    s.inst(85, &[dst(0, 2, 2), scalar(1, 0), imm(16)]);
    s.inst(1, &[dst(0, 3, 1), scalar(1, 1), imm(65535)]);
    s.inst(85, &[dst(0, 3, 2), scalar(1, 1), imm(16)]);
    for (r, a, b) in [(4, 0, 0), (5, 0, 1), (6, 1, 0), (7, 1, 1)] {
        s.inst(
            38,
            &[vec![13 << 12], dst(0, r, 1), scalar(2, a), scalar(3, b)],
        );
    }
    // middle = (p00>>16)+(p01&65535)+(p10&65535), no overflow.
    s.inst(85, &[dst(0, 8, 1), scalar(4, 0), imm(16)]);
    for r in [5, 6] {
        s.inst(1, &[dst(0, 9, 1), scalar(r, 0), imm(65535)]);
        s.inst(30, &[dst(0, 8, 1), scalar(8, 0), scalar(9, 0)]);
    }
    if high {
        for r in [5, 6, 8] {
            s.inst(85, &[dst(0, 9, 1), scalar(r, 0), imm(16)]);
            s.inst(30, &[dst(0, 7, 1), scalar(7, 0), scalar(9, 0)]);
        }
        s.inst(54, &[dst(0, 10, 1), scalar(7, 0)]);
    } else {
        s.inst(1, &[dst(0, 10, 1), scalar(4, 0), imm(65535)]);
        s.inst(41, &[dst(0, 8, 1), scalar(8, 0), imm(16)]);
        s.inst(60, &[dst(0, 10, 1), scalar(10, 0), scalar(8, 0)]);
    }
    for c in 0..4 {
        s.inst(85, &[dst(0, 11, 1 << c), scalar(10, 0), imm(c * 8)]);
    }
    s.inst(1, &[dst(0, 11, 15), src(0, 11), imm(255)]);
    s.output_bytes(11);
    s
}

// Test a general triangle with fragment-centre edge functions and explicit
// top-left ties; the independent reference below evaluates edges with i128.
fn triangle_top_left() -> Shader {
    let mut s = colour([219, 37, 83, 255]);
    for (a, b) in [
        ((21i32, 31i32), (235i32, 62i32)),
        ((235, 62), (83, 230)),
        ((83, 230), (21, 31)),
    ] {
        let dx = b.0 - a.0;
        let dy = b.1 - a.1;
        s.inst(
            35,
            &[
                dst(0, 2, 1),
                scalar(0, 0),
                imm(2),
                imm((1 - 2 * a.0) as u32),
            ],
        );
        s.inst(
            35,
            &[
                dst(0, 2, 2),
                scalar(0, 1),
                imm(2),
                imm((1 - 2 * a.1) as u32),
            ],
        );
        s.inst(35, &[dst(0, 3, 1), scalar(2, 0), imm((-dy) as u32), imm(0)]);
        s.inst(
            35,
            &[dst(0, 3, 1), scalar(2, 1), imm(dx as u32), scalar(3, 0)],
        );
        s.inst(34, &[dst(0, 3, 2), scalar(3, 0), imm(0)]);
        if !(dy < 0 || (dy == 0 && dx > 0)) {
            s.inst(32, &[dst(0, 3, 4), scalar(3, 0), imm(0)]);
            s.inst(60, &[dst(0, 3, 2), scalar(3, 1), scalar(3, 2)]);
        }
        s.inst(13 | (1 << 18), &[scalar(3, 1)]);
    }
    s
}

// Restoring division over three 32-bit words. No native high multiply or
// native wide division. Exactly 96 steps, written here as a bounded encoder
// loop; each generated program returns one quotient word as four UNORM bytes.
fn divide96(word: u32) -> Shader {
    let mut s = Shader::pixel(false);
    s.inst(
        35,
        &[dst(0, 1, 1), scalar(0, 0), imm(0x01010101), imm(0xfedcba98)],
    );
    s.inst(
        35,
        &[dst(0, 1, 2), scalar(0, 1), imm(0x00100101), imm(0x87654321)],
    );
    s.inst(41, &[dst(0, 1, 4), scalar(0, 0), imm(24)]);
    s.inst(87, &[dst(0, 1, 4), scalar(1, 2), imm(0x80000000)]);
    s.inst(35, &[dst(0, 5, 1), scalar(0, 1), imm(0x00010001), imm(0)]);
    s.inst(87, &[dst(0, 1, 4), scalar(1, 2), scalar(5, 0)]);
    s.inst(35, &[dst(0, 1, 8), scalar(0, 1), imm(65537), imm(1)]);
    s.inst(60, &[dst(0, 5, 1), scalar(1, 3), imm(0x80000000)]);
    s.inst(1, &[dst(0, 5, 4), scalar(0, 1), imm(1)]);
    s.inst(
        55,
        &[dst(0, 1, 8), scalar(5, 2), scalar(5, 0), scalar(1, 3)],
    );
    s.inst(40, &[dst(0, 5, 2), scalar(1, 3)]);
    s.inst(54, &[dst(0, 2, 15), imm(0)]);
    s.inst(54, &[dst(0, 3, 1), imm(0)]);
    for bit in (0..96).rev() {
        s.inst(85, &[dst(0, 4, 1), scalar(3, 0), imm(31)]);
        s.inst(41, &[dst(0, 3, 1), scalar(3, 0), imm(1)]);
        s.inst(85, &[dst(0, 4, 2), scalar(1, bit / 32), imm(bit % 32)]);
        s.inst(1, &[dst(0, 4, 2), scalar(4, 1), imm(1)]);
        s.inst(60, &[dst(0, 3, 1), scalar(3, 0), scalar(4, 1)]);
        s.inst(80, &[dst(0, 4, 4), scalar(3, 0), scalar(1, 3)]);
        s.inst(60, &[dst(0, 4, 4), scalar(4, 2), scalar(4, 0)]);
        s.inst(30, &[dst(0, 4, 8), scalar(3, 0), scalar(5, 1)]);
        s.inst(
            55,
            &[dst(0, 3, 1), scalar(4, 2), scalar(4, 3), scalar(3, 0)],
        );
        s.inst(1, &[dst(0, 4, 2), scalar(4, 2), imm(1)]);
        s.inst(41, &[dst(0, 4, 2), scalar(4, 1), imm(bit % 32)]);
        s.inst(
            60,
            &[
                dst(0, 2, 1 << (bit / 32)),
                scalar(2, bit / 32),
                scalar(4, 1),
            ],
        );
    }
    for c in 0..4 {
        s.inst(85, &[dst(0, 11, 1 << c), scalar(2, word), imm(c * 8)]);
    }
    s.inst(1, &[dst(0, 11, 15), src(0, 11), imm(255)]);
    s.output_bytes(11);
    s
}
fn reference96(x: u32, y: u32, word: u32) -> [u8; 4] {
    let lo = x.wrapping_mul(0x01010101).wrapping_add(0xfedcba98);
    let mid = y.wrapping_mul(0x00100101).wrapping_add(0x87654321);
    let hi = 0x80000000 ^ (x << 24) ^ y.wrapping_mul(65537);
    let divisor = (y * 65537 + 1) | if y % 2 == 1 { 0x80000000 } else { 0 };
    let numerator = ((hi as u128) << 64) | ((mid as u128) << 32) | lo as u128;
    (((numerator / (divisor as u128)) >> (word * 32)) as u32).to_le_bytes()
}
fn reference_over(x: u32, y: u32, draws: u32) -> [u8; 4] {
    let mut dst = [17u32, 83, 151, 239];
    for _ in 0..draws {
        for (c, d) in dst.iter_mut().enumerate() {
            let source = if c == 3 { y } else { (x * y + 127) / 255 };
            *d = source + (*d * (255 - y) + 127) / 255;
        }
    }
    dst.map(|c| c as u8)
}

pub(super) fn run() -> ! {
    serial_println!("GPU_SPIKE START backend=svga3-dx size={}x{}", W, H);
    let mut p = Probe {
        next: 16,
        passed: 0,
        failed: 0,
        pixels: 0,
        fence: None,
        sequence: 0,
        rtv: false,
        srv: false,
    };
    if let Err(reason) = execute(&mut p) {
        p.failed += 1;
        serial_println!("GPU_SPIKE BLOCKER {}", reason);
    }
    serial_println!(
        "GPU_SPIKE DONE passed={} failed={} compared_pixels={}",
        p.passed,
        p.failed,
        p.pixels
    );
    // This is a standalone scratch workload, not a normal desktop boot.
    // Leave the final GPU image visible without creating idle desktop load.
    loop {
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack));
        }
    }
}
fn execute(p: &mut Probe) -> Result<()> {
    for (name, n, required) in [
        ("DXCONTEXT", 95, 1),
        ("BGRA", 240, 7),
        ("RGBA", 167, 7),
        ("R8", 192, 3),
    ] {
        let v = devcap(n);
        serial_println!(
            "GPU_SPIKE CAP {} index={} value={:#x} required={:#x}",
            name,
            n,
            v,
            required
        );
        if v & required != required {
            return Err("profile capability missing");
        }
    }
    // Device context commands are unframed; rendering queue 0 must be started.
    let mut e = CmdEncoder::new();
    for v in [1, 1, 0] {
        e.push_u32(v);
    }
    submit(e, false, true)?;
    // Published entries: MOB=16, surface=64, screen target=64, DX context=8.
    for (ty, size) in [(0, 4096), (1, 4096), (4, 4096), (5, 4096)] {
        p.table(ty, size)?;
    }
    let (mob, _context) = p.mob(8192)?;
    let mut e = CmdEncoder::new();
    cmd(&mut e, 1143, &[CID]);
    cmd(&mut e, 1145, &[CID, mob, 0]);
    submit(e, false, false)?;
    // Every used object table gets space for IDs 0..15; element layouts are 1KiB.
    for (ty, entry) in [
        (0, 32),
        (2, 32),
        (3, 1024),
        (4, 128),
        (5, 16),
        (6, 32),
        (10, 32),
    ] {
        let (mob, _table) = p.mob(entry * 16)?;
        let mut e = CmdEncoder::new();
        cmd(&mut e, 1207, &[CID, mob, ty, 0]);
        submit(e, false, false)?;
    }
    serial_println!("GPU_SPIKE context-bound cotables=7 context_backing=8192");
    p.fence = Some(p.mob(4096)?);
    p.shader(0, 1, vertex())?;
    for (id, s) in [
        (1, colour([17, 83, 151, 239])),
        (2, unorm()),
        (3, fetch(false)),
        (4, fetch(true)),
        (5, triangle()),
        (6, over()),
        (7, wide(false)),
        (8, wide(true)),
        (9, triangle_top_left()),
        (10, divide96(0)),
        (11, divide96(1)),
        (12, divide96(2)),
    ] {
        p.shader(id, 2, s)?;
    }
    serial_println!("GPU_SPIKE shaders-bound programs=13 format=SM4+VMware-signature");
    let vb = p.surface(37, 1 << 20, 96, 1)?;
    let vertices: [u32; 24] = [
        0xbf800000, 0x3f800000, 0, 0x3f800000, 0x3f800000, 0x3f800000, 0, 0x3f800000, 0xbf800000,
        0xbf800000, 0, 0x3f800000, 0xbf800000, 0xbf800000, 0, 0x3f800000, 0x3f800000, 0x3f800000,
        0, 0x3f800000, 0x3f800000, 0xbf800000, 0, 0x3f800000,
    ];
    p.upload(&vb, &words(&vertices))?;
    let mut e = CmdEncoder::new();
    cmd(&mut e, 1191, &[0, 0, 0, 122, 0, 0, 0]);
    cmd(&mut e, 1157, &[0]);
    cmd(&mut e, 1158, &[0, vb.sid, 16, 0]);
    cmd(&mut e, 1160, &[1]);
    cmd(&mut e, 1150, &[0, 1]);
    // Explicit no blending (all channels writable), depth/stencil disabled.
    let mut blend = vec![0, 0];
    for _ in 0..8 {
        blend.extend_from_slice(&[0x01010200, 0x0f010102, 0]);
    }
    cmd(&mut e, 1193, &blend);
    cmd(&mut e, 1162, &[0, 0, 0, 0, 0, INVALID]);
    cmd(&mut e, 1195, &[0, 0x00080100, 0, 0x08010101, 0x08010101]);
    cmd(&mut e, 1163, &[0, 0]);
    cmd(&mut e, 1197, &[0, 0x00000103, 0, 0, 0, 1, 0x3f800000, 0]);
    cmd(
        &mut e,
        1197,
        &[1, 0x00000103, 0, 0, 0, 0x101, 0x3f800000, 0],
    );
    cmd(&mut e, 1164, &[0]);
    cmd(
        &mut e,
        1174,
        &[0, 0, 0, 0x43800000, 0x43800000, 0, 0x3f800000],
    );
    submit(e, true, false)?;
    let bg = p.surface(BGRA, RT | SR | (1 << 16), W, H)?;
    let rg = p.surface(RGBA, RT | SR, W, H)?;
    let mask = p.surface(R8, SR, W, H)?;
    let scratch = p.surface(BGRA, SR | RT, W, H)?;
    let scratch_r = p.surface(RGBA, SR | RT, W, H)?;
    let read_b = p.surface(BGRA, 0, W, H)?;
    let read_r = p.surface(RGBA, 0, W, H)?;
    for (target, read) in [(&bg, &read_b), (&rg, &read_r)] {
        p.target(target)?;
        p.clear()?;
        p.judge("clear", target, read, |_, _| [0; 4], false)?;
        p.draw(1)?;
        p.judge("solid-quad", target, read, |_, _| [17, 83, 151, 239], false)?;
        p.draw(2)?;
        p.judge(
            "all256-unorm-each-channel",
            target,
            read,
            |x, _| [x as u8; 4],
            false,
        )?;
        p.clear()?;
        p.draw(5)?;
        p.judge(
            "triangle-integer",
            target,
            read,
            |x, y| {
                if x >= 32 && y >= 32 && x + y < 224 {
                    [219, 37, 83, 255]
                } else {
                    [0; 4]
                }
            },
            false,
        )?;
        // A partial rectangle exposes both missed pixels and writes outside.
        let partial: [u32; 24] = [
            0xbf400000, 0x3f200000, 0, 0x3f800000, 0x3f400000, 0x3f200000, 0, 0x3f800000,
            0xbf400000, 0xbf200000, 0, 0x3f800000, 0xbf400000, 0xbf200000, 0, 0x3f800000,
            0x3f400000, 0x3f200000, 0, 0x3f800000, 0x3f400000, 0xbf200000, 0, 0x3f800000,
        ];
        p.upload(&vb, &words(&partial))?;
        p.clear()?;
        p.draw(1)?;
        p.judge(
            "rect-half-open",
            target,
            read,
            |x, y| {
                if (32..224).contains(&x) && (48..208).contains(&y) {
                    [17, 83, 151, 239]
                } else {
                    [0; 4]
                }
            },
            false,
        )?;
        p.upload(&vb, &words(&vertices))?;
        p.clear()?;
        p.draw(9)?;
        p.judge(
            "triangle-top-left",
            target,
            read,
            |x, y| {
                let inside = [
                    ((21i128, 31i128), (235i128, 62i128)),
                    ((235, 62), (83, 230)),
                    ((83, 230), (21, 31)),
                ]
                .iter()
                .all(|(a, b)| {
                    let dx = b.0 - a.0;
                    let dy = b.1 - a.1;
                    let edge =
                        dx * (2 * y as i128 + 1 - 2 * a.1) - dy * (2 * x as i128 + 1 - 2 * a.0);
                    edge > 0 || (edge == 0 && (dy < 0 || (dy == 0 && dx > 0)))
                });
                if inside {
                    [219, 37, 83, 255]
                } else {
                    [0; 4]
                }
            },
            false,
        )?;
        p.clear()?;
        p.scissor(true)?;
        p.draw(1)?;
        p.judge(
            "scissor-half-open",
            target,
            read,
            |x, y| {
                if (23..191).contains(&x) && (41..217).contains(&y) {
                    [17, 83, 151, 239]
                } else {
                    [0; 4]
                }
            },
            false,
        )?;
        p.scissor(false)?;
        for word in 0..3 {
            p.draw(10 + word)?;
            p.judge(
                match word {
                    0 => "divide96-low",
                    1 => "divide96-mid",
                    _ => "divide96-high",
                },
                target,
                read,
                |x, y| reference96(x, y, word),
                false,
            )?;
        }
        for high in [false, true] {
            p.draw(if high { 8 } else { 7 })?;
            p.judge(
                if high {
                    "multiply-high"
                } else {
                    "multiply-low"
                },
                target,
                read,
                |x, y| {
                    let a = x.wrapping_mul(0x01010101).wrapping_add(0xfedcba98) as u64;
                    let b = y.wrapping_mul(0x00100101).wrapping_add(0x87654321) as u64;
                    ((a * b >> if high { 32 } else { 0 }) as u32).to_le_bytes()
                },
                false,
            )?;
        }
    }
    for source in [&bg, &rg] {
        let fixture: Vec<u8> = (0..W * H)
            .flat_map(|i| {
                // RGBA must differ from the earlier GPU copy into it: otherwise
                // a missing upload could accidentally preserve the expected image.
                let mut c = if source.format == RGBA {
                    [
                        (i as u8) ^ 0x5a,
                        ((i / W) as u8) ^ 0xa5,
                        (i as u8) ^ 0xc3,
                        255,
                    ]
                } else {
                    [i as u8, (i / W) as u8, (i as u8) ^ 0x69, 255]
                };
                if source.format == BGRA {
                    c.swap(0, 2);
                }
                c
            })
            .collect();
        p.upload(source, &fixture)?;
        let (target, read) = if source.format == BGRA {
            (&rg, &read_r)
        } else {
            (&bg, &read_b)
        };
        p.target(target)?;
        p.texture(source)?;
        p.draw(3)?;
        p.judge(
            "ld-format-swizzle",
            target,
            read,
            |x, y| {
                if source.format == RGBA {
                    [(x as u8) ^ 0x5a, (y as u8) ^ 0xa5, (x as u8) ^ 0xc3, 255]
                } else {
                    [x as u8, y as u8, (x as u8) ^ 0x69, 255]
                }
            },
            false,
        )?;
    }
    p.upload(&mask, &(0..W * H).map(|i| i as u8).collect::<Vec<_>>())?;
    p.target(&bg)?;
    p.texture(&mask)?;
    p.draw(4)?;
    p.judge("ld-R8", &bg, &read_b, |x, _| [x as u8; 4], false)?;
    p.draw(1)?;
    p.copy(&bg, &scratch)?;
    p.texture(&scratch)?;
    p.draw(6)?;
    p.judge(
        "destination-copy-straight-over",
        &bg,
        &read_b,
        |x, y| {
            let mut out = [0; 4];
            for (i, d) in [17, 83, 151, 239].iter().enumerate() {
                let s = if i == 3 { y } else { (x * y + 127) / 255 };
                out[i] = (s + (d * (255 - y) + 127) / 255) as u8;
            }
            out
        },
        false,
    )?;
    p.copy(&bg, &scratch)?;
    p.texture(&scratch)?;
    p.draw(6)?;
    p.judge(
        "over-overlap",
        &bg,
        &read_b,
        |x, y| reference_over(x, y, 2),
        false,
    )?;
    p.target(&rg)?;
    p.draw(1)?;
    p.copy(&rg, &scratch_r)?;
    p.texture(&scratch_r)?;
    p.draw(6)?;
    p.judge(
        "destination-copy-straight-over",
        &rg,
        &read_r,
        |x, y| reference_over(x, y, 1),
        false,
    )?;
    p.copy(&rg, &scratch_r)?;
    p.texture(&scratch_r)?;
    p.draw(6)?;
    p.judge(
        "over-overlap",
        &rg,
        &read_r,
        |x, y| reference_over(x, y, 2),
        false,
    )?;
    // Planted translator failure: suppress intended solid draw, retain completed
    // command fence and readback. Independent expected image remains the solid.
    p.judge(
        "fault-suppressed-draw",
        &bg,
        &read_b,
        |_, _| [17, 83, 151, 239],
        true,
    )?;
    let mut e = CmdEncoder::new();
    e.define_gb_screentarget(0, W, H, 1);
    e.bind_gb_screentarget(0, bg.sid);
    e.update_gb_screentarget(0, 0, 0, W, H);
    submit(e, false, false)?;
    serial_println!("GPU_SPIKE present-completed surface=BGRA size={}x{}", W, H);
    p.passed += 1;
    p.judge(
        "presented-target",
        &bg,
        &read_b,
        |x, y| reference_over(x, y, 2),
        false,
    )?;
    Ok(())
}
