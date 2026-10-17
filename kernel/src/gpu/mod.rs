//! The kernel GPU core: system call 416, `gpu(op, arg, len)` (`docs/gui/gpu.md` section
//! 1.4, argument layouts in `docs/gui/gpu-abi.md`).
//!
//! Per-process contexts and resources behind opaque handles; every stream validated whole
//! before anything reaches the device (`gpu_abi::stream`), against one copy of the buffer
//! ranges it names taken at `SUBMIT`; fences that complete in submission order; waits that
//! time out, take signals and end on a kill; and a watchdog over every device wait the
//! kernel makes, submissions and its own synchronous requests alike, that quarantines the
//! context of one the device has not answered 2 s after it was sent and resets the device
//! when its queue then makes no progress for another 2 s, or when the device faults
//! (section 1.4.1). Two device errors in a row on one context quarantine that context only.
//! The device is the VirGL backend (`virgl`), on ARM64 QEMU in this milestone.
//!
//! A fence's result reaching its callers and the device's use of the memory behind it
//! ending are separate events. Every request goes through one queue in order (the backlog,
//! then the control queue), so nothing is dropped when the control queue is full, and each
//! request owns the memory the device may use for it. A submission's holds on its resources
//! end only when the device has returned every request of it, or a reset the device
//! confirmed has ended them; a reset the device does not confirm leaves everything held.
//!
//! Locking: one spin lock over the core, taken with preemption disabled and never by the
//! interrupt handler, which only reads the device's ISR byte, counts an event and wakes the
//! waiters. User memory is copied outside the lock; nothing blocks or waits on the device
//! while holding it, and the reset itself runs on `kgpu` outside it; no path here prints
//! (section 1.4: no kernel output on any path).

mod completion;
pub mod dma;
pub mod virgl;

use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cell::Cell;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use gpu_abi::errno::{EAGAIN, EDQUOT, EFAULT, EINTR, EINVAL, EIO, ENODEV, ENOMEM, ENOSYS, ETIMEDOUT};
use gpu_abi::stream::{self, Resolver, TargetInfo};
use gpu_abi::{self as abi, capability, fence_status, format, kind, limits, record, test_point, Plain};

use crate::syscall::SyscallResult;
use crate::task::thread::ThreadState;
use crate::task::waitqueue::WaitQueueHead;
use dma::{Pages, WordBuf, PAGE};
use virgl::{encode, wire, Device, Done, Reap, Request, Tag};

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
/// R, the reset budget: from the reset decision to the device back up (section 1.4.1).
const RESET_BUDGET_NS: u64 = 100_000_000;
/// The watchdog examines outstanding work at least this often (section 1.4.1).
const EXAMINE_NS: u64 = 250_000_000;
/// Failed fences a live context keeps answers for; one more quarantines it.
const OUTCOMES_MAX: usize = 256;

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

/// Wake `kgpu` for work only it does: a reset.
fn wake_reaper() {
    let tid = REAPER.load(Ordering::Acquire);
    if tid != 0 {
        crate::task::scheduler::wake_thread_any_context(tid);
    }
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
    creating: bool,
}

struct Ctx {
    id: u32,
    owner: u64,
    /// The fence namespace: a BGL fence is `ns << 32 | n` for the context's `n`th
    /// submission.
    ns: u32,
    state: CtxState,
    dropped: bool,
    /// A submission is being prepared.
    busy: bool,
    device_alive: bool,
    issued: u32,
    done_upto: u32,
    early: BTreeSet<u32>,
    /// Fences that completed other than OK, with their status and the device's answer.
    outcomes: BTreeMap<u32, (u32, u32)>,
    /// Submissions whose result has not reached callers, in fence order.
    inflight: VecDeque<u64>,
    totals: Totals,
    objects: encode::Objects,
    vb: u32,
    scratch: u32,
    warmed: u32,
    /// Device errors in a row, in fence order.
    errors_in_row: u32,
    /// Test point 31's fence-depth mode on this context.
    depth: Option<Depth>,
}

/// Test point 31's fence-depth mode: how many more submissions to keep, and the ones kept.
struct Depth {
    left: u32,
    kept: Vec<u64>,
}

struct Sub {
    ctx: u32,
    n: u32,
    fence: u64,
    /// When `SUBMIT` handed it to the device's queue; the watchdog's clock.
    sent_ns: u64,
    error: Option<u32>,
    mismatch: bool,
    /// Its requests the device or the backlog still holds.
    requests: u32,
    /// Its result has reached callers.
    published: bool,
    /// Its holds have ended: the device returned every request, or a confirmed reset
    /// ended them.
    retired: bool,
    /// Kept by test point 31's fence-depth mode.
    kept: bool,
    /// A kept submission's result, until it is published.
    result: Option<(u32, u32)>,
    written: Vec<u32>,
    whole: Vec<u32>,
    held: Vec<u32>,
}

/// A synchronous request a thread waits on.
struct SyncReq {
    /// The device context it runs in, 0 for none.
    ctx: u32,
    sent_ns: u64,
    /// Resources it holds until the device returns it.
    holds: Vec<u32>,
    /// A readback's target, whose `reading` mark the request keeps if its waiter leaves.
    reading: Option<u32>,
    /// Its waiter has left; its answer is dropped when it comes.
    abandoned: bool,
    /// The watchdog or a reset has failed its wait.
    failed: bool,
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
    /// The reset epoch it was created in.
    epoch: u64,
    generation: u64,
    last: Option<LastWrite>,
    writers: u32,
    invalid: bool,
    invalidated_by: u64,
    dropped: bool,
    holds: u32,
    lost: bool,
    reading: bool,
}

