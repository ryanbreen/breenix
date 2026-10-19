//! `device`: the 3D device and its capability profile (`docs/gui/gpu.md` sections 1.4,
//! 1.5 item 8 and 1.6; the CAPS record's layout in `docs/gui/gpu-abi.md`).

use libbreenix::gpu::{self, abi};
use libbreenix::suite::{value, CaseError, CaseResult};

use super::common::*;

/// The CAPS record as raw bytes, so its layout is checked at the documented offsets.
fn raw(c: &abi::Caps) -> [u8; 512] {
    // SAFETY: Caps is 512 bytes of integers.
    unsafe { core::mem::transmute::<abi::Caps, [u8; 512]>(*c) }
}

pub fn feature_negotiated() -> CaseResult {
    let _case = begin()?;
    let c = caps()?;
    match c.backend {
        abi::backend::VIRGL => {
            let both = abi::feature::VIRGL | abi::feature::VERSION_1;
            ensure(c.features & both == both, || format!("VirGL backend without VIRGL and VERSION_1 negotiated: features {:#x}", c.features))
        }
        abi::backend::SVGA3D_DX => {
            ensure(c.features & abi::feature::DX_CONTEXT != 0, || format!("SVGA3D DX backend without the DX-context entry: features {:#x}", c.features))
        }
        b => Err(fail_msg(format!("CAPS names backend {b}, neither 4 (VirGL) nor 5 (SVGA3D DX)"))),
    }
}

pub fn caps_parsed() -> CaseResult {
    let _case = begin()?;
    let c = caps()?;
    let n = c.capset_count as usize;
    ensure((1..=8).contains(&n), || format!("CAPS lists {n} capability sets"))?;
    let ids = &c.capset_ids[..n];
    ensure(ids.iter().all(|&i| i != 0), || format!("a listed capability set id is 0: {ids:?}"))?;
    ensure(ids.windows(2).all(|w| w[0] < w[1]), || format!("capability set ids are not each listed once in ascending order: {ids:?}"))?;
    ensure(c.capset_ids[n..].iter().all(|&i| i == 0), || format!("ids past the count are not zero: {:?}", c.capset_ids))?;
    ensure(ids.contains(&c.capset_id), || format!("the set read, {}, is not among the listed {ids:?}", c.capset_id))?;
    if c.backend == abi::backend::VIRGL {
        // VirtIO 1.3 section 5.7.6.7: capability set 1 is VIRGL, 2 is VIRGL2.
        ensure(c.capset_id == 1 || c.capset_id == 2, || format!("the VirGL profile came from capability set {}", c.capset_id))?;
    }
    ensure(c.capset_version >= 1, || format!("capability set version {}", c.capset_version))?;
    value("capability-sets", n as i64, "", Some((1, 8)));
    pass()
}

pub fn profile_formats() -> CaseResult {
    let _case = begin()?;
    let c = caps()?;
    let both = abi::format_flag::SAMPLE | abi::format_flag::RENDER;
    let want = [(format::B8G8R8A8, both, "B8G8R8A8"), (format::R8G8B8A8, both, "R8G8B8A8"), (format::R8, abi::format_flag::SAMPLE, "R8")];
    for (i, (f, flags, name)) in want.iter().enumerate() {
        let e = c.formats[i];
        ensure(e.format == *f, || format!("format entry {i} is format {}, expected {name}", e.format))?;
        ensure(e.flags & flags == *flags, || format!("{name}: the device's entry has flags {:#x}, needs {flags:#x}", e.flags))?;
    }
    pass()
}

pub fn profile_integer_ops() -> CaseResult {
    let _case = begin()?;
    let c = caps()?;
    let need = abi::profile::INTEGER_OPS | abi::profile::TEXEL_FETCH | abi::profile::REGION_COPY;
    ensure(c.profile & need == need, || format!("profile bits {:#x}: integer ops, texel fetch and region copies need {need:#x}", c.profile))?;
    ensure(c.shading_level > 0, || "the device reports no shading-language level".to_string())?;
    value("shading-level", c.shading_level as i64, "", None);
    pass()
}

pub fn profile_limits() -> CaseResult {
    let _case = begin()?;
    let c = caps()?;
    value("max-target-width", c.max_width as i64, "", Some((2560, 16_384)));
    ensure((2560..=16_384).contains(&c.max_width), || format!("maximum target width {}", c.max_width))?;
    ensure((1600..=16_384).contains(&c.max_height), || format!("maximum target height {}", c.max_height))
}

