//! The kernel GPU core: system call 416, `gpu(op, arg, len)` (`docs/gui/gpu.md` section
//! 1.4, argument layouts in `docs/gui/gpu-abi.md`).
//!
//! Per-process contexts and resources behind opaque handles; every stream validated whole
//! before anything reaches the device (`gpu_abi::stream`); fences that complete in
//! submission order; waits that time out, take signals and end on a kill; and a watchdog
//! that quarantines a context whose submission has not completed 2 s after it was sent and
//! resets the device when its queue then makes no progress for another 2 s (section
//! 1.4.1). The device is the VirGL backend (`virgl`), on ARM64 QEMU in this milestone.
//!
//! Locking: one spin lock over the core, taken with preemption disabled and never by the
//! interrupt handler, which only reads the device's ISR byte, counts an event and wakes the
//! waiters. User memory is copied outside the lock; nothing blocks while holding it; no
//! path here prints (section 1.4: no kernel output on any path).

pub mod dma;
pub mod virgl;

use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use gpu_abi::errno::{EAGAIN, EDQUOT, EFAULT, EINTR, EINVAL, EIO, ENODEV, ENOMEM, ENOSYS, ETIMEDOUT};
use gpu_abi::stream::{self, Resolver, TargetInfo};
use gpu_abi::{self as abi, capability, fence_status, format, kind, limits, record, test_point, Plain};

use crate::syscall::SyscallResult;
use crate::task::thread::ThreadState;
use crate::task::waitqueue::WaitQueueHead;
use dma::{Pages, WordBuf, PAGE};
use virgl::{encode, wire, Device, Done, Request, Tag};

/// The guest backing one process's resources may hold.
const MEMORY_QUOTA: u64 = 192 << 20;
/// Resource handles per process.
const RESOURCE_QUOTA: usize = 256;
/// The largest buffer.
const BUFFER_MAX: u64 = 128 << 20;
/// Rows per readback transfer are chosen so a band is at most this many bytes.
const READBACK_BAND: usize = 1 << 20;
/// The first observation's poison; the second's is its complement (section 1.8).
const POISON: u32 = 0x3C5A_A5C3;
/// VirGL's renderer keeps a target's row 0 as the transfer's row 0, so the viewport keeps
/// row 0 at the top without a flip.
const FLIP_Y: bool = false;

static CORE: spin::Mutex<Option<Core>> = spin::Mutex::new(None);
/// Counts interrupts and state changes; a waiter sleeps only while it is unchanged.
static EVENTS: AtomicU64 = AtomicU64::new(0);
static WAITERS: WaitQueueHead = WaitQueueHead::new();
static ISR: AtomicU64 = AtomicU64::new(0);
static INTID: AtomicU32 = AtomicU32::new(0);
static REAPER: AtomicU64 = AtomicU64::new(0);
static REAPER_STARTING: AtomicBool = AtomicBool::new(false);

fn now_ns() -> u64 {
    crate::signal::monotonic_nanos()
}

fn with_core<R>(f: impl FnOnce(&mut Core) -> R) -> Option<R> {
    crate::per_cpu::preempt_disable();
    let r = CORE.lock().as_mut().map(f);
    crate::per_cpu::preempt_enable();
    r
}