struct Core {
    dev: Option<Device>,
    /// The control queue's size, kept while the device is out for a reset.
    queue_size: usize,
    /// A device whose reset was never confirmed, with every request it was given or that
    /// was queued for it: none of their memory is ever released.
    wedged: Option<(Device, Vec<Request>)>,
    status: u32,
    reason: &'static str,
    /// Each profile capability the device has, indexed by capability id.
    have: [bool; capability::LAST as usize + 1],
    caps: abi::Caps,
    procs: BTreeMap<u64, Proc>,
    contexts: BTreeMap<u32, Ctx>,
    resources: BTreeMap<u32, Res>,
    subs: BTreeMap<u64, Sub>,
    syncs: BTreeMap<u64, SyncReq>,
    done: BTreeMap<u64, Done>,
    /// Requests not yet in the control queue, in order, with the reset epoch they were
    /// queued in.
    backlog: VecDeque<(u64, Request)>,
    /// Counts reset decisions.
    epoch: u64,
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
    resetting: bool,
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
                    rd(wire::caps::CAPABILITY_BITS_V2),
                    rd(wire::caps::MAX_TEXTURE_2D_SIZE),
                    core::str::from_utf8(&core.caps.renderer).unwrap_or("").trim_end_matches('\0')
                );
            }
            ISR.store(dev.isr, Ordering::Release);
            INTID.store(dev.intid, Ordering::Release);
            crate::arch_impl::aarch64::gic::configure_spi_level_triggered(dev.intid);
            crate::arch_impl::aarch64::gic::enable_spi(dev.intid);
            core.queue_size = dev.queue_size();
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
            queue_size: 0,
            wedged: None,
            status: abi::status::NO_DEVICE,
            reason: "no virtio-gpu device",
            have: [false; capability::LAST as usize + 1],
            caps,
            procs: BTreeMap::new(),
            contexts: BTreeMap::new(),
            resources: BTreeMap::new(),
            subs: BTreeMap::new(),
            syncs: BTreeMap::new(),
            done: BTreeMap::new(),
            backlog: VecDeque::new(),
            epoch: 0,
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
            resetting: false,
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

    /// Whether ops may run, and if not, the status and the missing capability. Test point
    /// 23 (`mask-capability`) is one-shot: the next admission decision counts the
    /// capability its parameter names as absent, so that op fails naming it
    /// (`device/missing-capability-fails`), and the decision after it sees the device's
    /// real capabilities.
    fn admission(&self) -> Result<(), (u32, u32)> {
        if self.dev.is_none() && !self.resetting {
            return Err((if self.status == abi::status::ADMITTED { abi::status::DEVICE_FAILED } else { self.status }, 0));
        }
        let masked = crate::test_control::take(test_point::MASK_CAPABILITY, 0)
            .map(|p| p as u32)
            .filter(|&c| (1..=capability::LAST).contains(&c));
        match self.first_missing(masked) {
            Some(missing) => Err((abi::status::MISSING_CAPABILITY, missing)),
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
            creating: false,
        })
    }

    fn ctx_of(&self, tgid: u64, handle: u64) -> Option<u32> {
        self.procs.get(&tgid)?.contexts.get(&handle).copied()
    }

    fn res_of(&self, tgid: u64, handle: u64) -> Option<u32> {
        self.procs.get(&tgid)?.resources.get(&handle).copied()
    }

    // -----------------------------------------------------------------------------------
    // Requests.

    /// Whether a request can be queued: a device is up or coming back from a reset, and
    /// the request's chain fits its control queue.
    fn can_queue(&self, req: &Request) -> bool {
        (self.dev.is_some() || self.resetting) && req.segment_count() <= self.queue_size
    }

    /// Queue `req` behind everything already queued and hand the device what fits.
    fn enqueue(&mut self, req: Request) -> Result<(), Request> {
        if !self.can_queue(&req) {
            return Err(req);
        }
        self.backlog.push_back((self.epoch, req));
        self.flush();
        Ok(())
    }

    /// Move queued requests into the control queue, in order, while they fit; nothing
    /// more once a device fault has asked for a reset.
    fn flush(&mut self) {
        if self.reset_wanted {
            return;
        }
        let Some(dev) = self.dev.as_mut() else { return };
        while let Some((_, req)) = self.backlog.front() {
            if !dev.has_room(req.segment_count()) {
                break;
            }
            let Some((epoch, req)) = self.backlog.pop_front() else { break };
            if let Err(req) = dev.send(req) {
                self.backlog.push_front((epoch, req));
                break;
            }
        }
    }

    /// Queue a request nobody waits for.
    fn background(&mut self, req: Request) {
        if let Err(req) = self.enqueue(req) {
            self.unsendable(req);
        }
    }

    /// A request that can no longer reach a device: kept for ever while a device that may
    /// still use memory it names was never confirmed reset.
    fn unsendable(&mut self, req: Request) {
        match self.wedged.as_mut() {
            Some((_, keep)) => keep.push(req),
            None => drop(req),
        }
    }

    /// Register synchronous request `serial`, take its holds and queue it.
    fn start_sync(&mut self, serial: u64, req: Request, ctx: u32, holds: &[u32], reading: Option<u32>) -> Result<(), i64> {
        if !self.can_queue(&req) {
            return Err(EIO);
        }
        for id in holds {
            if let Some(r) = self.resources.get_mut(id) {
                r.holds += 1;
            }
        }
        self.syncs.insert(serial, SyncReq { ctx, sent_ns: now_ns(), holds: holds.to_vec(), reading, abandoned: false, failed: false });
        self.backlog.push_back((self.epoch, req));
        self.flush();
        Ok(())
    }

    /// The waiter of `serial` left: drop its answer if it came, or when it comes.
    fn abandon(&mut self, serial: u64) {
        if self.done.remove(&serial).is_none() {
            if let Some(s) = self.syncs.get_mut(&serial) {
                s.abandoned = true;
            }
        }
    }

    // -----------------------------------------------------------------------------------
    // The device's answers.

    /// Take every completion the device has returned and the watchdog's decisions.
    fn pump(&mut self) {
        self.reap_all();
        self.watchdog(now_ns());
        self.flush();
    }

    fn reap_all(&mut self) {
        if self.stalled {
            return;
        }
        let mut any = false;
        loop {
            let reaped = match self.dev.as_mut() {
                Some(dev) => dev.reap(),
                None => break,
            };
            match reaped {
                Reap::Empty => break,
                Reap::Fault => {
                    self.fault();
                    break;
                }
                Reap::Done(d) => {
                    any = true;
                    self.last_progress = now_ns();
                    self.returned(d);
                    if self.stalled {
                        break;
                    }
                }
            }
        }
        if !self.reset_wanted && self.dev.as_ref().is_some_and(|d| d.needs_reset()) {
            self.fault();
        }
        if any {
            notify();
        }
    }

    /// A device fault (section 1.4.1): the device is reset.
    fn fault(&mut self) {
        self.reset_wanted = true;
        notify();
        wake_reaper();
    }

    /// The device returned a request.
    fn returned(&mut self, d: Done) {
        match d.tag {
            Tag::Sync(serial) => self.sync_ended(serial, Some(d)),
            Tag::Submit { serial, last, .. } => self.submit_returned(serial, last, d.answer, d.echoed),
            Tag::Background => {}
        }
    }

    /// A request the device will never answer: never sent before a confirmed reset, or
    /// held by the device across one.
    fn ended(&mut self, req: Request) {
        match req.tag {
            Tag::Sync(serial) => self.sync_ended(serial, None),
            Tag::Submit { serial, .. } => {
                if let Some(s) = self.subs.get_mut(&serial) {
                    s.requests = s.requests.saturating_sub(1);
                }
                self.publish(serial, fence_status::DEVICE_RESET, 0);
                self.retire_if_done(serial);
            }
            Tag::Background => {}
        }
        drop(req);
    }

    /// Synchronous request `serial` is over: with the device's answer, or without one
    /// after a confirmed reset.
    fn sync_ended(&mut self, serial: u64, d: Option<Done>) {
        let Some(s) = self.syncs.remove(&serial) else { return };
        for id in &s.holds {
            self.unhold(*id);
        }
        if s.abandoned {
            if let Some(id) = s.reading {
                if let Some(r) = self.resources.get_mut(&id) {
                    r.reading = false;
                }
                self.free_if_unused(id);
            }
        } else {
            let d = d.unwrap_or_else(|| Done { tag: Tag::Sync(serial), fence: None, answer: 0, echoed: 0, resp: dma::DmaBox::new(wire::HDR) });
            self.done.insert(serial, d);
        }
        notify();
    }

    /// One SUBMIT_3D of submission `serial` came back.
    fn submit_returned(&mut self, serial: u64, last: bool, answer: u32, echoed: u64) {
        let Some(sub) = self.subs.get_mut(&serial) else { return };
        sub.requests = sub.requests.saturating_sub(1);
        if !virgl::ok(answer) {
            sub.error.get_or_insert(answer);
        }
        if last {
            let mut echoed = echoed;
            // Test point 20, `stale-fence`: the answer claims another submission's fence.
            if crate::test_control::take(test_point::STALE_FENCE, 0).is_some() {
                echoed = sub.fence + 1;
            }
            if sub.error.is_none() && echoed != sub.fence {
                sub.mismatch = true;
            }
            let result = match (sub.error, sub.mismatch) {
                (Some(a), _) => (fence_status::DEVICE_ERROR, a),
                (None, true) => (fence_status::FENCE_MISMATCH, echoed as u32),
                (None, false) => (fence_status::OK, answer),
            };
            let (ctx, kept) = (sub.ctx, sub.kept);
            // Test point 31, `withhold-fence`, parameters 0 and 1: ignore this completion,
            // or every completion from now until the device is reset.
            let withheld = crate::test_control::take_if(test_point::WITHHOLD_FENCE, 0, |p| p != test_point::WITHHOLD_DEPTH);
            match withheld {
                Some(p) => {
                    if p == test_point::WITHHOLD_ALL {
                        self.stalled = true;
                    }
                }
                None => {
                    // Fence depth keeps the answer until a fifth SUBMIT waits for room.
                    let keep = kept && self.contexts.get(&ctx).is_some_and(|c| c.depth.is_some());
                    if keep {
                        if let Some(sub) = self.subs.get_mut(&serial) {
                            sub.result = Some(result);
                        }
                    } else {
                        self.publish(serial, result.0, result.1);
                    }
                }
            }
        }
        self.retire_if_done(serial);
    }

    /// Give submission `serial`'s result to its callers: its fence, its targets'
    /// publication or invalidation, and its context's error count. Its holds are not
    /// touched: they end when the device is done with it.
    fn publish(&mut self, serial: u64, status: u32, answer: u32) {
        let Some(sub) = self.subs.get_mut(&serial) else { return };
        if sub.published {
            return;
        }
        sub.published = true;
        let (ctx_id, n, fence, retired) = (sub.ctx, sub.n, sub.fence, sub.retired);
        let written = core::mem::take(&mut sub.written);
        let whole = core::mem::take(&mut sub.whole);
        let ok = status == fence_status::OK;
        if let Some(ctx) = self.contexts.get_mut(&ctx_id) {
            if !ok {
                ctx.outcomes.insert(n, (status, answer));
            }
            ctx.early.insert(n);
            while ctx.early.remove(&(ctx.done_upto + 1)) {
                ctx.done_upto += 1;
                // In fence order, an OK ends a run of device errors.
                match ctx.outcomes.get(&ctx.done_upto) {
                    None => ctx.errors_in_row = 0,
                    Some(&(fence_status::DEVICE_ERROR, _)) => ctx.errors_in_row += 1,
                    Some(_) => {}
                }
            }
            ctx.inflight.retain(|&s| s != serial);
            if status == fence_status::DEVICE_ERROR {
                ctx.totals.device_errors += 1;
                if let Some(p) = self.procs.get_mut(&ctx.owner) {
                    p.totals.device_errors += 1;
                }
            }
            // Two device errors in a row quarantine this context and no other; so does
            // keeping more failed answers than a live context may.
            if ctx.state == CtxState::Live && (ctx.errors_in_row >= 2 || ctx.outcomes.len() > OUTCOMES_MAX) {
                ctx.state = CtxState::Quarantined;
            }
        }
        for id in &written {
            if let Some(r) = self.resources.get_mut(id) {
                r.writers = r.writers.saturating_sub(1);
                completion::finish_write(&mut r.invalid, &mut r.invalidated_by, serial, ok, whole.contains(id));
                if ok {
                    r.generation += 1;
                    r.last = Some(LastWrite { ctx: ctx_id, fence, serial, answer });
                }
            }
        }
        for id in written {
            self.free_if_unused(id);
        }
        if retired {
            self.subs.remove(&serial);
        }
        self.settle(ctx_id);
        notify();
    }

    /// End submission `serial`'s holds once the device holds none of its requests.
    fn retire_if_done(&mut self, serial: u64) {
        let Some(sub) = self.subs.get_mut(&serial) else { return };
        if sub.requests > 0 {
            return;
        }
        if !sub.retired {
            sub.retired = true;
            let held = core::mem::take(&mut sub.held);
            for id in held {
                self.unhold(id);
            }
        }
        if self.subs.get(&serial).is_some_and(|s| s.published) {
            self.subs.remove(&serial);
        }
    }

    /// Test point 31's fence-depth mode ends: a fifth SUBMIT waits at the four-in-flight
    /// limit, so the kept answers are published, in fence order.
    fn release_depth(&mut self, id: u32) {
        let Some(depth) = self.contexts.get_mut(&id).and_then(|c| c.depth.take()) else { return };
        let mut kept: Vec<(u32, u64)> = depth.kept.iter().filter_map(|s| self.subs.get(s).map(|x| (x.n, *s))).collect();
        kept.sort_unstable();
        for (_, serial) in kept {
            if let Some((status, answer)) = self.subs.get_mut(&serial).and_then(|s| s.result.take()) {
                self.publish(serial, status, answer);
            }
        }
    }

    fn unhold(&mut self, id: u32) {
        if let Some(r) = self.resources.get_mut(&id) {
            r.holds = r.holds.saturating_sub(1);
        }
        self.free_if_unused(id);
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
        if r.lost {
            // A confirmed reset ended the device's use of it.
            return;
        }
        // The device may use the backing until it answers the unref, which keeps it.
        let req = virgl::resource_unref(Tag::Background, id, Some(r.backing));
        self.background(req);
    }

    /// Destroy the device context of a context that is dropped or quarantined once none of
    /// its submissions awaits its result, and forget a dropped context then.
    fn settle(&mut self, id: u32) {
        let Some(c) = self.contexts.get_mut(&id) else { return };
        if c.busy || !c.inflight.is_empty() || c.state == CtxState::Creating {
            return;
        }
        let destroy = c.device_alive && (c.dropped || c.state == CtxState::Quarantined);
        if destroy {
            c.device_alive = false;
        }
        let dropped = c.dropped;
        if destroy {
            let req = virgl::ctx_destroy(Tag::Background, id);
            self.background(req);
        }
        if dropped {
            if let Some(c) = self.contexts.remove(&id) {
                for internal in [c.vb, c.scratch] {
                    if let Some(r) = self.resources.get_mut(&internal) {
                        r.dropped = true;
                    }
                    self.free_if_unused(internal);
                }
            }
        }
    }

    // -----------------------------------------------------------------------------------
    // The watchdog and the reset.

    /// The watchdog (section 1.4.1): quarantine the context of a submission or a
    /// synchronous request the device has not answered 2 s after it was sent, and have the
    /// device reset when it then makes no progress for 2 s.
    fn watchdog(&mut self, now: u64) {
        if self.dev.is_none() || self.reset_wanted {
            return;
        }
        if let Some(at) = self.quarantined_at {
            let busy = self.stalled || !self.backlog.is_empty() || self.dev.as_ref().is_some_and(|d| d.outstanding() > 0);
            if !busy {
                self.quarantined_at = None;
            } else if now.saturating_sub(at.max(self.last_progress)) >= limits::WATCHDOG_NS {
                self.reset_wanted = true;
                notify();
                wake_reaper();
                return;
            }
        }
        let hung: Vec<u32> = self
            .subs
            .values()
            .filter(|s| !s.published && now.saturating_sub(s.sent_ns) >= limits::WATCHDOG_NS)
            .map(|s| s.ctx)
            .collect();
        for ctx in hung {
            self.quarantine(ctx, now);
        }
        let late: Vec<u64> = self
            .syncs
            .iter()
            .filter(|(_, s)| !s.failed && now.saturating_sub(s.sent_ns) >= limits::WATCHDOG_NS)
            .map(|(&serial, _)| serial)
            .collect();
        for serial in late {
            let Some(s) = self.syncs.get_mut(&serial) else { continue };
            s.failed = true;
            let ctx = s.ctx;
            if ctx != 0 {
                self.quarantine(ctx, now);
            } else {
                self.quarantined_at.get_or_insert(now);
                notify();
            }
        }
    }

    /// Quarantine context `id`: its pending fences fail with `DEVICE_HUNG`, its targets
    /// become invalid, further submissions are refused and its device context is
    /// destroyed. Its work keeps its holds until the device returns it.
    fn quarantine(&mut self, id: u32, now: u64) {
        if let Some(c) = self.contexts.get_mut(&id) {
            if c.state != CtxState::Reset {
                c.state = CtxState::Quarantined;
            }
            c.depth = None;
            let pending: Vec<u64> = c.inflight.iter().copied().collect();
            for serial in pending {
                self.publish(serial, fence_status::DEVICE_HUNG, 0);
            }
            self.settle(id);
        }
        // No progress is measured from the first quarantine.
        self.quarantined_at.get_or_insert(now);
        notify();
    }

    /// The earliest time the watchdog has something to decide.
    fn next_deadline(&self, now: u64) -> Option<u64> {
        let hangs = self.subs.values().filter(|s| !s.published).map(|s| s.sent_ns);
        let waits = self.syncs.values().filter(|s| !s.failed).map(|s| s.sent_ns);
        let progress = self.quarantined_at.map(|q| q.max(self.last_progress));
        let at = hangs.chain(waits).chain(progress).min()?;
        Some((at + limits::WATCHDOG_NS).min(now + EXAMINE_NS))
    }

    /// The reset decision, on `kgpu`: every context fails with `DEVICE_RESET` and every
    /// synchronous wait with EIO, and the device is taken out to be reset outside the lock.
    fn begin_reset(&mut self) -> Option<Device> {
        if !core::mem::replace(&mut self.reset_wanted, false) || self.dev.is_none() {
            return None;
        }
        self.quarantined_at = None;
        self.stalled = false;
        self.resetting = true;
        self.epoch += 1;
        for c in self.contexts.values_mut() {
            c.state = CtxState::Reset;
            c.device_alive = false;
            c.depth = None;
        }
        let pending: Vec<u64> = self.subs.iter().filter(|(_, s)| !s.published).map(|(&k, _)| k).collect();
        for serial in pending {
            self.publish(serial, fence_status::DEVICE_RESET, 0);
        }
        for s in self.syncs.values_mut() {
            s.failed = true;
        }
        notify();
        self.dev.take()
    }

    /// The device confirmed the reset: nothing it was given, and nothing queued for it
    /// before the decision, is in use any more. Every resource from before is lost.
    fn finish_reset(&mut self, dev: Device, held: Vec<Request>, up: Result<(), &'static str>) {
        for req in held {
            self.ended(req);
        }
        let epoch = self.epoch;
        let queued: Vec<(u64, Request)> = self.backlog.drain(..).collect();
        for (at, req) in queued {
            if at < epoch {
                self.ended(req);
            } else {
                self.backlog.push_back((at, req));
            }
        }
        let old: Vec<u32> = self.resources.iter().filter(|(_, r)| r.epoch < epoch).map(|(&id, _)| id).collect();
        for id in old {
            if let Some(r) = self.resources.get_mut(&id) {
                r.lost = true;
            }
            self.free_if_unused(id);
        }
        self.resetting = false;
        match up {
            Ok(()) => self.dev = Some(dev),
            Err(e) => {
                drop(dev);
                self.status = abi::status::DEVICE_FAILED;
                self.reason = e;
                let queued: Vec<(u64, Request)> = self.backlog.drain(..).collect();
                for (_, req) in queued {
                    self.ended(req);
                }
            }
        }
        self.last_progress = now_ns();
        self.flush();
        let ids: Vec<u32> = self.contexts.keys().copied().collect();
        for id in ids {
            self.settle(id);
        }
        notify();
    }

    /// The device did not complete the reset: it may still use everything it was given,
    /// so nothing is released, and the core refuses further work.
    fn reset_failed(&mut self, dev: Device) {
        let queued: Vec<Request> = self.backlog.drain(..).map(|(_, r)| r).collect();
        self.wedged = Some((dev, queued));
        self.resetting = false;
        self.status = abi::status::DEVICE_FAILED;
        self.reason = "virtio-gpu reset did not complete";
        for s in self.syncs.values_mut() {
            s.failed = true;
        }
        notify();
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
            self.settle(id);
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

/// Send synchronous request `serial` (built with `Tag::Sync(serial)`) in device context
/// `ctx` (0 for none), holding `holds` until the device returns it, and wait for its
/// answer. The watchdog fails the wait 2 s after the request was sent (EIO), and a signal
/// or a kill ends it (EINTR); either way the request keeps its holds, and `reading`'s
/// mark, until the device returns it or a confirmed reset ends it.
fn exec_req(serial: u64, req: Request, ctx: u32, holds: &[u32], reading: Option<u32>) -> Result<Done, i64> {
    with_core(|c| c.start_sync(serial, req, ctx, holds, reading)).unwrap_or(Err(ENODEV))?;
    let outcome = wait_for(None, true, |c| {
        if let Some(d) = c.done.remove(&serial) {
            return Some(Ok(d));
        }
        c.syncs.get(&serial).filter(|s| s.failed).map(|_| Err(EIO))
    });
    match outcome {
        Ok(Ok(d)) => Ok(d),
        other => {
            with_core(|c| c.abandon(serial));
            Err(match other {
                Ok(Err(e)) => e,
                _ => EINTR,
            })
        }
    }
}

/// Build and run one synchronous command, requiring a success answer.
fn exec_ok(build: impl FnOnce(Tag) -> Option<Request>, ctx: u32, holds: &[u32]) -> Result<Done, i64> {
    let serial = with_core(|c| c.serial()).ok_or(ENODEV)?;
    let req = build(Tag::Sync(serial)).ok_or(ENOMEM)?;
    let d = exec_req(serial, req, ctx, holds, None)?;
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

/// Start `kgpu` if it is not running; false when it cannot be started.
fn ensure_reaper() -> bool {
    loop {
        if REAPER.load(Ordering::Acquire) != 0 {
            return true;
        }
        if REAPER_STARTING.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            let started = match crate::task::kthread::kthread_run(reaper, "kgpu") {
                Ok(handle) => {
                    REAPER.store(handle.tid(), Ordering::Release);
                    true
                }
                Err(_) => false,
            };
            REAPER_STARTING.store(false, Ordering::Release);
            return started;
        }
        crate::task::scheduler::yield_current();
    }
}

/// Block `kgpu` until `wake` (or indefinitely) or until a GPU event after `seen`.
fn reaper_block(tid: u64, seen: u64, wake: Option<u64>) {
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

/// `kgpu`: takes completions nobody is waiting for, runs the watchdog at its deadlines
/// and at least every 250 ms while work is outstanding, resets the device when the
/// watchdog or a device fault asks for it, and reclaims what exited processes left, once
/// a second while any process holds GPU state.
fn reaper() {
    let Some(tid) = crate::task::scheduler::current_thread_id() else { return };
    let mut next_sweep = now_ns();
    loop {
        let seen = EVENTS.load(Ordering::Acquire);
        let now = now_ns();
        let (deadline, busy, reset) = with_core(|c| {
            c.pump();
            (c.next_deadline(now), !c.procs.is_empty(), c.reset_wanted)
        })
        .unwrap_or((None, false, false));
        if reset {
            reset_device(tid);
            continue;
        }
        if busy && now >= next_sweep {
            sweep();
            next_sweep = now + 1_000_000_000;
        }
        let wake = match (deadline, busy) {
            (Some(d), true) => Some(d.min(next_sweep)),
            (Some(d), false) => Some(d),
            (None, true) => Some(next_sweep),
            (None, false) => None,
        };
        reaper_block(tid, seen, wake);
    }
}

/// Reset the device (VirtIO 1.3 section 2.4) outside the core's lock: write status 0 and
/// wait, within the reset budget, for the device to read it back as 0. Only that
/// confirmation releases what the device was given; without it the device is kept with
/// everything it holds.
fn reset_device(tid: u64) {
    let Some(mut dev) = with_core(|c| c.begin_reset()).flatten() else { return };
    dev.begin_reset();
    let start = now_ns();
    while !dev.reset_done() && now_ns().saturating_sub(start) < RESET_BUDGET_NS {
        reaper_block(tid, EVENTS.load(Ordering::Acquire), Some(now_ns() + 1_000_000));
    }
    if dev.reset_done() {
        let (held, up) = dev.restart();
        with_core(move |c| c.finish_reset(dev, held, up));
    } else {
        with_core(move |c| c.reset_failed(dev));
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

/// Serialize a process's context/resource creation through device attachment and
/// handle publication, so their snapshots cannot both miss each other.
struct CreationGuard(u64);

impl CreationGuard {
    fn acquire(tgid: u64) -> Result<Self, i64> {
        wait_for(None, true, |c| {
            let p = c.proc_mut(tgid);
            if p.creating {
                return None;
            }
            p.creating = true;
            Some(())
        }).map_err(|_| EINTR)?;
        Ok(Self(tgid))
    }
}

impl Drop for CreationGuard {
    fn drop(&mut self) {
        with_core(|c| {
            if let Some(p) = c.procs.get_mut(&self.0) {
                p.creating = false;
            }
        });
        notify();
    }
}

/// A new resource record: no handle yet, nothing written.
#[allow(clippy::too_many_arguments)]
fn new_res(owner: u64, kind_: u32, format_: u32, width: u32, height: u32, size: u64, pitch: u32, backing: Arc<Pages>, epoch: u64) -> Res {
    Res {
        owner,
        kind: kind_,
        format: format_,
        width,
        height,
        size,
        pitch,
        backing,
        epoch,
        generation: 0,
        last: None,
        writers: 0,
        invalid: false,
        invalidated_by: 0,
        dropped: false,
        holds: 0,
        lost: false,
        reading: false,
    }
}

fn op_context_create(arg: u64, len: u64) -> Result<u64, i64> {
    let mut a: abi::ContextCreate = read_arg(arg, len)?;
    if a.flags != 0 || a.handle != 0 || a.warmed != 0 || a.reserved != [0; 2] {
        return Err(EINVAL);
    }
    let tgid = caller()?;
    let _creation = CreationGuard::acquire(tgid)?;
    sweep();
    if !ensure_reaper() {
        return Err(ENOMEM);
    }
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
        let epoch = c.epoch;
        c.resources.insert(vb, new_res(0, kind::BUFFER, format::B8G8R8A8, 0, 0, encode::VERTEX_BUFFER_BYTES as u64, 256, vb_pages.clone(), epoch));
        c.resources.insert(scratch, new_res(0, kind::TARGET, format::B8G8R8A8, 1, 1, 0, 256, scratch_pages.clone(), epoch));
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
            depth: None,
        });
        Ok((id, vb, scratch))
    })
    .ok_or(ENODEV)??;
    let created = create_device_context(tgid, id, vb, scratch, &vb_pages, &scratch_pages).and_then(|()| {
        with_core(|c| {
            // The watchdog or a reset may have ended the context while it was created.
            if !c.contexts.get(&id).is_some_and(|x| x.state == CtxState::Creating && x.device_alive) {
                return Err(EIO);
            }
            let h = c.handle(tgid);
            c.proc_mut(tgid).contexts.insert(h, id);
            if let Some(ctx) = c.contexts.get_mut(&id) {
                ctx.state = CtxState::Live;
                ctx.warmed = encode::CATALOGUE_PROGRAMS;
            }
            Ok(h)
        })
        .unwrap_or(Err(ENODEV))
    });
    let handle = match created {
        Ok(h) => h,
        Err(e) => {
            with_core(|c| {
                if let Some(ctx) = c.contexts.get_mut(&id) {
                    if ctx.state == CtxState::Creating {
                        ctx.state = CtxState::Live;
                    }
                    ctx.dropped = true;
                }
                c.settle(id);
            });
            return Err(e);
        }
    };
    a.handle = handle;
    a.warmed = encode::CATALOGUE_PROGRAMS;
    if let Err(e) = write_arg(arg, &a) {
        destroy_context(tgid, handle);
        return Err(e);
    }
    Ok(0)
}