pub fn profile_record() -> CaseResult {
    let _case = begin()?;
    let c = caps()?;
    let b = raw(&c);
    ensure(u32_at(&b, 0) == 1, || format!("record version {}", u32_at(&b, 0)))?;
    ensure(u32_at(&b, 4) == 512, || format!("record size {}", u32_at(&b, 4)))?;
    ensure(u32_at(&b, 8) == abi::status::ADMITTED && u32_at(&b, 12) == 0, || "status and missing are not 0 for an admitted device".to_string())?;
    ensure(matches!(u32_at(&b, 16), 4 | 5), || format!("backend at offset 16 is {}", u32_at(&b, 16)))?;
    ensure(u32_at(&b, 108) == 1, || format!("catalogue version {}", u32_at(&b, 108)))?;
    ensure(u32_at(&b, 112) & 1 << pipeline::SOLID != 0, || "the pipeline mask lacks solid".to_string())?;
    ensure(b[120..128].iter().all(|&x| x == 0), || "reserved bytes 120 to 127 are not zero".to_string())?;
    ensure(b[192..256].iter().all(|&x| x == 0), || "the reason is not empty for an admitted device".to_string())?;
    let renderer = &b[128..192];
    if let Some(nul) = renderer.iter().position(|&x| x == 0) {
        ensure(renderer[nul..].iter().all(|&x| x == 0), || "the renderer string is not NUL-padded".to_string())?;
    }
    let n = u32_at(&b, 32) as usize;
    ensure(b[36 + 4 * n.min(8)..68].iter().all(|&x| x == 0), || "capability ids past the count are not zero".to_string())?;
    let scanouts = u32_at(&b, 116) as usize;
    ensure(scanouts <= 16, || format!("{scanouts} scanouts"))?;
    for i in 0..16 {
        let e = &b[256 + 16 * i..256 + 16 * (i + 1)];
        ensure(u32_at(e, 12) == 0, || format!("scanout {i}'s reserved word is not zero"))?;
        if i >= scanouts {
            ensure(e.iter().all(|&x| x == 0), || format!("scanout entry {i}, past the count, is not zero"))?;
        }
    }
    // The record's size is its length: any other length is refused.
    let mut big = [0u8; 513];
    // SAFETY: a live buffer of the length passed.
    let r = unsafe { gpu::call(abi::op::CAPS, big.as_mut_ptr(), big.len()) };
    ensure(r == -EINVAL, || format!("CAPS with a 513-byte argument returned {}", errno(r)))
}

pub fn missing_capability_fails() -> CaseResult {
    let _case = begin()?;
    for cap in 1..=capability::LAST {
        let before = fired(test_point::MASK_CAPABILITY);
        arm(test_point::MASK_CAPABILITY, cap as u64)?;
        let c = caps()?;
        let reason = text(&c.reason);
        let want = format!("missing {}", capability::name(cap));
        ensure(c.status == abi::status::MISSING_CAPABILITY, || format!("masking {} left status {}", capability::name(cap), c.status))?;
        ensure(c.missing == cap, || format!("masking {} reports capability {} missing", capability::name(cap), c.missing))?;
        ensure(reason == want, || format!("masking {}: the reason is {reason:?}, expected {want:?}", capability::name(cap)))?;
        ensure(fired(test_point::MASK_CAPABILITY) == before + 1, || "the mask did not fire exactly once".to_string())?;
        // One-shot (`docs/gui/gpu.md` section 1.8): the next decision sees the device's
        // real capabilities without DISARM_ALL.
        let c = caps()?;
        ensure(c.status == abi::status::ADMITTED && c.missing == 0, || format!("the CAPS after the one masking {} has status {} missing {}", capability::name(cap), c.status, c.missing))?;
    }
    pass()
}