/// Tell every waiter that something changed.
fn notify() {
    EVENTS.fetch_add(1, Ordering::AcqRel);
    WAITERS.wake_up();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CtxState {
    Creating,
    Live,
    Quarantined,
    Reset,
}

#[derive(Clone, Copy, Default)]
struct Totals {
    submissions: u64,
    draws: u64,
    instances: u64,
    area: u64,
    estimate: u64,
    readbacks: u64,
    ring_waits: u64,
    device_errors: u64,
}

struct Proc {
    contexts: BTreeMap<u64, u32>,
    resources: BTreeMap<u64, u32>,
    totals: Totals,
    seq: u64,
    bytes: u64,
}

struct Ctx {
    id: u32,
    owner: u64,
    /// The fence namespace: a BGL fence is `ns << 32 | n` for the context's `n`th
    /// submission.
    ns: u32,
    state: CtxState,
    dropped: bool,
    /// A submission is being translated and sent.
    busy: bool,
    device_alive: bool,
    issued: u32,
    done_upto: u32,
    early: BTreeSet<u32>,
    /// Fences that completed other than OK, with their status and the device's answer.
    outcomes: BTreeMap<u32, (u32, u32)>,
    inflight: VecDeque<u64>,
    totals: Totals,
    objects: encode::Objects,
    vb: u32,
    scratch: u32,
    warmed: u32,
    errors_in_row: u32,
}

struct Sub {
    ctx: u32,
    n: u32,
    fence: u64,
    sent_ns: u64,
    error: Option<u32>,
    mismatch: bool,
    written: Vec<u32>,
    held: Vec<u32>,
}

#[derive(Clone, Copy)]
struct LastWrite {
    ctx: u32,
    fence: u64,
    serial: u64,
    answer: u32,
}

struct Res {
    owner: u64,
    kind: u32,
    format: u32,
    width: u32,
    height: u32,
    size: u64,
    pitch: u32,
    backing: Arc<Pages>,
    generation: u64,
    last: Option<LastWrite>,
    writers: u32,
    invalid: bool,
    dropped: bool,
    holds: u32,
    lost: bool,
    reading: bool,
}

struct Core {
    dev: Option<Device>,
    status: u32,
    reason: &'static str,
    /// Each profile capability the device has, indexed by capability id.
    have: [bool; capability::LAST as usize + 1],
    caps: abi::Caps,
    procs: BTreeMap<u64, Proc>,
    contexts: BTreeMap<u32, Ctx>,
    resources: BTreeMap<u32, Res>,
    subs: BTreeMap<u64, Sub>,
    done: BTreeMap<u64, Done>,
    abandoned: BTreeSet<u64>,
    retire: BTreeMap<u64, Arc<Pages>>,
    next_ctx: u32,
    next_res: u32,
    next_serial: u64,
    next_fence: u64,
    next_ns: u32,
    next_obs: u64,
    key: u64,
    stalled: bool,
    quarantined_at: Option<u64>,
    last_progress: u64,
    reset_wanted: bool,
}

// ---------------------------------------------------------------------------------------
// Initialisation and the profile.

/// Find and initialise the GPU on QEMU's PCI bus, read its profile, and route its
/// interrupt. Called once from driver initialisation; an absent or failing device leaves
/// a core that refuses every op naming why.
pub fn init_qemu() {
    let mut core = Core::empty();
    match Device::probe() {
        Ok(dev) => {
            core.profile(&dev);
            crate::serial_println!(
                "[gpu] virtio-gpu-gl: features={:#x} capsets={:?} chosen={:?} scanouts={} status={} {}",
                dev.info.features,
                dev.info.capset_ids,
                dev.info.capset.as_ref().map(|c| (c.0, c.1, c.2.len())),
                dev.info.num_scanouts,
                core.status,
                core.reason
            );
            if let Some((_, _, bytes)) = dev.info.capset.as_ref() {
                let rd = |o: usize| bytes.get(o..o + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
                crate::serial_println!(
                    "[gpu] capset glsl={} caps={:#x} caps2={:#x} max2d={} renderer={:?}",
                    rd(wire::caps::GLSL_LEVEL),
                    rd(wire::caps::CAPABILITY_BITS),
                    rd(688),
                    rd(wire::caps::MAX_TEXTURE_2D_SIZE),
                    core::str::from_utf8(&core.caps.renderer).unwrap_or("").trim_end_matches('\0')
                );
            }
            ISR.store(dev.isr, Ordering::Release);
            INTID.store(dev.intid, Ordering::Release);
            crate::arch_impl::aarch64::gic::configure_spi_level_triggered(dev.intid);
            crate::arch_impl::aarch64::gic::enable_spi(dev.intid);
            core.dev = Some(dev);
        }
        Err(e) => {
            core.status = abi::status::NO_DEVICE;
            core.reason = e;
            crate::serial_println!("[gpu] no GPU core device: {}", e);
        }
    }
    let counter: u64;
    // SAFETY: the virtual counter is readable at EL1.
    unsafe { core::arch::asm!("mrs {}, cntvct_el0", out(reg) counter, options(nomem, nostack)) };
    core.key = counter ^ 0x9E37_79B9_7F4A_7C15;
    *CORE.lock() = Some(core);
}

impl Core {
    fn empty() -> Core {
        let mut caps = abi::Caps::ZERO;
        caps.version = 1;
        caps.size = core::mem::size_of::<abi::Caps>() as u32;
        caps.catalogue_version = abi::CATALOGUE_VERSION;
        Core {
            dev: None,
            status: abi::status::NO_DEVICE,
            reason: "no virtio-gpu device",
            have: [false; capability::LAST as usize + 1],
            caps,
            procs: BTreeMap::new(),
            contexts: BTreeMap::new(),
            resources: BTreeMap::new(),
            subs: BTreeMap::new(),
            done: BTreeMap::new(),
            abandoned: BTreeSet::new(),
            retire: BTreeMap::new(),
            next_ctx: 1,
            next_res: 0x100,
            next_serial: 1,
            next_fence: 1,
            next_ns: 1,
            next_obs: 1,
            key: 0,
            stalled: false,
            quarantined_at: None,
            last_progress: 0,
            reset_wanted: false,
        }
    }

    /// Read the profile from the device's capability set (`docs/gui/gpu.md` section 1.5,
    /// item 8) into the CAPS record and the capability table.
    fn profile(&mut self, dev: &Device) {
        let c = &mut self.caps;
        c.backend = abi::backend::VIRGL;
        if dev.info.features & wire::F_VIRGL != 0 {
            c.features |= abi::feature::VIRGL;
        }
        if dev.info.features & wire::F_VERSION_1 != 0 {
            c.features |= abi::feature::VERSION_1;
        }
        c.capset_count = dev.info.capset_ids.len() as u32;
        for (slot, id) in c.capset_ids.iter_mut().zip(dev.info.capset_ids.iter()) {
            *slot = *id;
        }
        c.scanout_count = dev.info.num_scanouts;
        for (slot, s) in c.scanouts.iter_mut().zip(dev.info.scanouts.iter()) {
            *slot = abi::ScanoutInfo { enabled: s.enabled as u32, width: s.width, height: s.height, reserved: 0 };
        }
        c.pipelines = 1 << abi::pipeline::SOLID;
        self.have[capability::FEATURE_3D as usize] = c.features & abi::feature::VIRGL != 0;
        let empty = Vec::new();
        let bytes = dev.info.capset.as_ref().map_or(&empty, |s| &s.2);
        let rd = |o: usize| bytes.get(o..o + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        let bit = |mask: usize, f: u32| rd(mask + 4 * (f as usize / 32)) >> (f % 32) & 1 == 1;
        if let Some((id, ver, b)) = dev.info.capset.as_ref() {
            c.capset_id = *id;
            c.capset_version = *ver;
            self.have[capability::CAPABILITY_SET as usize] = b.len() >= wire::caps::V1_SIZE;
        }
        let formats = [
            (format::B8G8R8A8, wire::format::B8G8R8A8_UNORM),
            (format::R8G8B8A8, wire::format::R8G8B8A8_UNORM),
            (format::R8, wire::format::R8_UNORM),
        ];
        for (i, (bgl, vf)) in formats.iter().enumerate() {
            let mut flags = 0;
            if bit(wire::caps::SAMPLER_MASK, *vf) {
                flags |= abi::format_flag::SAMPLE;
            }
            if bit(wire::caps::RENDER_MASK, *vf) {
                flags |= abi::format_flag::RENDER;
            }
            c.formats[i] = abi::FormatCaps { format: *bgl, flags };
        }
        let f = |i: usize, flag: u32| c.formats[i].flags & flag != 0;
        self.have[capability::BGRA_RENDER as usize] = f(0, abi::format_flag::RENDER);
        self.have[capability::BGRA_SAMPLE as usize] = f(0, abi::format_flag::SAMPLE);
        self.have[capability::RGBA_RENDER as usize] = f(1, abi::format_flag::RENDER);
        self.have[capability::RGBA_SAMPLE as usize] = f(1, abi::format_flag::SAMPLE);
        self.have[capability::R8_SAMPLE as usize] = f(2, abi::format_flag::SAMPLE);
        // The set's shading-language level is the level the device offers its guest (a
        // GLES-backed host reports it on the same scale). Integer types and operations and
        // integer-addressed texel fetch arrive with GLSL 1.30 (the Khronos OpenGL Shading
        // Language 1.30 specification, sections 4.1.3 and 8.7).
        let glsl = rd(wire::caps::GLSL_LEVEL);
        let integer = glsl >= 130;
        c.shading_level = glsl;
        if integer {
            c.profile |= abi::profile::INTEGER_OPS | abi::profile::TEXEL_FETCH;
        }
        // RESOURCE_COPY_REGION is a base command of the VirGL protocol, which every VirGL
        // capability set describes; no capability bit gates it.
        if self.have[capability::CAPABILITY_SET as usize] {
            c.profile |= abi::profile::REGION_COPY;
        }
        self.have[capability::INTEGER_OPS as usize] = integer;
        self.have[capability::TEXEL_FETCH as usize] = integer;
        self.have[capability::REGION_COPY as usize] = c.profile & abi::profile::REGION_COPY != 0;
        let max2d = if bytes.len() >= wire::caps::MAX_TEXTURE_2D_SIZE + 4 { rd(wire::caps::MAX_TEXTURE_2D_SIZE) } else { 0 };
        let side = max2d.min(limits::SIDE);
        c.max_width = side;
        c.max_height = side;
        self.have[capability::TARGET_SIZE as usize] = side >= 2560;
        if bytes.len() >= wire::caps::RENDERER + wire::caps::RENDERER_LEN {
            c.renderer.copy_from_slice(&bytes[wire::caps::RENDERER..wire::caps::RENDERER + wire::caps::RENDERER_LEN]);
            // Keep the record's string NUL-padded after its first NUL.
            if let Some(nul) = c.renderer.iter().position(|&b| b == 0) {
                c.renderer[nul..].fill(0);
            }
        }
        self.status = abi::status::ADMITTED;
        self.reason = "";
        if let Some(missing) = self.first_missing(None) {
            self.status = abi::status::MISSING_CAPABILITY;
            self.reason = capability::name(missing);
        }
    }

    /// The first profile capability the device lacks, with `masked` treated as absent.
    fn first_missing(&self, masked: Option<u32>) -> Option<u32> {
        (1..=capability::LAST).find(|&id| !self.have[id as usize] || masked == Some(id))
    }

    /// Whether ops may run, and if not, the status and the missing capability. While test
    /// point 23 (`mask-capability`) is armed, the capability its parameter names counts
    /// as absent, so admission fails naming it (`device/missing-capability-fails`).
    fn admission(&self) -> Result<(), (u32, u32)> {
        if self.dev.is_none() {
            return Err((if self.status == abi::status::ADMITTED { abi::status::DEVICE_FAILED } else { self.status }, 0));
        }
        let masked = crate::test_control::peek(test_point::MASK_CAPABILITY)
            .map(|p| p as u32)
            .filter(|&c| (1..=capability::LAST).contains(&c));
        match self.first_missing(masked) {
            Some(missing) => {
                if masked.is_some() {
                    crate::test_control::count_fired(test_point::MASK_CAPABILITY);
                }
                Err((abi::status::MISSING_CAPABILITY, missing))
            }
            None => Ok(()),
        }
    }

    fn caps_record(&self) -> abi::Caps {
        let mut c = self.caps;
        c.status = abi::status::ADMITTED;
        c.missing = capability::NONE;
        c.reason = [0; 64];
        let text: &[u8] = match self.admission() {
            Ok(()) => b"",
            Err((status, missing)) => {
                c.status = status;
                c.missing = missing;
                if status == abi::status::MISSING_CAPABILITY {
                    let name = capability::name(missing).as_bytes();
                    let n = name.len().min(55);
                    c.reason[..8].copy_from_slice(b"missing ");
                    c.reason[8..8 + n].copy_from_slice(&name[..n]);
                    &[]
                } else {
                    self.reason.as_bytes()
                }
            }
        };
        if !text.is_empty() {
            let n = text.len().min(63);
            c.reason[..n].copy_from_slice(&text[..n]);
        }
        c
    }

    // -----------------------------------------------------------------------------------
    // Identity.

    fn serial(&mut self) -> u64 {
        let s = self.next_serial;
        self.next_serial += 1;
        s
    }

    fn device_fence(&mut self) -> u64 {
        let f = self.next_fence;
        self.next_fence += 1;
        f
    }

    /// A new handle for process `tgid`, unique among its handles and never 0.
    fn handle(&mut self, tgid: u64) -> u64 {
        let key = self.key;
        let p = self.proc_mut(tgid);
        loop {
            p.seq += 1;
            let mut z = key ^ tgid.rotate_left(29) ^ p.seq.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            if z != 0 && !p.contexts.contains_key(&z) && !p.resources.contains_key(&z) {
                return z;
            }
        }
    }

    fn proc_mut(&mut self, tgid: u64) -> &mut Proc {
        self.procs.entry(tgid).or_insert_with(|| Proc {
            contexts: BTreeMap::new(),
            resources: BTreeMap::new(),
            totals: Totals::default(),
            seq: 0,
            bytes: 0,
        })
    }

    fn ctx_of(&self, tgid: u64, handle: u64) -> Option<u32> {
        self.procs.get(&tgid)?.contexts.get(&handle).copied()
    }

    fn res_of(&self, tgid: u64, handle: u64) -> Option<u32> {
        self.procs.get(&tgid)?.resources.get(&handle).copied()
    }

    // -----------------------------------------------------------------------------------
    // The device.

    /// Hand a request to the device, or give it back when the queue has no room.
    fn send(&mut self, req: Request) -> Result<(), Request> {
        match self.dev.as_mut() {
            Some(dev) => dev.send(req),
            None => Err(req),
        }
    }

    /// Take every completion the device has returned and the watchdog's decisions.
    fn pump(&mut self) {
        self.reap_all();
        self.watchdog(now_ns());
    }

    fn reap_all(&mut self) {
        if self.stalled {
            return;
        }
        let mut any = false;
        loop {
            let Some(d) = self.dev.as_mut().and_then(|dev| dev.reap()) else { break };
            any = true;
            self.last_progress = now_ns();
            match d.tag {
                Tag::Sync(serial) => {
                    if !self.abandoned.remove(&serial) {
                        self.done.insert(serial, d);
                    }
                }
                Tag::Retire(serial) => {
                    self.retire.remove(&serial);
                }
                Tag::Background => {}
                Tag::Submit { serial, last, .. } => {
                    self.submit_returned(serial, last, d);
                    if self.stalled {
                        break;
                    }
                }
            }
        }
        if any {
            notify();
        }
    }

    /// One SUBMIT_3D of submission `serial` came back.
    fn submit_returned(&mut self, serial: u64, last: bool, d: Done) {
        let Some(sub) = self.subs.get_mut(&serial) else { return };
        if !virgl::ok(d.answer) {
            sub.error.get_or_insert(d.answer);
        }
        if !last {
            return;
        }
        let mut echoed = d.echoed;
        // Test point 20, `stale-fence`: the answer claims another submission's fence.
        if crate::test_control::take(test_point::STALE_FENCE, 0).is_some() {
            echoed = sub.fence + 1;
        }
        if sub.error.is_none() && echoed != sub.fence {
            sub.mismatch = true;
        }
        // Test point 31, `withhold-fence`: ignore this completion, or every completion
        // from now until the device is reset.
        if let Some(param) = crate::test_control::take(test_point::WITHHOLD_FENCE, 0) {
            if param == test_point::WITHHOLD_ALL {
                self.stalled = true;
            }
            return;
        }
        let (status, answer) = match (sub.error, sub.mismatch) {
            (Some(a), _) => (fence_status::DEVICE_ERROR, a),
            (None, true) => (fence_status::FENCE_MISMATCH, echoed as u32),
            (None, false) => (fence_status::OK, d.answer),
        };
        self.finish_sub(serial, status, answer);
    }

    /// Complete submission `serial` with `status`: its fence, its targets and its holds.
    fn finish_sub(&mut self, serial: u64, status: u32, answer: u32) {
        let Some(sub) = self.subs.remove(&serial) else { return };
        let ok = status == fence_status::OK;
        let mut reset = false;
        if let Some(ctx) = self.contexts.get_mut(&sub.ctx) {
            if !ok {
                ctx.outcomes.insert(sub.n, (status, answer));
                let floor = ctx.done_upto.saturating_sub(4096);
                while ctx.outcomes.first_key_value().is_some_and(|(&k, _)| k < floor) {
                    ctx.outcomes.pop_first();
                }
            }
            ctx.early.insert(sub.n);
            while ctx.early.remove(&(ctx.done_upto + 1)) {
                ctx.done_upto += 1;
            }
            ctx.inflight.retain(|&s| s != serial);
            if status == fence_status::DEVICE_ERROR {
                ctx.totals.device_errors += 1;
                ctx.errors_in_row += 1;
                // A second device error in a row escalates to a device reset.
                reset = ctx.errors_in_row >= 2;
                if let Some(p) = self.procs.get_mut(&ctx.owner) {
                    p.totals.device_errors += 1;
                }
            } else if ok {
                ctx.errors_in_row = 0;
            }
        }
        if reset {
            self.reset_wanted = true;
        }
        for id in &sub.written {
            if let Some(r) = self.resources.get_mut(id) {
                r.writers = r.writers.saturating_sub(1);
                if ok {
                    r.generation += 1;
                    r.invalid = false;
                    r.last = Some(LastWrite { ctx: sub.ctx, fence: sub.fence, serial, answer });
                } else {
                    r.invalid = true;
                }
            }
        }
        for id in sub.held {
            if let Some(r) = self.resources.get_mut(&id) {
                r.holds = r.holds.saturating_sub(1);
            }
            self.free_if_unused(id);
        }
        self.finalize_if_dropped(sub.ctx);
    }

    /// Free a resource whose handle is gone and that nothing holds.
    fn free_if_unused(&mut self, id: u32) {
        let Some(r) = self.resources.get(&id) else { return };
        if !r.dropped || r.holds > 0 || r.writers > 0 || r.reading {
            return;
        }
        let Some(r) = self.resources.remove(&id) else { return };
        if let Some(p) = self.procs.get_mut(&r.owner) {
            p.bytes = p.bytes.saturating_sub(r.backing.bytes() as u64);
        }
        if !r.lost {
            // The device may use the backing until it answers the unref.
            let serial = self.serial();
            self.retire.insert(serial, r.backing.clone());
            if let Err(req) = self.send(virgl::resource_unref(Tag::Retire(serial), id)) {
                drop(req);
                self.retire.remove(&serial);
            }
        }
    }

    /// Destroy a dropped context once its work has completed.
    fn finalize_if_dropped(&mut self, id: u32) {
        let Some(c) = self.contexts.get(&id) else { return };
        if !c.dropped || !c.inflight.is_empty() || c.busy || c.state == CtxState::Creating {
            return;
        }
        let Some(c) = self.contexts.remove(&id) else { return };
        if c.device_alive {
            let _ = self.send(virgl::ctx_destroy(Tag::Background, c.id));
        }
        for internal in [c.vb, c.scratch] {
            if let Some(r) = self.resources.get_mut(&internal) {
                r.dropped = true;
            }
            self.free_if_unused(internal);
        }
    }

    /// The watchdog (section 1.4.1): quarantine a context whose submission is 2 s old,
    /// and reset a device that then makes no progress for 2 s.
    fn watchdog(&mut self, now: u64) {
        if self.dev.is_none() {
            return;
        }
        // A device that has made no progress for 2 s since a quarantine is reset first,
        // so the work it holds completes as reset rather than being found hung.
        if let Some(at) = self.quarantined_at {
            let busy = self.stalled || self.dev.as_ref().is_some_and(|d| d.outstanding() > 0);
            if !busy {
                self.quarantined_at = None;
            } else if now.saturating_sub(at.max(self.last_progress)) >= limits::WATCHDOG_NS {
                self.reset_wanted = true;
            }
        }
        if self.reset_wanted {
            self.reset(now);
            return;
        }
        let hung: Vec<u32> = self
            .subs
            .values()
            .filter(|s| s.sent_ns != 0 && now.saturating_sub(s.sent_ns) >= limits::WATCHDOG_NS)
            .map(|s| s.ctx)
            .collect();
        for ctx in hung {
            self.quarantine(ctx, now);
        }
        if self.reset_wanted {
            self.reset(now);
        }
    }

    /// Quarantine context `id`: its pending fences fail with `DEVICE_HUNG`, its targets
    /// become invalid, its device context is destroyed and further submissions refused.
    fn quarantine(&mut self, id: u32, now: u64) {
        let Some(c) = self.contexts.get_mut(&id) else { return };
        if c.state == CtxState::Live {
            c.state = CtxState::Quarantined;
        }
        let pending: Vec<u64> = c.inflight.iter().copied().collect();
        let alive = core::mem::replace(&mut c.device_alive, false);
        for serial in pending {
            self.finish_sub(serial, fence_status::DEVICE_HUNG, 0);
        }
        if alive {
            let _ = self.send(virgl::ctx_destroy(Tag::Background, id));
        }
        // No progress is measured from the first quarantine.
        self.quarantined_at.get_or_insert(now);
        notify();
    }

    /// Reset the device: every outstanding request ends, every context fails with
    /// `DEVICE_RESET`, every resource is lost, and the device comes back empty.
    fn reset(&mut self, now: u64) {
        self.reset_wanted = false;
        self.quarantined_at = None;
        self.stalled = false;
        let tags = match self.dev.as_mut().map(|d| d.reset()) {
            Some(Ok(tags)) => tags,
            _ => {
                self.dev = None;
                self.status = abi::status::DEVICE_FAILED;
                self.reason = "virtio-gpu reset failed";
                Vec::new()
            }
        };
        for tag in tags {
            if let Tag::Sync(serial) = tag {
                if !self.abandoned.remove(&serial) {
                    self.done.insert(serial, Done { tag, fence: None, answer: 0, echoed: 0, resp: dma::DmaBox::new(wire::HDR) });
                }
            }
        }
        self.retire.clear();
        let serials: Vec<u64> = self.subs.keys().copied().collect();
        for serial in serials {
            self.finish_sub(serial, fence_status::DEVICE_RESET, 0);
        }
        for c in self.contexts.values_mut() {
            c.state = CtxState::Reset;
            c.device_alive = false;
        }
        let ids: Vec<u32> = self.resources.keys().copied().collect();
        for id in ids {
            if let Some(r) = self.resources.get_mut(&id) {
                r.lost = true;
            }
            self.free_if_unused(id);
        }
        let ctxs: Vec<u32> = self.contexts.keys().copied().collect();
        for id in ctxs {
            self.finalize_if_dropped(id);
        }
        self.last_progress = now;
        notify();
    }

    /// The earliest time the watchdog has something to decide.
    fn next_deadline(&self) -> Option<u64> {
        let hang = self.subs.values().filter(|s| s.sent_ns != 0).map(|s| s.sent_ns + limits::WATCHDOG_NS).min();
        let reset = self.quarantined_at.map(|at| at.max(self.last_progress) + limits::WATCHDOG_NS);
        match (hang, reset) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    // -----------------------------------------------------------------------------------
    // Process reclamation.

    /// Release everything process `tgid` holds: contexts are destroyed once their work
    /// has completed, resources are freed once nothing holds them.
    fn reclaim(&mut self, tgid: u64) {
        let Some(p) = self.procs.remove(&tgid) else { return };
        for (_, id) in p.contexts {
            if let Some(c) = self.contexts.get_mut(&id) {
                c.dropped = true;
            }
            self.finalize_if_dropped(id);
        }
        for (_, id) in p.resources {
            if let Some(r) = self.resources.get_mut(&id) {
                r.dropped = true;
            }
            self.free_if_unused(id);
        }
    }
}

// ---------------------------------------------------------------------------------------
// Waiting.

enum Stop {
    Timeout,
    Interrupted,
}

/// Wait until `f` (run under the core's lock after taking completions) returns a value,
/// the absolute `deadline` passes, or a signal arrives.
fn wait_for<T>(deadline: Option<u64>, interruptible: bool, mut f: impl FnMut(&mut Core) -> Option<T>) -> Result<T, Stop> {
    loop {
        let seen = EVENTS.load(Ordering::Acquire);
        if let Some(v) = with_core(|c| {
            c.pump();
            f(c)
        })
        .flatten()
        {
            return Ok(v);
        }
        if deadline.is_some_and(|d| now_ns() >= d) {
            return Err(Stop::Timeout);
        }
        if interruptible && crate::syscall::check_signals_for_wait().is_some() {
            return Err(Stop::Interrupted);
        }
        let outcome = WAITERS.prepare_to_wait_checked(ThreadState::BlockedOnIO, deadline, || {
            EVENTS.load(Ordering::Acquire) == seen
        });
        if crate::syscall::blocking_io::wait_prepared(&WAITERS, outcome).is_err() && interruptible {
            return Err(Stop::Interrupted);
        }
    }
}

/// Hand `req` to the device, waiting while its queue has no room. Calls `sent` under
/// the lock once it is in the device's hands.
fn send_blocking(mut req: Request, mut sent: impl FnMut(&mut Core)) -> Result<(), i64> {
    loop {
        let segments = req.segment_count();
        let back = with_core(|c| {
            if c.dev.is_none() {
                return Some(Err(()));
            }
            match c.send(req) {
                Ok(()) => {
                    sent(c);
                    None
                }
                Err(r) => Some(Ok(r)),
            }
        })
        .unwrap_or(Some(Err(())));
        match back {
            None => return Ok(()),
            Some(Err(())) => return Err(EIO),
            Some(Ok(r)) => req = r,
        }
        // The device returns requests as it finishes them; the wait ends when it has.
        let _ = wait_for(Some(now_ns() + 10_000_000), false, |c| {
            c.dev.as_ref().map(|d| d.has_room(segments)).filter(|&room| room)
        });
    }
}

/// Send a synchronous request (built with `Tag::Sync(serial)`) and wait for its answer.
/// A kill abandons the wait; the request's answer is then dropped when it comes.
fn exec(serial: u64, req: Request, deadline: Option<u64>) -> Result<Done, i64> {
    send_blocking(req, |_| {})?;
    match wait_for(deadline, true, |c| c.done.remove(&serial)) {
        Ok(d) => Ok(d),
        Err(stop) => {
            with_core(|c| {
                if c.done.remove(&serial).is_none() {
                    c.abandoned.insert(serial);
                }
            });
            Err(match stop {
                Stop::Timeout => EIO,
                Stop::Interrupted => EINTR,
            })
        }
    }
}

/// Build and run one synchronous command, requiring a success answer.
fn exec_ok(build: impl FnOnce(Tag) -> Request) -> Result<Done, i64> {
    exec_opt(|t| Some(build(t)))
}

/// `exec_ok` for a command whose building can run out of memory.
fn exec_opt(build: impl FnOnce(Tag) -> Option<Request>) -> Result<Done, i64> {
    let serial = with_core(|c| c.serial()).ok_or(ENODEV)?;
    let req = build(Tag::Sync(serial)).ok_or(ENOMEM)?;
    let d = exec(serial, req, None)?;
    if virgl::ok(d.answer) {
        Ok(d)
    } else {
        Err(EIO)
    }
}

// ---------------------------------------------------------------------------------------
// The interrupt and the reaper thread.

/// The GIC interrupt id of the GPU's INTx line, once the device is up.
pub fn irq() -> Option<u32> {
    match INTID.load(Ordering::Acquire) {
        0 => None,
        id => Some(id),
    }
}

/// The GPU interrupt: acknowledge it through the ISR byte, count it and wake whoever
/// waits. No lock, no allocation, no output.
pub fn handle_interrupt() {
    let isr = ISR.load(Ordering::Acquire);
    if isr == 0 || virgl::read_isr(isr) == 0 {
        return;
    }
    EVENTS.fetch_add(1, Ordering::AcqRel);
    WAITERS.wake_up();
    let tid = REAPER.load(Ordering::Acquire);
    if tid != 0 {
        crate::task::scheduler::wake_thread_any_context(tid);
    }
}

fn ensure_reaper() {
    if REAPER.load(Ordering::Acquire) != 0 {
        return;
    }
    if REAPER_STARTING.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
        if let Ok(handle) = crate::task::kthread::kthread_run(reaper, "kgpu") {
            REAPER.store(handle.tid(), Ordering::Release);
        }
        REAPER_STARTING.store(false, Ordering::Release);
    }
}

/// `kgpu`: takes completions nobody is waiting for, runs the watchdog at its deadlines
/// and reclaims what exited processes left, once a second while any process holds GPU
/// state.
fn reaper() {
    let Some(tid) = crate::task::scheduler::current_thread_id() else { return };
    let mut next_sweep = now_ns();
    loop {
        let seen = EVENTS.load(Ordering::Acquire);
        let (deadline, busy) = with_core(|c| {
            c.pump();
            (c.next_deadline(), !c.procs.is_empty())
        })
        .unwrap_or((None, false));
        let now = now_ns();
        if busy && now >= next_sweep {
            sweep();
            next_sweep = now + 1_000_000_000;
        }
        notify();
        let wake = match (deadline, busy) {
            (Some(d), true) => Some(d.min(next_sweep)),
            (Some(d), false) => Some(d),
            (None, true) => Some(next_sweep),
            (None, false) => None,
        };
        crate::arch_without_interrupts(|| {
            crate::task::scheduler::with_scheduler(|sched| match wake {
                Some(at) => sched.block_current_for_timer(at),
                None => sched.block_current(),
            });
            if EVENTS.load(Ordering::Acquire) != seen {
                crate::task::scheduler::with_scheduler(|sched| {
                    sched.unblock(tid);
                });
            }
        });
        crate::task::scheduler::yield_current();
        crate::arch_halt_with_interrupts();
    }
}

/// Reclaim the GPU state of processes that have exited (PIDs are never reused).
fn sweep() {
    let tgids: Vec<u64> = with_core(|c| c.procs.keys().copied().collect()).unwrap_or_default();
    let dead: Vec<u64> = tgids
        .into_iter()
        .filter(|&tgid| {
            !crate::process::with_process_manager(|m| {
                m.get_process(crate::process::ProcessId::new(tgid)).is_some_and(|p| !p.is_terminated())
            })
            .unwrap_or(true)
        })
        .collect();
    if !dead.is_empty() {
        with_core(|c| {
            for tgid in dead {
                c.reclaim(tgid);
            }
        });
    }
}

// ---------------------------------------------------------------------------------------
// The system call.

fn caller() -> Result<u64, i64> {
    let tid = crate::task::scheduler::current_thread_id().ok_or(EINVAL)?;
    crate::process::with_process_manager(|m| {
        m.find_process_by_thread(tid).map(|(row, p)| p.thread_group_id.unwrap_or(row.as_u64()))
    })
    .flatten()
    .ok_or(EINVAL)
}

fn read_arg<T: Plain>(arg: u64, len: u64) -> Result<T, i64> {
    if len != core::mem::size_of::<T>() as u64 {
        return Err(EINVAL);
    }
    crate::syscall::userptr::copy_from_user(arg as *const T).map_err(|_| EFAULT)
}

fn write_arg<T: Plain>(arg: u64, value: &T) -> Result<(), i64> {
    crate::syscall::userptr::copy_to_user(arg as *mut T, value).map_err(|_| EFAULT)
}

fn result(r: Result<u64, i64>) -> SyscallResult {
    match r {
        Ok(v) => SyscallResult::Ok(v),
        Err(e) => SyscallResult::Err(e as u64),
    }
}

/// `gpu(op, arg, len)`.
pub fn sys_gpu(op: u64, arg: u64, len: u64) -> SyscallResult {
    let Ok(op) = u32::try_from(op) else { return result(Err(EINVAL)) };
    result(match op {
        abi::op::VERSION => {
            if len != 0 {
                Err(EINVAL)
            } else {
                Ok(abi::INTERFACE_VERSION as u64)
            }
        }
        abi::op::CAPS => op_caps(arg, len),
        o if o > abi::op::LAST => Err(EINVAL),
        o => match with_core(|c| c.admission()) {
            None => Err(ENODEV),
            Some(Err(_)) => Err(ENODEV),
            Some(Ok(())) => match o {
                abi::op::CONTEXT_CREATE => op_context_create(arg, len),
                abi::op::CONTEXT_DESTROY => op_context_destroy(arg, len),
                abi::op::RESOURCE_CREATE => op_resource_create(arg, len),
                abi::op::RESOURCE_DESTROY => op_resource_destroy(arg, len),
                abi::op::SUBMIT => op_submit(arg, len),
                abi::op::WAIT => op_wait(arg, len),
                abi::op::READBACK => op_readback(arg, len),
                abi::op::COUNTERS => op_counters(arg, len),
                // UPLOAD (G3), MODES, SET_MODE and PRESENT (G2, G7) are not built yet.
                _ => Err(ENOSYS),
            },
        },
    })
}

fn op_caps(arg: u64, len: u64) -> Result<u64, i64> {
    if len != core::mem::size_of::<abi::Caps>() as u64 {
        return Err(EINVAL);
    }
    let caps = with_core(|c| c.caps_record()).unwrap_or_else(|| Core::empty().caps_record());
    write_arg(arg, &caps)?;
    Ok(0)
}

fn op_context_create(arg: u64, len: u64) -> Result<u64, i64> {
    let mut a: abi::ContextCreate = read_arg(arg, len)?;
    if a.flags != 0 || a.handle != 0 || a.warmed != 0 || a.reserved != [0; 2] {
        return Err(EINVAL);
    }
    let tgid = caller()?;
    sweep();
    ensure_reaper();
    // Reserve the context and its internal resources.
    let vb_pages = Arc::new(Pages::new(encode::VERTEX_BUFFER_BYTES as usize / PAGE).ok_or(ENOMEM)?);
    let scratch_pages = Arc::new(Pages::new(1).ok_or(ENOMEM)?);
    let (id, vb, scratch) = with_core(|c| {
        let p = c.proc_mut(tgid);
        if p.contexts.len() >= limits::CONTEXTS {
            return Err(EAGAIN);
        }
        let id = c.next_ctx;
        c.next_ctx += 1;
        let ns = c.next_ns;
        c.next_ns += 1;
        let vb = c.next_res;
        let scratch = c.next_res + 1;
        c.next_res += 2;
        for (rid, kind_, pages, w, h, size) in [
            (vb, kind::BUFFER, vb_pages.clone(), 0, 0, encode::VERTEX_BUFFER_BYTES as u64),
            (scratch, kind::TARGET, scratch_pages.clone(), 1, 1, 0),
        ] {
            c.resources.insert(rid, Res {
                owner: 0,
                kind: kind_,
                format: format::B8G8R8A8,
                width: w,
                height: h,
                size,
                pitch: 256,
                backing: pages,
                generation: 0,
                last: None,
                writers: 0,
                invalid: false,
                dropped: false,
                holds: 0,
                lost: false,
                reading: false,
            });
        }
        c.contexts.insert(id, Ctx {
            id,
            owner: tgid,
            ns,
            state: CtxState::Creating,
            dropped: false,
            busy: false,
            device_alive: false,
            issued: 0,
            done_upto: 0,
            early: BTreeSet::new(),
            outcomes: BTreeMap::new(),
            inflight: VecDeque::new(),
            totals: Totals::default(),
            objects: encode::Objects::new(),
            vb,
            scratch,
            warmed: 0,
            errors_in_row: 0,
        });
        Ok((id, vb, scratch))
    })
    .ok_or(ENODEV)??;
    match create_device_context(tgid, id, vb, scratch, &vb_pages, &scratch_pages) {
        Ok(()) => {}
        Err(e) => {
            with_core(|c| {
                if let Some(ctx) = c.contexts.get_mut(&id) {
                    ctx.state = CtxState::Live;
                    ctx.dropped = true;
                }
                c.finalize_if_dropped(id);
            });
            return Err(e);
        }
    }
    let handle = with_core(|c| {
        let h = c.handle(tgid);
        c.proc_mut(tgid).contexts.insert(h, id);
        if let Some(ctx) = c.contexts.get_mut(&id) {
            ctx.state = CtxState::Live;
            ctx.warmed = encode::CATALOGUE_PROGRAMS;
        }
        h
    })
    .ok_or(ENODEV)?;
    a.handle = handle;
    a.warmed = encode::CATALOGUE_PROGRAMS;
    if let Err(e) = write_arg(arg, &a) {
        destroy_context(tgid, handle);
        return Err(e);
    }
    Ok(0)
}

/// Create device context `id` with its internal resources and the process's resources
/// attached, then compile and warm the catalogue's programs, waiting for the warm-up's
/// fence under the watchdog's 2 s.
fn create_device_context(tgid: u64, id: u32, vb: u32, scratch: u32, vb_pages: &Pages, scratch_pages: &Pages) -> Result<(), i64> {
    exec_ok(|t| virgl::ctx_create(t, id))?;
    with_core(|c| {
        if let Some(ctx) = c.contexts.get_mut(&id) {
            ctx.device_alive = true;
        }
    });
    exec_ok(|t| {
        virgl::resource_create(t, vb, wire::target::BUFFER, wire::format::R8_UNORM, wire::bind::VERTEX_BUFFER, encode::VERTEX_BUFFER_BYTES, 1)
    })?;
    exec_opt(|t| virgl::attach_backing(t, vb, vb_pages))?;
    exec_ok(|t| virgl::ctx_attach(t, id, vb))?;
    exec_ok(|t| {
        virgl::resource_create(
            t,
            scratch,
            wire::target::TEXTURE_2D,
            wire::format::B8G8R8A8_UNORM,
            wire::bind::RENDER_TARGET | wire::bind::SAMPLER_VIEW,
            1,
            1,
        )
    })?;
    exec_opt(|t| virgl::attach_backing(t, scratch, scratch_pages))?;
    exec_ok(|t| virgl::ctx_attach(t, id, scratch))?;
    let theirs: Vec<u32> = with_core(|c| c.procs.get(&tgid).map(|p| p.resources.values().copied().collect()).unwrap_or_default())
        .unwrap_or_default();
    for rid in theirs {
        exec_ok(|t| virgl::ctx_attach(t, id, rid))?;
    }
    // The warm-up: a fenced submission, waited for no longer than the watchdog allows.
    let (serial, fence, req) = with_core(|c| {
        let mut enc = encode::Enc::new();
        let ctx = c.contexts.get_mut(&id)?;
        encode::warm_up(&mut enc, &mut ctx.objects, vb, scratch, wire::format::B8G8R8A8_UNORM, FLIP_Y);
        let mut chunks = enc.finish()?;
        if chunks.len() != 1 {
            return None;
        }
        let serial = c.serial();
        let fence = c.device_fence();
        Some((serial, fence, virgl::submit(Tag::Sync(serial), id, Some(fence), chunks.remove(0))))
    })
    .flatten()
    .ok_or(ENOMEM)?;
    let d = exec(serial, req, Some(now_ns() + limits::WATCHDOG_NS))?;
    if !virgl::ok(d.answer) || d.echoed != fence {
        return Err(EIO);
    }
    Ok(())
}

fn op_context_destroy(arg: u64, len: u64) -> Result<u64, i64> {
    let a: abi::Handle = read_arg(arg, len)?;
    if a.reserved != 0 {
        return Err(EINVAL);
    }
    let tgid = caller()?;
    if destroy_context(tgid, a.handle) {
        Ok(0)
    } else {
        Err(EINVAL)
    }
}

/// Drop a context handle; the device context goes once its submitted work completes.
fn destroy_context(tgid: u64, handle: u64) -> bool {
    with_core(|c| {
        let Some(id) = c.procs.get_mut(&tgid).and_then(|p| p.contexts.remove(&handle)) else { return false };
        if let Some(ctx) = c.contexts.get_mut(&id) {
            ctx.dropped = true;
        }
        c.finalize_if_dropped(id);
        true
    })
    .unwrap_or(false)
}

fn op_resource_create(arg: u64, len: u64) -> Result<u64, i64> {
    let mut a: abi::ResourceCreate = read_arg(arg, len)?;
    if a.handle != 0 || a.reserved != [0; 2] {
        return Err(EINVAL);
    }
    let tgid = caller()?;
    let max = with_core(|c| c.caps.max_width).unwrap_or(0);
    let (vtarget, vformat, bind, bytes, pitch) = match a.kind {
        kind::TARGET | kind::TEXTURE => {
            let vformat = match (a.kind, a.format) {
                (_, format::B8G8R8A8) => wire::format::B8G8R8A8_UNORM,
                (_, format::R8G8B8A8) => wire::format::R8G8B8A8_UNORM,
                (kind::TEXTURE, format::R8) => wire::format::R8_UNORM,
                _ => return Err(EINVAL),
            };
            if a.width == 0 || a.height == 0 || a.width > max || a.height > max || a.size != 0 {
                return Err(EINVAL);
            }
            // Transfers use rows aligned to 256 bytes (section 1.3, backing and row pitch).
            let pitch = (a.width * format::bytes(a.format)).next_multiple_of(256);
            let bind = if a.kind == kind::TARGET {
                wire::bind::RENDER_TARGET | wire::bind::SAMPLER_VIEW
            } else {
                wire::bind::SAMPLER_VIEW
            };
            (wire::target::TEXTURE_2D, vformat, bind, pitch as u64 * a.height as u64, pitch)
        }
        kind::BUFFER => {
            if a.format != 0 || a.width != 0 || a.height != 0 || a.size == 0 || a.size > BUFFER_MAX {
                return Err(EINVAL);
            }
            (wire::target::BUFFER, wire::format::R8_UNORM, wire::bind::VERTEX_BUFFER, a.size, 0)
        }
        _ => return Err(EINVAL),
    };
    let frames = (bytes as usize).div_ceil(PAGE);
    sweep();
    let id = with_core(|c| {
        let p = c.proc_mut(tgid);
        if p.resources.len() >= RESOURCE_QUOTA {
            return Err(EAGAIN);
        }
        if p.bytes + (frames * PAGE) as u64 > MEMORY_QUOTA {
            return Err(ENOMEM);
        }
        p.bytes += (frames * PAGE) as u64;
        let id = c.next_res;
        c.next_res += 1;
        Ok(id)
    })
    .ok_or(ENODEV)??;
    let unreserve = |c: &mut Core| {
        if let Some(p) = c.procs.get_mut(&tgid) {
            p.bytes = p.bytes.saturating_sub((frames * PAGE) as u64);
        }
    };
    let Some(pages) = Pages::new(frames) else {
        with_core(unreserve);
        return Err(ENOMEM);
    };
    let pages = Arc::new(pages);
    let width = if a.kind == kind::BUFFER { a.size as u32 } else { a.width };
    let height = if a.kind == kind::BUFFER { 1 } else { a.height };
    let made = exec_ok(|t| virgl::resource_create(t, id, vtarget, vformat, bind, width, height));
    if let Err(e) = made {
        with_core(unreserve);
        return Err(if e == EIO { EINVAL } else { e });
    }
    let attached = exec_opt(|t| virgl::attach_backing(t, id, &pages));
    let ctxs: Vec<u32> = with_core(|c| {
        c.procs.get(&tgid).map(|p| p.contexts.values().copied().collect()).unwrap_or_default()
    })
    .unwrap_or_default();
    let mut outcome = attached.map(|_| ());
    for ctx in ctxs {
        if outcome.is_ok() {
            outcome = exec_ok(|t| virgl::ctx_attach(t, ctx, id)).map(|_| ());
        }
    }
    let handle = with_core(|c| {
        let h = c.handle(tgid);
        c.resources.insert(id, Res {
            owner: tgid,
            kind: a.kind,
            format: a.format,
            width: a.width,
            height: a.height,
            size: a.size,
            pitch,
            backing: pages,
            generation: 0,
            last: None,
            writers: 0,
            invalid: false,
            dropped: outcome.is_err(),
            holds: 0,
            lost: false,
            reading: false,
        });
        if outcome.is_ok() {
            c.proc_mut(tgid).resources.insert(h, id);
        } else {
            c.free_if_unused(id);
        }
        h
    })
    .ok_or(ENODEV)?;
    outcome?;
    a.handle = handle;
    if let Err(e) = write_arg(arg, &a) {
        destroy_resource(tgid, handle);
        return Err(e);
    }
    Ok(0)
}

fn op_resource_destroy(arg: u64, len: u64) -> Result<u64, i64> {
    let a: abi::Handle = read_arg(arg, len)?;
    if a.reserved != 0 {
        return Err(EINVAL);
    }
    let tgid = caller()?;
    if destroy_resource(tgid, a.handle) {
        Ok(0)
    } else {
        Err(EINVAL)
    }
}

/// Drop a resource handle; storage goes when nothing holds it (section 1.4.1, hold sets).
fn destroy_resource(tgid: u64, handle: u64) -> bool {
    with_core(|c| {
        let Some(id) = c.procs.get_mut(&tgid).and_then(|p| p.resources.remove(&handle)) else { return false };
        if let Some(r) = c.resources.get_mut(&id) {
            r.dropped = true;
        }
        c.free_if_unused(id);
        true
    })
    .unwrap_or(false)
}

/// The process's resources as the validator and translator see them.
struct View {
    entries: Vec<(u64, u32, u32, u32, u32, u32, u64, bool, Arc<Pages>)>,
}

impl View {
    fn of(c: &Core, tgid: u64) -> View {
        let mut entries = Vec::new();
        if let Some(p) = c.procs.get(&tgid) {
            for (&h, id) in &p.resources {
                if let Some(r) = c.resources.get(id) {
                    if !r.lost {
                        entries.push((h, *id, r.kind, r.format, r.width, r.height, r.size, r.invalid, r.backing.clone()));
                    }
                }
            }
        }
        View { entries }
    }

    fn get(&self, handle: u64) -> Option<&(u64, u32, u32, u32, u32, u32, u64, bool, Arc<Pages>)> {
        self.entries.iter().find(|e| e.0 == handle)
    }
}

impl Resolver for View {
    fn target(&self, handle: u64) -> Option<TargetInfo> {
        let e = self.get(handle)?;
        (e.2 == kind::TARGET).then_some(TargetInfo { width: e.4, height: e.5, invalid: e.7 })
    }

    fn buffer_size(&self, handle: u64) -> Option<u64> {
        let e = self.get(handle)?;
        (e.2 == kind::BUFFER).then_some(e.6)
    }

    fn buffer_rect(&self, handle: u64, offset: u64) -> [i32; 4] {
        let Some(e) = self.get(handle) else { return [0; 4] };
        let mut b = [0u8; 16];
        e.8.read(offset as usize, &mut b);
        let w = |i: usize| i32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        [w(0), w(4), w(8), w(12)]
    }
}

impl encode::Bindings for View {
    fn target(&self, handle: u64) -> Option<encode::TargetRes> {
        let e = self.get(handle)?;
        if e.2 != kind::TARGET {
            return None;
        }
        let vformat = if e.3 == format::R8G8B8A8 { wire::format::R8G8B8A8_UNORM } else { wire::format::B8G8R8A8_UNORM };
        Some(encode::TargetRes { id: e.1, format: vformat, info: TargetInfo { width: e.4, height: e.5, invalid: e.7 } })
    }

    fn buffer_rect(&self, handle: u64, offset: u64) -> [i32; 4] {
        Resolver::buffer_rect(self, handle, offset)
    }
}

/// The resources a validated stream names: (targets it writes, every resource).
fn named(words: &WordBuf, view: &View) -> (Vec<u32>, Vec<u32>) {
    let mut written = Vec::new();
    let mut all = Vec::new();
    let mut at = 0;
    while at < words.len() {
        let code = words.word(at);
        let len = words.word(at + 1) as usize;
        let handle = |i: usize| words.word(at + i) as u64 | (words.word(at + i + 1) as u64) << 32;
        let found = match code {
            stream::cmd::BEGIN => view.get(handle(2)).map(|e| (e.1, true)),
            stream::cmd::DRAW_BUFFER => view.get(handle(4)).map(|e| (e.1, false)),
            _ => None,
        };
        if let Some((id, write)) = found {
            if write && !written.contains(&id) {
                written.push(id);
            }
            if !all.contains(&id) {
                all.push(id);
            }
        }
        at += len.max(2);
    }
    (written, all)
}

fn op_submit(arg: u64, len: u64) -> Result<u64, i64> {
    let mut a: abi::Submit = read_arg(arg, len)?;
    if a.fence != 0 || a.error_index != 0 || a.error != 0 || a.estimate != 0 || a.reserved != [0; 2] {
        return Err(EINVAL);
    }
    let tgid = caller()?;
    let refuse = |a: &mut abi::Submit, index: u32, error: u32, errno: i64| -> Result<u64, i64> {
        a.error_index = index;
        a.error = error;
        write_arg(arg, a)?;
        Err(errno)
    };
    if a.length == 0 || a.length % 4 != 0 || a.length > limits::STREAM_BYTES {
        return refuse(&mut a, 0, stream::error::STREAM_LENGTH, EINVAL);
    }
    let id = with_core(|c| match c.ctx_of(tgid, a.context).and_then(|id| c.contexts.get(&id)) {
        None => Err(EINVAL),
        Some(ctx) if ctx.state != CtxState::Live => Err(EIO),
        Some(ctx) => Ok(ctx.id),
    })
    .ok_or(ENODEV)??;
    let words = WordBuf::from_user(a.stream, a.length as usize)?;
    let view = with_core(|c| View::of(c, tgid)).ok_or(ENODEV)?;
    let plan = match stream::validate(&words, &view) {
        Ok(plan) => plan,
        Err(r) => {
            let errno = if r.is_work_limit() { EDQUOT } else { EINVAL };
            return refuse(&mut a, r.index, r.error, errno);
        }
    };
    let (written, held) = named(&words, &view);
    // Take the context's submission slot: one sender at a time, at most four in flight.
    let mut waited = false;
    let admitted = wait_for(None, true, |c| {
        let ctx = c.contexts.get_mut(&id)?;
        if ctx.state != CtxState::Live || ctx.dropped {
            return Some(Err(EIO));
        }
        if ctx.busy || ctx.inflight.len() >= limits::IN_FLIGHT {
            if ctx.inflight.len() >= limits::IN_FLIGHT {
                waited = true;
            }
            return None;
        }
        ctx.busy = true;
        Some(Ok(()))
    });
    match admitted {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err(EINTR),
    }
    let release_slot = || {
        with_core(|c| {
            if let Some(ctx) = c.contexts.get_mut(&id) {
                ctx.busy = false;
            }
            c.finalize_if_dropped(id);
        });
        notify();
    };
    // The submission's own vertex buffer: no other submission writes it, and it is
    // freed when the submission completes.
    let vertex_buffer = match vertex_buffer_for(id, encode::vertex_bytes(plan.drawn)) {
        Ok(vb) => vb,
        Err(e) => {
            release_slot();
            return Err(e);
        }
    };
    let named_ids = held.clone();
    let mut held = held;
    if let Some(vb) = vertex_buffer {
        held.push(vb);
    }
    // Translate and register the submission; nothing has reached the device yet.
    let prepared = with_core(|c| -> Result<(u64, u64, Vec<WordBuf>), i64> {
        // A resource the stream names may have been destroyed since it was validated.
        for rid in &named_ids {
            match c.resources.get(rid) {
                Some(r) if !r.dropped && !r.lost => {}
                _ => return Err(EINVAL),
            }
        }
        let ctx = c.contexts.get_mut(&id).ok_or(EINVAL)?;
        let mut enc = encode::Enc::new();
        let vb = vertex_buffer.unwrap_or(ctx.vb);
        encode::translate(&mut enc, &mut ctx.objects, &words, &view, vb, FLIP_Y);
        let chunks = enc.finish().ok_or(ENOMEM)?;
        ctx.issued += 1;
        let n = ctx.issued;
        let ns = ctx.ns;
        ctx.totals.submissions += 1;
        ctx.totals.draws += plan.draws;
        ctx.totals.instances += plan.instances;
        ctx.totals.area += plan.area;
        ctx.totals.estimate += plan.estimate;
        if waited {
            ctx.totals.ring_waits += 1;
        }
        let serial = c.serial();
        let fence = c.device_fence();
        if let Some(ctx) = c.contexts.get_mut(&id) {
            ctx.inflight.push_back(serial);
        }
        let p = c.proc_mut(tgid);
        p.totals.submissions += 1;
        p.totals.draws += plan.draws;
        p.totals.instances += plan.instances;
        p.totals.area += plan.area;
        p.totals.estimate += plan.estimate;
        if waited {
            p.totals.ring_waits += 1;
        }
        for rid in &held {
            if let Some(r) = c.resources.get_mut(rid) {
                r.holds += 1;
            }
        }
        for rid in &written {
            if let Some(r) = c.resources.get_mut(rid) {
                r.writers += 1;
            }
        }
        c.subs.insert(serial, Sub {
            ctx: id,
            n,
            fence,
            sent_ns: 0,
            error: None,
            mismatch: false,
            written: written.clone(),
            held: held.clone(),
        });
        Ok((serial, (ns as u64) << 32 | n as u64, chunks))
    })
    .ok_or(ENODEV);
    let (serial, bgl_fence, chunks) = match prepared {
        Ok(Ok(p)) => p,
        Ok(Err(e)) | Err(e) => {
            if let Some(vb) = vertex_buffer {
                with_core(|c| c.free_if_unused(vb));
            }
            release_slot();
            return Err(e);
        }
    };
    // Send the chunks in order; only the last command carries the fence. Test point 27,
    // `device-error`, follows the chunks with a fenced command of a type VirtIO does not
    // define, which the device can only refuse, and makes that the last. A reset or
    // quarantine while sending ends the submission, which has then already completed
    // with its error.
    let forced = with_core(|_| crate::test_control::take(test_point::DEVICE_ERROR, 0).is_some()).unwrap_or(false);
    let mut chunks: VecDeque<WordBuf> = chunks.into_iter().collect();
    if chunks.is_empty() && !forced {
        // An empty translation still needs a fenced command to complete in order.
        chunks.push_back(WordBuf::new());
    }
    let mut sending = Ok(());
    while let Some(words) = chunks.pop_front() {
        let last = chunks.is_empty() && !forced;
        let Some(fence) = with_core(|c| c.subs.get(&serial).map(|s| s.fence)).flatten() else { break };
        let req = virgl::submit(Tag::Submit { ctx: id, serial, last }, id, last.then_some(fence), words);
        sending = send_blocking(req, |c| {
            if last {
                if let Some(sub) = c.subs.get_mut(&serial) {
                    sub.sent_ns = now_ns();
                }
            }
        });
        if sending.is_err() {
            break;
        }
    }
    if forced && sending.is_ok() {
        if let Some(fence) = with_core(|c| c.subs.get(&serial).map(|s| s.fence)).flatten() {
            let req = virgl::undefined_command(Tag::Submit { ctx: id, serial, last: true }, id, fence);
            sending = send_blocking(req, |c| {
                if let Some(sub) = c.subs.get_mut(&serial) {
                    sub.sent_ns = now_ns();
                }
            });
        }
    }
    release_slot();
    sending?;
    a.fence = bgl_fence;
    a.estimate = plan.estimate;
    write_arg(arg, &a)?;
    Ok(0)
}

/// A device buffer of `bytes` for one submission's vertices, attached to context `ctx`;
/// `None` when the submission draws nothing. It has no guest backing (the vertices
/// reach it by inline writes) and is freed once the submission no longer holds it.
fn vertex_buffer_for(ctx: u32, bytes: u64) -> Result<Option<u32>, i64> {
    if bytes == 0 {
        return Ok(None);
    }
    let size = u32::try_from(bytes).map_err(|_| ENOMEM)?;
    let id = with_core(|c| {
        let id = c.next_res;
        c.next_res += 1;
        c.resources.insert(id, Res {
            owner: 0,
            kind: kind::BUFFER,
            format: format::B8G8R8A8,
            width: 0,
            height: 0,
            size: bytes,
            pitch: 0,
            backing: Arc::new(Pages::none()),
            generation: 0,
            last: None,
            writers: 0,
            invalid: false,
            dropped: true,
            holds: 1,
            lost: false,
            reading: false,
        });
        id
    })
    .ok_or(ENODEV)?;
    let made = exec_ok(|t| virgl::resource_create(t, id, wire::target::BUFFER, wire::format::R8_UNORM, wire::bind::VERTEX_BUFFER, size, 1))
        .and_then(|_| exec_ok(|t| virgl::ctx_attach(t, ctx, id)));
    // The submission takes over the hold made above.
    with_core(|c| {
        if let Some(r) = c.resources.get_mut(&id) {
            r.holds -= 1;
            if made.is_err() {
                r.lost = true;
            }
        }
    });
    match made {
        Ok(_) => Ok(Some(id)),
        Err(e) => {
            with_core(|c| c.free_if_unused(id));
            Err(e)
        }
    }
}

fn op_wait(arg: u64, len: u64) -> Result<u64, i64> {
    let mut a: abi::Wait = read_arg(arg, len)?;
    if a.status != 0 || a.answer != 0 || a.reserved != [0; 2] {
        return Err(EINVAL);
    }
    let tgid = caller()?;
    let (id, n) = with_core(|c| {
        let id = c.ctx_of(tgid, a.context)?;
        let ctx = c.contexts.get(&id)?;
        let n = (a.fence & 0xFFFF_FFFF) as u32;
        // A fence names its context: another context's fence is refused.
        ((a.fence >> 32) as u32 == ctx.ns && n >= 1 && n <= ctx.issued).then_some((id, n))
    })
    .flatten()
    .ok_or(EINVAL)?;
    let deadline = now_ns().saturating_add(a.timeout_ns);
    let outcome = wait_for(Some(deadline), true, |c| {
        let ctx = c.contexts.get(&id)?;
        (n <= ctx.done_upto).then(|| ctx.outcomes.get(&n).copied().unwrap_or((fence_status::OK, 0)))
    });
    match outcome {
        Ok((fence_status::OK, _)) => Ok(0),
        Ok((status, answer)) => {
            a.status = status;
            a.answer = answer;
            write_arg(arg, &a)?;
            Err(EIO)
        }
        Err(Stop::Timeout) => Err(ETIMEDOUT),
        Err(Stop::Interrupted) => Err(EINTR),
    }
}

fn op_readback(arg: u64, len: u64) -> Result<u64, i64> {
    let a: abi::Readback = read_arg(arg, len)?;
    if a.reserved0 != 0 || a.reserved != 0 {
        return Err(EINVAL);
    }
    let tgid = caller()?;
    let (id, fmt, tw, th, pitch, backing) = with_core(|c| {
        let id = c.res_of(tgid, a.target).ok_or(EINVAL)?;
        let r = c.resources.get(&id).ok_or(EINVAL)?;
        if r.kind != kind::TARGET {
            return Err(EINVAL);
        }
        if r.lost {
            return Err(EIO);
        }
        Ok((id, r.format, r.width, r.height, r.pitch, r.backing.clone()))
    })
    .ok_or(ENODEV)??;
    let (x, y, w, h) = (a.x as u64, a.y as u64, a.width as u64, a.height as u64);
    if w == 0 || h == 0 || x + w > tw as u64 || y + h > th as u64 {
        return Err(EINVAL);
    }
    let row = w * 4;
    if (a.stride as u64) < row || a.pixels_len < (h - 1) * a.stride as u64 + row {
        return Err(EINVAL);
    }
    // Wait for every accepted write of the target to complete, then take it for reading.
    let taken = wait_for(None, true, |c| {
        let r = c.resources.get_mut(&id)?;
        if r.lost || r.dropped {
            return Some(Err(EIO));
        }
        if r.writers > 0 || r.reading {
            return None;
        }
        r.reading = true;
        Some(Ok((r.invalid, r.generation, r.last)))
    });
    let (invalid, generation, last) = match taken {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err(EINTR),
    };
    let release = || {
        with_core(|c| {
            if let Some(r) = c.resources.get_mut(&id) {
                r.reading = false;
            }
            c.free_if_unused(id);
        });
        notify();
    };
    if invalid {
        release();
        return Err(EIO);
    }
    let outcome = observe(tgid, id, last, &backing, pitch, fmt, a);
    release();
    let (unwritten, transfer_fence, transfer_answer) = outcome?;
    let mut rec = [0u8; record::SIZE];
    let put32 = |rec: &mut [u8; record::SIZE], at: usize, v: u32| rec[at..at + 4].copy_from_slice(&v.to_le_bytes());
    let put64 = |rec: &mut [u8; record::SIZE], at: usize, v: u64| rec[at..at + 8].copy_from_slice(&v.to_le_bytes());
    let (serial, renderer) = with_core(|c| {
        let s = c.next_obs;
        c.next_obs += 1;
        (s, c.caps.renderer)
    })
    .ok_or(ENODEV)?;
    put32(&mut rec, record::VERSION_OFF, record::VERSION);
    put32(&mut rec, record::SIZE_OFF, record::SIZE as u32);
    put64(&mut rec, record::SERIAL, serial);
    put32(&mut rec, record::BACKEND, abi::backend::VIRGL);
    put32(&mut rec, record::BOUNDARY, record::BOUNDARY_HOST);
    put32(&mut rec, record::FORMAT, fmt);
    put32(&mut rec, record::X, a.x);
    put32(&mut rec, record::Y, a.y);
    put32(&mut rec, record::WIDTH, a.width);
    put32(&mut rec, record::HEIGHT, a.height);
    put32(&mut rec, record::TARGET_WIDTH, tw);
    put32(&mut rec, record::TARGET_HEIGHT, th);
    put32(&mut rec, record::STRIDE, a.stride);
    put64(&mut rec, record::TARGET, a.target);
    put64(&mut rec, record::RESOURCE, id as u64);
    put64(&mut rec, record::GENERATION, generation);
    if let Some(l) = last {
        put64(&mut rec, record::CONTEXT, l.ctx as u64);
        put64(&mut rec, record::RENDER_SERIAL, l.serial);
        put64(&mut rec, record::RENDER_FENCE, l.fence);
        put32(&mut rec, record::RENDER_STATUS, l.answer);
        put32(&mut rec, record::RENDER_KIND, wire::cmd::SUBMIT_3D);
    }
    put64(&mut rec, record::READBACK_FENCE, transfer_fence);
    put32(&mut rec, record::READBACK_STATUS, transfer_answer);
    put32(&mut rec, record::READBACK_KIND, wire::cmd::TRANSFER_FROM_HOST_3D);
    put32(&mut rec, record::POISON, POISON);
    put32(&mut rec, record::UNWRITTEN, unwritten);
    rec[record::RENDERER..record::RENDERER + 64].copy_from_slice(&renderer);
    crate::syscall::userptr::write_user_bytes(a.record, rec.as_ptr(), record::SIZE).map_err(|_| EFAULT)?;
    with_core(|c| {
        if let Some(l) = last {
            if let Some(ctx) = c.contexts.get_mut(&l.ctx) {
                ctx.totals.readbacks += 1;
            }
        }
        c.proc_mut(tgid).totals.readbacks += 1;
    });
    if unwritten != 0 {
        return Err(EIO);
    }
    Ok(0)
}

/// Read a rectangle of target `id` into the caller's pixels, band by band: each band is
/// transferred twice, into the poison and into its complement, and the bytes that differ
/// between the two are counted as unwritten (section 1.8). Returns that count and the
/// first transfer's fence and answer.
fn observe(tgid: u64, id: u32, last: Option<LastWrite>, backing: &Pages, pitch: u32, fmt: u32, a: abi::Readback) -> Result<(u32, u64, u32), i64> {
    let ctx = with_core(|c| {
        let mine = |cid: &u32| c.contexts.get(cid).is_some_and(|x| x.owner == tgid && x.state == CtxState::Live && x.device_alive);
        last.map(|l| l.ctx)
            .filter(mine)
            .or_else(|| c.procs.get(&tgid).and_then(|p| p.contexts.values().copied().find(mine)))
    })
    .flatten()
    .ok_or(EINVAL)?;
    let row = a.width as usize * 4;
    let band_rows = (READBACK_BAND / row).max(1).min(a.height as usize);
    let mut temp = alloc::vec![0u8; band_rows * row];
    let mut unwritten = 0u32;
    let mut first: Option<(u64, u32)> = None;
    let mut by = 0usize;
    while by < a.height as usize {
        let rows = band_rows.min(a.height as usize - by);
        let ty = a.y as usize + by;
        let offset = ty * pitch as usize + a.x as usize * 4;
        for (pass, pattern) in [POISON, !POISON].into_iter().enumerate() {
            for r in 0..rows {
                backing.fill(offset + r * pitch as usize, row, pattern);
            }
            backing.maintain(offset, (rows - 1) * pitch as usize + row, false);
            let (serial, fence) = with_core(|c| (c.serial(), c.device_fence())).ok_or(ENODEV)?;
            let req = virgl::transfer_from_host(Tag::Sync(serial), ctx, fence, id, a.x, ty as u32, a.width, rows as u32, offset as u64, pitch);
            let d = exec(serial, req, None)?;
            if !virgl::ok(d.answer) || d.echoed != fence {
                return Err(EIO);
            }
            first.get_or_insert((fence, d.answer));
            backing.maintain(offset, (rows - 1) * pitch as usize + row, true);
            for r in 0..rows {
                let dst = &mut temp[r * row..(r + 1) * row];
                if pass == 0 {
                    backing.read(offset + r * pitch as usize, dst);
                } else {
                    let mut second = alloc::vec![0u8; row];
                    backing.read(offset + r * pitch as usize, &mut second);
                    unwritten += dst.iter().zip(second.iter()).filter(|(p, q)| p != q).count() as u32;
                }
            }
        }
        for r in 0..rows {
            let src = &mut temp[r * row..(r + 1) * row];
            if fmt == format::R8G8B8A8 {
                // Canonical bytes are BGRA (record v2's `canonical` 0).
                for px in src.chunks_exact_mut(4) {
                    px.swap(0, 2);
                }
            }
            let dst = a.pixels + ((by + r) as u64) * a.stride as u64;
            crate::syscall::userptr::write_user_bytes(dst, src.as_ptr(), row).map_err(|_| EFAULT)?;
        }
        by += rows;
    }
    let (fence, answer) = first.unwrap_or((0, 0));
    Ok((unwritten, fence, answer))
}

fn op_counters(arg: u64, len: u64) -> Result<u64, i64> {
    let mut a: abi::Counters = read_arg(arg, len)?;
    let tgid = caller()?;
    let filled = with_core(|c| {
        let proc_live = c.procs.get(&tgid);
        let fill = |out: &mut abi::Counters, t: &Totals| {
            out.submissions = t.submissions;
            out.draws = t.draws;
            out.instances = t.instances;
            out.fragment_area = t.area;
            out.work_estimate = t.estimate;
            out.readbacks = t.readbacks;
            out.ring_waits = t.ring_waits;
            out.device_errors = t.device_errors;
        };
        let mut out = abi::Counters::ZERO;
        out.context = a.context;
        if a.context == 0 {
            if let Some(p) = proc_live {
                fill(&mut out, &p.totals);
                out.contexts = p.contexts.len() as u64;
                out.resources = p.resources.len() as u64;
                out.resource_bytes = p.bytes;
            }
        } else {
            let id = c.ctx_of(tgid, a.context)?;
            let ctx = c.contexts.get(&id)?;
            fill(&mut out, &ctx.totals);
            out.state = match ctx.state {
                CtxState::Quarantined => abi::context_state::QUARANTINED,
                CtxState::Reset => abi::context_state::RESET,
                _ => abi::context_state::LIVE,
            };
            out.warmed = ctx.warmed as u64;
            if let Some(p) = proc_live {
                out.contexts = p.contexts.len() as u64;
                out.resources = p.resources.len() as u64;
                out.resource_bytes = p.bytes;
            }
        }
        out.device_contexts = c.contexts.values().filter(|x| x.owner == tgid && x.device_alive).count() as u64;
        Some(out)
    })
    .flatten();
    match filled {
        Some(out) => {
            a = out;
            write_arg(arg, &a)?;
            Ok(0)
        }
        None => Err(EINVAL),
    }
}