/// Create device context `id` with its internal resources and the process's resources
/// attached, then compile and warm the catalogue's programs and wait for the warm-up's
/// fence. Every wait here is a synchronous device request under the watchdog, ended by a
/// kill (section 1.4.1, program warm-up).
fn create_device_context(tgid: u64, id: u32, vb: u32, scratch: u32, vb_pages: &Arc<Pages>, scratch_pages: &Arc<Pages>) -> Result<(), i64> {
    // Counted as alive from the request on, so a failed creation queues CTX_DESTROY
    // behind it whatever the device made of it.
    with_core(|c| {
        if let Some(ctx) = c.contexts.get_mut(&id) {
            ctx.device_alive = true;
        }
    });
    exec_ok(|t| Some(virgl::ctx_create(t, id)), id, &[])?;
    exec_ok(
        |t| Some(virgl::resource_create(t, vb, wire::target::BUFFER, wire::format::R8_UNORM, wire::bind::VERTEX_BUFFER, encode::VERTEX_BUFFER_BYTES, 1)),
        0,
        &[],
    )?;
    exec_ok(|t| virgl::attach_backing(t, vb, vb_pages.clone()), 0, &[])?;
    exec_ok(|t| Some(virgl::ctx_attach(t, id, vb)), id, &[])?;
    exec_ok(
        |t| {
            Some(virgl::resource_create(
                t,
                scratch,
                wire::target::TEXTURE_2D,
                wire::format::B8G8R8A8_UNORM,
                wire::bind::RENDER_TARGET | wire::bind::SAMPLER_VIEW,
                1,
                1,
            ))
        },
        0,
        &[],
    )?;
    exec_ok(|t| virgl::attach_backing(t, scratch, scratch_pages.clone()), 0, &[])?;
    exec_ok(|t| Some(virgl::ctx_attach(t, id, scratch)), id, &[])?;
    let theirs: Vec<u32> = with_core(|c| c.procs.get(&tgid).map(|p| p.resources.values().copied().collect()).unwrap_or_default())
        .unwrap_or_default();
    for rid in theirs {
        exec_ok(|t| Some(virgl::ctx_attach(t, id, rid)), id, &[])?;
    }
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
    let d = exec_req(serial, req, id, &[vb, scratch], None)?;
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
        c.settle(id);
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
    let _creation = CreationGuard::acquire(tgid)?;
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
    let charge = (frames * PAGE) as u64;
    sweep();
    let id = with_core(|c| {
        let p = c.proc_mut(tgid);
        if p.resources.len() >= RESOURCE_QUOTA {
            return Err(EAGAIN);
        }
        if p.bytes + charge > MEMORY_QUOTA {
            return Err(ENOMEM);
        }
        p.bytes += charge;
        let id = c.next_res;
        c.next_res += 1;
        Ok(id)
    })
    .ok_or(ENODEV)??;
    let Some(pages) = Pages::new(frames) else {
        with_core(|c| {
            if let Some(p) = c.procs.get_mut(&tgid) {
                p.bytes = p.bytes.saturating_sub(charge);
            }
        });
        return Err(ENOMEM);
    };
    let pages = Arc::new(pages);
    let width = if a.kind == kind::BUFFER { a.size as u32 } else { a.width };
    let height = if a.kind == kind::BUFFER { 1 } else { a.height };
    // The record exists, held by its creation and without a handle, from before the
    // device is asked to create it, so whatever happens the device's resource is unref'd.
    with_core(|c| {
        let mut r = new_res(tgid, a.kind, a.format, a.width, a.height, a.size, pitch, pages.clone(), c.epoch);
        r.holds = 1;
        c.resources.insert(id, r);
    });
    let mut outcome = exec_ok(|t| Some(virgl::resource_create(t, id, vtarget, vformat, bind, width, height)), 0, &[])
        .map(drop)
        .map_err(|e| if e == EIO { EINVAL } else { e });
    if outcome.is_ok() {
        outcome = exec_ok(|t| virgl::attach_backing(t, id, pages.clone()), 0, &[]).map(drop);
    }
    let ctxs: Vec<u32> = with_core(|c| {
        c.procs.get(&tgid).map(|p| p.contexts.values().copied().collect()).unwrap_or_default()
    })
    .unwrap_or_default();
    for ctx in ctxs {
        if outcome.is_ok() {
            if let Err(e) = exec_ok(|t| Some(virgl::ctx_attach(t, ctx, id)), ctx, &[]) {
                // A context that ended meanwhile needs no attachment.
                let ended = with_core(|c| !c.contexts.get(&ctx).is_some_and(|x| x.state == CtxState::Live && x.device_alive && !x.dropped))
                    .unwrap_or(true);
                if !ended {
                    outcome = Err(e);
                }
            }
        }
    }
    let handle = with_core(|c| {
        if let Some(r) = c.resources.get_mut(&id) {
            r.holds -= 1;
            r.dropped = outcome.is_err();
        }
        if outcome.is_ok() {
            let h = c.handle(tgid);
            c.proc_mut(tgid).resources.insert(h, id);
            Some(h)
        } else {
            c.free_if_unused(id);
            None
        }
    })
    .flatten();
    outcome?;
    a.handle = handle.ok_or(ENODEV)?;
    if let Err(e) = write_arg(arg, &a) {
        destroy_resource(tgid, a.handle);
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

/// One of the process's resources as `SUBMIT` sees it.
struct Entry {
    handle: u64,
    id: u32,
    kind: u32,
    format: u32,
    width: u32,
    height: u32,
    size: u64,
    invalid: bool,
    /// A buffer's backing, read once into the submission's snapshot.
    backing: Option<Arc<Pages>>,
}

/// The process's resources as the validator and translator see them.
struct View {
    entries: Vec<Entry>,
}

impl View {
    fn of(c: &Core, tgid: u64) -> View {
        let mut entries = Vec::new();
        if let Some(p) = c.procs.get(&tgid) {
            for (&handle, &id) in &p.resources {
                if let Some(r) = c.resources.get(&id) {
                    if !r.lost {
                        entries.push(Entry {
                            handle,
                            id,
                            kind: r.kind,
                            format: r.format,
                            width: r.width,
                            height: r.height,
                            size: r.size,
                            invalid: r.invalid,
                            backing: (r.kind == kind::BUFFER).then(|| r.backing.clone()),
                        });
                    }
                }
            }
        }
        View { entries }
    }

    fn get(&self, handle: u64) -> Option<&Entry> {
        self.entries.iter().find(|e| e.handle == handle)
    }

    fn has_id(&self, id: u32) -> bool {
        self.entries.iter().any(|e| e.id == id)
    }
}

/// One buffer range a stream names, copied into the snapshot at word `first`.
struct Range {
    handle: u64,
    offset: u64,
    bytes: u64,
    first: usize,
}

/// The buffer ranges a stream names, copied once at `SUBMIT` before validation:
/// validation, the work estimate and translation all read this copy, never the buffer
/// (`docs/gui/gpu.md` section 1.4.1, buffers are snapshotted at submission).
struct Snapshot {
    ranges: Vec<Range>,
    words: WordBuf,
    /// The range the last lookup found: commands read their ranges in stream order.
    hint: Cell<usize>,
}

impl Snapshot {
    /// Copy every `DRAW_BUFFER` range of `stream` that lies inside a buffer the process
    /// owns. The copy stops at the first malformed command, where validation refuses the
    /// stream, and takes at most the instance limit's rectangles, which bounds every range
    /// validation reads.
    fn take(stream: &WordBuf, view: &View) -> Result<Snapshot, i64> {
        let mut snap = Snapshot { ranges: Vec::new(), words: WordBuf::new(), hint: Cell::new(0) };
        let total = stream.len();
        let mut rects = 0u64;
        let mut at = 0usize;
        while total - at >= 2 {
            let code = stream.word(at);
            let len = stream.word(at + 1) as usize;
            if len < 2 || len > total - at {
                break;
            }
            if code == stream::cmd::DRAW_BUFFER && len == 8 {
                let handle = stream.word(at + 4) as u64 | (stream.word(at + 5) as u64) << 32;
                let offset = stream.word(at + 6) as u64;
                let count = stream.word(at + 7) as u64;
                let bytes = 16 * count;
                let fits = |e: &&Entry| offset % 16 == 0 && offset + bytes <= e.size && rects + count <= limits::INSTANCES;
                if let Some(backing) = view.get(handle).filter(fits).and_then(|e| e.backing.as_ref()) {
                    snap.ranges.try_reserve(1).map_err(|_| ENOMEM)?;
                    let first = snap.words.len();
                    let mut chunk = [0u8; 256];
                    let mut done = 0u64;
                    while done < bytes {
                        let n = (bytes - done).min(chunk.len() as u64) as usize;
                        backing.read((offset + done) as usize, &mut chunk[..n]);
                        for w in chunk[..n].chunks_exact(4) {
                            if !snap.words.push(u32::from_le_bytes([w[0], w[1], w[2], w[3]])) {
                                return Err(ENOMEM);
                            }
                        }
                        done += n as u64;
                    }
                    snap.ranges.push(Range { handle, offset, bytes, first });
                    rects += count;
                }
            }
            at += len;
        }
        Ok(snap)
    }

    /// The rectangle at byte `offset` of buffer `handle`, from the copy.
    fn rect(&self, handle: u64, offset: u64) -> [i32; 4] {
        let inside = |r: &Range| r.handle == handle && offset >= r.offset && offset + 16 <= r.offset + r.bytes;
        let hint = self.hint.get();
        let found = [hint, hint + 1]
            .into_iter()
            .find(|&i| self.ranges.get(i).is_some_and(inside))
            .or_else(|| self.ranges.iter().position(inside));
        let Some(i) = found else { return [0; 4] };
        self.hint.set(i);
        let r = &self.ranges[i];
        let w = r.first + ((offset - r.offset) / 4) as usize;
        [0, 1, 2, 3].map(|k| self.words.word(w + k) as i32)
    }
}

/// The view's handles with the snapshot's buffer contents.
struct Resolved<'a> {
    view: &'a View,
    snap: &'a Snapshot,
}

impl Resolver for Resolved<'_> {
    fn target(&self, handle: u64) -> Option<TargetInfo> {
        let e = self.view.get(handle)?;
        (e.kind == kind::TARGET).then_some(TargetInfo { width: e.width, height: e.height, invalid: e.invalid })
    }

    fn buffer_size(&self, handle: u64) -> Option<u64> {
        let e = self.view.get(handle)?;
        (e.kind == kind::BUFFER).then_some(e.size)
    }

    fn buffer_rect(&self, handle: u64, offset: u64) -> [i32; 4] {
        self.snap.rect(handle, offset)
    }
}