pub fn no_fallback() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = target(format::B8G8R8A8, 16, 16)?;
    let green = bgra(0, 0xFF, 0, 0xFF);
    frame(ctx, &scene(t, green, &[]))?;
    let (_, rec) = read(t, 16, 16)?;
    let generation = u64_at(&rec, record::GENERATION);
    // Point 23 is one-shot (`docs/gui/gpu.md` section 1.8): armed before each op, it
    // refuses that op alone, which falls back to nothing.
    let mask = || -> Result<u64, CaseError> {
        let before = fired(test_point::MASK_CAPABILITY);
        arm(test_point::MASK_CAPABILITY, capability::REGION_COPY as u64)?;
        Ok(before)
    };
    let once = |before: u64, what: &str| {
        let n = fired(test_point::MASK_CAPABILITY);
        ensure(n == before + 1, || format!("{what}: the mask fired {} times, expected once", n - before))
    };
    let mut arg = [0u8; 256];
    for op in abi::op::CONTEXT_CREATE..=abi::op::LAST {
        let before = mask()?;
        // SAFETY: a live 256-byte buffer, at least every op's argument size.
        let r = unsafe { gpu::call(op, arg.as_mut_ptr(), 0) };
        ensure(r == -ENODEV, || format!("with region-copy masked, op {op} returned {}", errno(r)))?;
        once(before, &format!("op {op}"))?;
    }
    let before = mask()?;
    let (r, _) = gpu::submit(ctx, &scene(t, bgra(0, 0, 0xFF, 0xFF), &[]));
    ensure(r == -ENODEV, || format!("with region-copy masked, a valid SUBMIT returned {}", errno(r)))?;
    once(before, "SUBMIT")?;
    let before = mask()?;
    let (r, _) = gpu::counters(0);
    ensure(r == -ENODEV, || format!("with region-copy masked, COUNTERS returned {}", errno(r)))?;
    once(before, "COUNTERS")?;
    let before = mask()?;
    let c = caps()?;
    ensure(c.status == abi::status::MISSING_CAPABILITY && c.missing == capability::REGION_COPY, || format!("CAPS while masked: status {} missing {}", c.status, c.missing))?;
    once(before, "CAPS")?;
    // The mask is spent: the next decision sees the device's real capabilities.
    let c = caps()?;
    ensure(c.status == abi::status::ADMITTED && c.missing == 0, || format!("CAPS after the mask was spent: status {} missing {}", c.status, c.missing))?;
    // Nothing rendered another way: the target is as it was, at the same generation.
    let rec = expect(t, &scene_canvas(16, 16, green, &[]), "after the refused submission")?;
    ensure(u64_at(&rec, record::GENERATION) == generation, || format!("generation moved from {generation} to {}", u64_at(&rec, record::GENERATION)))?;
    let n = counters(ctx)?.submissions;
    ensure(n == 1, || format!("the context counts {n} submissions, expected 1"))
}

pub fn display_info() -> CaseResult {
    let _case = begin()?;
    let c = caps()?;
    let n = c.scanout_count as usize;
    ensure((1..=16).contains(&n), || format!("the device reports {n} scanouts"))?;
    let live = c.scanouts[..n].iter().filter(|s| s.enabled != 0 && s.width > 0 && s.height > 0).count();
    ensure(live >= 1, || format!("none of the {n} scanouts is enabled with a size: {:?}", &c.scanouts[..n]))?;
    ensure(c.scanouts[n..].iter().all(|s| *s == abi::ScanoutInfo { enabled: 0, width: 0, height: 0, reserved: 0 }), || "scanouts past the count are not zero".to_string())?;
    value("scanout-0-width", c.scanouts[0].width as i64, "px", None);
    value("scanout-0-height", c.scanouts[0].height as i64, "px", None);
    pass()
}

pub fn backend_named() -> CaseResult {
    let _case = begin()?;
    let c = caps()?;
    match c.backend {
        abi::backend::VIRGL => {
            let ids = &c.capset_ids[..c.capset_count.min(8) as usize];
            ensure(ids.iter().any(|&i| i == 1 || i == 2), || format!("backend 4 (VirGL) but the device lists no VirGL capability set: {ids:?}"))?;
            ensure(c.features & abi::feature::VIRGL != 0, || "backend 4 (VirGL) without the VirGL feature".to_string())
        }
        abi::backend::SVGA3D_DX => ensure(c.features & abi::feature::DX_CONTEXT != 0, || "backend 5 without the DX-context entry".to_string()),
        b => Err(fail_msg(format!("CAPS names backend {b}"))),
    }
}

pub fn device_error_reported() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = target(format::B8G8R8A8, 8, 8)?;
    let before = fired(test_point::DEVICE_ERROR);
    arm(test_point::DEVICE_ERROR, 0)?;
    let f = submit(ctx, &scene(t, bgra(1, 2, 3, 4), &[]))?;
    let (r, w) = gpu::wait(ctx, f, 2_000_000_000);
    ensure(fired(test_point::DEVICE_ERROR) == before + 1, || "the device-error point did not fire".to_string())?;
    ensure(r == -EIO, || format!("WAIT on the forced device error returned {}", errno(r)))?;
    ensure(w.status == fence_status::DEVICE_ERROR, || format!("the fence's status is {}, expected DEVICE_ERROR", w.status))?;
    // Native error answers: VirtIO 1.3 5.7.6.7 and VMware svga_reg.h.
    let (low, high) = match caps()?.backend {
        abi::backend::VIRGL => (0x1200, 0x12ff),
        abi::backend::SVGA3D_DX => (3, 3), // SVGA_CB_STATUS_COMMAND_ERROR
        b => return Err(fail_msg(format!("unknown backend {b}"))),
    };
    ensure((low..=high).contains(&w.answer), || format!("the device's answer {:#x} is not its backend's error response", w.answer))?;
    value("device-answer", w.answer as i64, "", Some((low as i64, high as i64)));
    pass()
}