impl encode::Bindings for Resolved<'_> {
    fn target(&self, handle: u64) -> Option<encode::TargetRes> {
        let e = self.view.get(handle)?;
        if e.kind != kind::TARGET {
            return None;
        }
        let vformat = if e.format == format::R8G8B8A8 { wire::format::R8G8B8A8_UNORM } else { wire::format::B8G8R8A8_UNORM };
        Some(encode::TargetRes { id: e.id, format: vformat, info: TargetInfo { width: e.width, height: e.height, invalid: e.invalid } })
    }

    fn buffer_rect(&self, handle: u64, offset: u64) -> [i32; 4] {
        self.snap.rect(handle, offset)
    }
}

/// The resources a validated stream names: written targets, every resource, full clears.
fn named(words: &WordBuf, view: &View) -> (Vec<u32>, Vec<u32>, Vec<u32>) {
    let mut written = Vec::new();
    let mut all = Vec::new();
    let mut whole = Vec::new();
    let mut target = None;
    let mut at = 0;
    while at < words.len() {
        let code = words.word(at);
        let len = words.word(at + 1) as usize;
        let handle = |i: usize| words.word(at + i) as u64 | (words.word(at + i + 1) as u64) << 32;
        let found = match code {
            stream::cmd::BEGIN => view.get(handle(2)).map(|e| (e.id, true)),
            stream::cmd::DRAW_BUFFER => view.get(handle(4)).map(|e| (e.id, false)),
            _ => None,
        };
        if code == stream::cmd::BEGIN {
            target = found.map(|(id, _)| id);
        } else if code == stream::cmd::CLEAR {
            if let Some(id) = target {
                if !whole.contains(&id) {
                    whole.push(id);
                }
            }
        }
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
    (written, all, whole)
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
    let snap = Snapshot::take(&words, &view)?;
    let resolved = Resolved { view: &view, snap: &snap };
    let plan = match stream::validate(&words, &resolved) {
        Ok(plan) => plan,
        Err(r) => {
            let errno = if r.is_work_limit() { EDQUOT } else { EINVAL };
            return refuse(&mut a, r.index, r.error, errno);
        }
    };
    let (written, named_ids, whole) = named(&words, &view);
    // Take the context's submission slot: one sender at a time, at most four in flight.
    let mut waited = false;
    let admitted = wait_for(None, true, |c| {
        let Some(ctx) = c.contexts.get_mut(&id) else { return Some(Err(EIO)) };
        if ctx.state != CtxState::Live || ctx.dropped {
            return Some(Err(EIO));
        }
        if ctx.inflight.len() >= limits::IN_FLIGHT {
            waited = true;
            if ctx.depth.is_some() {
                c.release_depth(id);
            }
            return None;
        }
        if ctx.busy {
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
            c.settle(id);
        });
        notify();
    };
    // The submission's own vertex buffer: no other submission writes it, and it is
    // freed when the submission no longer holds it.
    let vertex_buffer = match vertex_buffer_for(id, encode::vertex_bytes(plan.drawn)) {
        Ok(vb) => vb,
        Err(e) => {
            release_slot();
            return Err(e);
        }
    };
    let drop_vertex_buffer = || {
        if let Some(vb) = vertex_buffer {
            with_core(|c| c.unhold(vb));
        }
    };
    // The context's busy slot serializes object allocation; translation can allocate
    // and process a large stream without holding the global core lock.
    let state = with_core(|c| c.contexts.get(&id).map(|ctx| (ctx.objects.clone(), ctx.vb, ctx.scratch))).flatten();
    let Some((mut objects, default_vb, scratch)) = state else {
        drop_vertex_buffer();
        release_slot();
        return Err(EIO);
    };
    let stale: Vec<u32> = objects.surfaces.keys().copied().filter(|&rid| rid != scratch && !view.has_id(rid)).collect();
    let mut enc = encode::Enc::new();
    encode::forget_surfaces(&mut enc, &mut objects, &stale);
    encode::translate(&mut enc, &mut objects, &words, &resolved, vertex_buffer.unwrap_or(default_vb), FLIP_Y);
    let chunks = enc.finish();
    let mut held = named_ids.clone();
    if let Some(vb) = vertex_buffer {
        held.push(vb);
    }
    // Recheck the context and the resources after translation, then hand the whole
    // submission to the device's queue at once: nothing of a refused submission is sent,
    // and nothing of an accepted one is left unsent.
    let prepared = with_core(|c| -> Result<u64, i64> {
        let ctx = c.contexts.get(&id).ok_or(EINVAL)?;
        if ctx.state != CtxState::Live || !ctx.device_alive || ctx.dropped {
            return Err(EIO);
        }
        for rid in &named_ids {
            match c.resources.get(rid) {
                Some(r) if !r.dropped && !r.lost => {}
                _ => return Err(EINVAL),
            }
        }
        let chunks = chunks.ok_or(ENOMEM)?;
        // Every chunk's chain, and test point 27's two-buffer command, fits the queue.
        if chunks.iter().any(|w| w.page_count() + 2 > c.queue_size) || c.queue_size < 2 {
            return Err(ENOMEM);
        }
        // Test point 27, `device-error`: the chunks are followed by a fenced command of a
        // type VirtIO does not define, which the device can only refuse, and that command
        // carries the fence. Taken only once the submission is certain to be sent.
        let forced = crate::test_control::take(test_point::DEVICE_ERROR, 0).is_some();
        let serial = c.serial();
        let fence = c.device_fence();
        let ctx = c.contexts.get_mut(&id).ok_or(EINVAL)?;
        // Test point 31's fence-depth mode: this context's next four answers are kept.
        if ctx.depth.is_none()
            && crate::test_control::take_if(test_point::WITHHOLD_FENCE, 0, |p| p == test_point::WITHHOLD_DEPTH).is_some()
        {
            ctx.depth = Some(Depth { left: limits::IN_FLIGHT as u32, kept: Vec::new() });
        }
        let kept = match ctx.depth.as_mut() {
            Some(d) if d.left > 0 => {
                d.left -= 1;
                d.kept.push(serial);
                true
            }
            _ => false,
        };
        ctx.objects = objects;
        ctx.issued += 1;
        let n = ctx.issued;
        let ns = ctx.ns;
        ctx.inflight.push_back(serial);
        let count_in = |t: &mut Totals| {
            t.submissions += 1;
            t.draws += plan.draws;
            t.instances += plan.instances;
            t.area += plan.area;
            t.estimate += plan.estimate;
            if waited {
                t.ring_waits += 1;
            }
        };
        count_in(&mut ctx.totals);
        count_in(&mut c.proc_mut(tgid).totals);
        let mut reqs: Vec<Request> = Vec::new();
        let count = chunks.len();
        for (i, words) in chunks.into_iter().enumerate() {
            let last = i + 1 == count && !forced;
            reqs.push(virgl::submit(Tag::Submit { ctx: id, serial, last }, id, last.then_some(fence), words));
        }
        if reqs.is_empty() && !forced {
            // An empty translation still needs a fenced command to complete in order.
            reqs.push(virgl::submit(Tag::Submit { ctx: id, serial, last: true }, id, Some(fence), WordBuf::new()));
        }
        if forced {
            reqs.push(virgl::undefined_command(Tag::Submit { ctx: id, serial, last: true }, id, fence));
        }
        for rid in &named_ids {
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
            sent_ns: now_ns(),
            error: None,
            mismatch: false,
            requests: reqs.len() as u32,
            published: false,
            retired: false,
            kept,
            result: None,
            written: written.clone(),
            whole,
            held,
        });
        let epoch = c.epoch;
        c.backlog.extend(reqs.into_iter().map(|r| (epoch, r)));
        c.flush();
        Ok((ns as u64) << 32 | n as u64)
    })
    .unwrap_or(Err(ENODEV));
    release_slot();
    let bgl_fence = match prepared {
        Ok(f) => f,
        Err(e) => {
            drop_vertex_buffer();
            return Err(e);
        }
    };
    a.fence = bgl_fence;
    a.estimate = plan.estimate;
    write_arg(arg, &a)?;
    Ok(0)
}

/// A device buffer of `bytes` for one submission's vertices, attached to context `ctx`
/// and held for the submission; `None` when the submission draws nothing. It has no guest
/// backing (the vertices reach it by inline writes) and is freed once the submission no
/// longer holds it.
fn vertex_buffer_for(ctx: u32, bytes: u64) -> Result<Option<u32>, i64> {
    if bytes == 0 {
        return Ok(None);
    }
    let size = u32::try_from(bytes).map_err(|_| ENOMEM)?;
    let id = with_core(|c| {
        let id = c.next_res;
        c.next_res += 1;
        let mut r = new_res(0, kind::BUFFER, format::B8G8R8A8, 0, 0, bytes, 0, Arc::new(Pages::none()), c.epoch);
        r.dropped = true;
        r.holds = 1;
        c.resources.insert(id, r);
        id
    })
    .ok_or(ENODEV)?;
    let made = exec_ok(
        |t| Some(virgl::resource_create(t, id, wire::target::BUFFER, wire::format::R8_UNORM, wire::bind::VERTEX_BUFFER, size, 1)),
        0,
        &[],
    )
    .and_then(|_| exec_ok(|t| Some(virgl::ctx_attach(t, ctx, id)), ctx, &[]));
    match made {
        Ok(_) => Ok(Some(id)),
        Err(e) => {
            with_core(|c| c.unhold(id));
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
        let Some(ctx) = c.contexts.get(&id) else { return Some(Err(EINVAL)) };
        (n <= ctx.done_upto).then(|| Ok(ctx.outcomes.get(&n).copied().unwrap_or((fence_status::OK, 0))))
    });
    match outcome {
        Ok(Ok((fence_status::OK, _))) => Ok(0),
        Ok(Ok((status, answer))) => {
            a.status = status;
            a.answer = answer;
            write_arg(arg, &a)?;
            Err(EIO)
        }
        Ok(Err(e)) => Err(e),
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
    // The target is ours to poison and read until every transfer into it has ended; a
    // transfer whose waiter left keeps the mark until the device returns it.
    let release = || {
        with_core(|c| {
            if !c.syncs.values().any(|s| s.reading == Some(id)) {
                if let Some(r) = c.resources.get_mut(&id) {
                    r.reading = false;
                }
                c.free_if_unused(id);
            }
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
/// between the two are counted as unwritten (section 1.8). Every transfer is a
/// synchronous request under the watchdog that holds the target, and its backing, until
/// the device returns it. Returns that count and the first transfer's fence and answer.
#[allow(clippy::too_many_arguments)]
fn observe(tgid: u64, id: u32, last: Option<LastWrite>, backing: &Arc<Pages>, pitch: u32, fmt: u32, a: abi::Readback) -> Result<(u32, u64, u32), i64> {
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
    let mut second = alloc::vec![0u8; row];
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
            let req = virgl::transfer_from_host(
                Tag::Sync(serial),
                ctx,
                fence,
                id,
                backing.clone(),
                a.x,
                ty as u32,
                a.width,
                rows as u32,
                offset as u64,
                pitch,
            );
            let d = exec_req(serial, req, ctx, &[id], Some(id))?;
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
    let mut input = a;
    input.context = 0;
    if input != abi::Counters::ZERO {
        return Err(EINVAL);
    }
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
