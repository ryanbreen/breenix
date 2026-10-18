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
//! `CONTEXT_CREATE` sends only the context's fixed state; `kgpu` warms the fixed rendering
//! catalogue afterwards, one pipeline per step and one step on the device at a time, and
//! not while a process is creating a context or resource, so creation and other requests wait
//! behind at most one step. A draw whose pipeline has not warmed creates its programs in its
//! own submission and waits for no other pipeline (ruling 33, #1725).
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
mod present_answers;
mod display;
pub mod console;
mod observation;
mod quota;
mod staging;
use quota::{Charge, Quota, QuotaCharge};
use staging::Staging;
pub mod dma;
pub mod virgl;

use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cell::Cell;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use gpu_abi::errno::{EAGAIN, EDQUOT, EFAULT, EINTR, EINVAL, EIO, ENODEV, ENOMEM, ENOSYS, ETIMEDOUT};
use gpu_abi::stream::{self, Resolver, TargetInfo};
use gpu_abi::{self as abi, readback_reason, capability, fence_status, format, kind, limits, record, test_point, Plain};

use crate::syscall::SyscallResult;
use crate::tracing::providers::gpu::{gauge, GpuPath, GPU_CONTEXTS, GPU_RESOURCES};
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
const READBACK_BAND: usize = 8 << 20;
/// The first observation's poison; the second's is its complement (section 1.8).
const POISON: u32 = 0x3C5A_A5C3;
/// VirGL's renderer keeps a target's row 0 as the transfer's row 0, so the viewport keeps
/// row 0 at the top without a flip.
const FLIP_Y: bool = false;
/// R, the reset budget: from the reset decision to the device back up (section 1.4.1).
const RESET_BUDGET_NS: u64 = 100_000_000;
/// The watchdog examines outstanding work at least this often (section 1.4.1).
const EXAMINE_NS: u64 = 250_000_000;
static CORE: spin::Mutex<Option<Core>> = spin::Mutex::new(None);
/// Counts the state changes waiters wait for; a waiter sleeps only while it is unchanged.
static EVENTS: AtomicU64 = AtomicU64::new(0);
/// Counts the device's interrupts. Only `kgpu` waits for them: it takes the device's
/// answers and tells waiters of the changes they wait for (`notify`).
static IRQS: AtomicU64 = AtomicU64::new(0);
static WAITERS: WaitQueueHead = WaitQueueHead::new();
static ISR: AtomicU64 = AtomicU64::new(0);
/// The control queue's notification address, and whether requests were queued since it
/// was last written: see [`ring_doorbell`].
static DOORBELL: AtomicU64 = AtomicU64::new(0);
static KICK: AtomicBool = AtomicBool::new(false);
/// The ISR's configuration-change bit was seen: the device may have set DEVICE_NEEDS_RESET.
static CONFIG_CHANGED: AtomicBool = AtomicBool::new(false);
static INTID: AtomicU32 = AtomicU32::new(0);
static REAPER: AtomicU64 = AtomicU64::new(0);
static REAPER_STARTING: AtomicBool = AtomicBool::new(false);

fn now_ns() -> u64 {
    crate::signal::monotonic_nanos()
}

fn with_core<R>(f: impl FnOnce(&mut Core) -> R) -> Option<R> {
    crate::per_cpu::preempt_disable();
    let r = CORE.lock().as_mut().map(f);
    ring_doorbell();
    crate::per_cpu::preempt_enable();
    r
}

/// Notify the device of the requests [`Core::flush`] made available, after the core's lock
/// is released. The notification is an MMIO write, and a hypervisor may hold the writing
/// CPU until its device model accepts it; no thread waits for the lock meanwhile. The
/// requests' descriptors and ring index were published before `KICK` was set.
fn ring_doorbell() {
    if KICK.swap(false, Ordering::AcqRel) {
        let at = DOORBELL.load(Ordering::Acquire);
        if at != 0 {
            // SAFETY: the control queue's mapped notification address; queue 0.
            unsafe { core::ptr::write_volatile(at as *mut u16, 0) };
        }
    }
}

/// Tell the waiters and `kgpu` that something changed.
fn notify() {
    EVENTS.fetch_add(1, Ordering::AcqRel);
    WAITERS.wake_up();
    wake_reaper();
}

/// What `kgpu` sleeps on: state changes and interrupts.
fn kgpu_events() -> u64 {
    EVENTS.load(Ordering::Acquire).wrapping_add(IRQS.load(Ordering::Acquire))
}

/// Wake `kgpu` for work only it does: a reset, the catalogue's next warm-up step, or
/// sending the requests the device's answers made room for.
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
    uploads_image: u64,
    uploads_fixture: u64,
}

struct Proc {
    contexts: BTreeMap<u64, u32>,
    resources: BTreeMap<u64, u32>,
    totals: Totals,
    /// Bytes charged to the process's GPU memory quota (`quota`): resource backing,
    /// staging, and `SUBMIT`'s copies and vertex storage while they are held.
    quota: Quota,
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
    /// The 1x1 texture the warm-up's sampling draws read.
    scratch_texture: u32,
    /// The rendering pipelines whose warm-up step completed, one bit each
    /// (`encode::ALL_WARM` when the catalogue's warm-up is complete).
    warm: u32,
    /// Warm-up steps the device refused in a row; two quarantine the context.
    warm_failures: u32,
    /// Device errors in a row, in fence order.
    errors_in_row: u32,
    /// Test point 31's fence-depth mode on this context.
    depth: Option<Depth>,
    /// The flush answers of completed `PRESENT`s by fence number, until their callers take
    /// them. Pending callers register before sending; interrupted callers unregister.
    flushes: present_answers::Answers,
}

impl Ctx {
    /// Programs whose warm-up completed: two per warmed rendering pipeline, and
    /// `probe_int`'s pair from its first PROBE.
    fn warmed(&self) -> u32 {
        2 * self.warm.count_ones() + if self.objects.has_probe() { encode::PROBE_PROGRAMS } else { 0 }
    }
}

/// The catalogue warm-up step on the device: one pipeline of one context, its requests not
/// yet returned, and whether the device refused any of them or a reset ended one.
struct WarmStep {
    ctx: u32,
    pipeline: u32,
    left: u32,
    refused: bool,
    lost: bool,
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
    sent_ns: Option<u64>,
    error: Option<u32>,
    mismatch: bool,
    /// Its requests the device or the backlog still holds.
    requests: u32,
    /// Its result has reached callers.
    published: bool,
    /// Kept by test point 31's fence-depth mode.
    kept: bool,
    /// A kept submission's result, until it is published.
    result: Option<(u32, u32)>,
    written: Vec<u32>,
    whole: Vec<u32>,
    /// Resources it holds until the device has returned its requests and its result has
    /// reached callers, or a confirmed reset has ended them.
    held: Vec<u32>,
    /// A `PRESENT`'s bind and flush rather than a `SUBMIT`.
    present: Option<display::Presenting>,
    /// The charge for its translated device chunks, released when it retires.
    _charge: Option<Charge>,
}

/// A synchronous request a thread waits on.
struct SyncReq {
    /// The device context it runs in, 0 for none.
    ctx: u32,
    sent_ns: Option<u64>,
    /// Texture upload and expected fence; completion accounting survives an interrupted waiter.
    upload: Option<(u32, u64)>,
    /// Resources it holds until the device returns it.
    holds: Vec<u32>,
    /// The resources whose `reading` mark the request keeps if its waiter leaves: a
    /// readback's target, and the resource its transfer names when that is another.
    reading: Vec<u32>,
    /// Its waiter has left; its answer is dropped when it comes.
    abandoned: bool,
    /// The watchdog or a reset has failed its wait.
    failed: bool,
    /// Why: `fence_status::DEVICE_HUNG` (the watchdog) or `DEVICE_RESET` (a reset decided
    /// while it was outstanding, whether the reset then completed or failed).
    why: u32,
    /// A request of the warm-up step, with its fence: its answer goes to the step.
    warm: Option<u64>,
}

#[derive(Clone, Copy)]
struct LastWrite {
    ctx: u32,
    fence: u64,
    serial: u64,
    answer: u32,
    /// The command answered: SUBMIT_3D for a submission, TRANSFER_TO_HOST_3D for an upload.
    kind: u32,
}

struct Res {
    owner: u64,
    kind: u32,
    /// Creation intent; only these targets have the device's SCANOUT bind.
    scanout: bool,
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
    display: display::State,
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
    /// The sequence handles are drawn from (`handle`).
    handle_seq: u64,
    key: u64,
    stalled: bool,
    quarantined_at: Option<u64>,
    last_progress: u64,
    reset_wanted: bool,
    resetting: bool,
    /// The one catalogue warm-up step the device has, if any.
    warm_step: Option<WarmStep>,
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
            DOORBELL.store(dev.doorbell(), Ordering::Release);
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
            display: display::State::new(),
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
            handle_seq: 0,
            key: 0,
            stalled: false,
            quarantined_at: None,
            last_progress: 0,
            reset_wanted: false,
            resetting: false,
            warm_step: None,
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
        c.pipelines = (1 << abi::pipeline::COUNT) - 1;
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

    /// A new handle for process `tgid`, unique among its handles, whose top byte is neither
    /// 0x00 nor 0xFF: no handle has the form of an address (section 1.16). The sequence it is
    /// drawn from is the core's, so a process's handles after an exec, which starts its GPU
    /// state again, are not the ones it held before.
    fn handle(&mut self, tgid: u64) -> u64 {
        let key = self.key;
        loop {
            self.handle_seq += 1;
            let seq = self.handle_seq;
            let p = self.proc_mut(tgid);
            let mut z = key ^ tgid.rotate_left(29) ^ seq.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            let top = z >> 56;
            if top != 0 && top != 0xFF && !p.contexts.contains_key(&z) && !p.resources.contains_key(&z) {
                return z;
            }
        }
    }

    fn insert_res(&mut self, id: u32, r: Res) {
        if self.resources.insert(id, r).is_none() {
            gauge(&GPU_RESOURCES, 1);
        }
    }

    fn insert_ctx(&mut self, id: u32, x: Ctx) {
        if self.contexts.insert(id, x).is_none() {
            gauge(&GPU_CONTEXTS, 1);
        }
    }

    fn proc_mut(&mut self, tgid: u64) -> &mut Proc {
        self.procs.entry(tgid).or_insert_with(|| Proc {
            contexts: BTreeMap::new(),
            resources: BTreeMap::new(),
            totals: Totals::default(),
            quota: Quota::default(),
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

    /// Move queued requests into the control queue, in order, while they fit; nothing
    /// more once a device fault has asked for a reset.
    fn flush(&mut self) {
        if self.reset_wanted {
            return;
        }
        let Some(dev) = self.dev.as_mut() else { return };
        let mut sent = false;
        while let Some((_, req)) = self.backlog.front() {
            if !dev.has_room(req.segment_count()) {
                break;
            }
            let Some((epoch, req)) = self.backlog.pop_front() else { break };
            let tag = req.tag;
            if let Err(req) = dev.send(req) {
                self.backlog.push_front((epoch, req));
                break;
            }
            let now = now_ns();
            match tag {
                Tag::Sync(serial) => {
                    if let Some(s) = self.syncs.get_mut(&serial) {
                        s.sent_ns = Some(now);
                    }
                }
                Tag::Submit { serial, .. } => {
                    if let Some(s) = self.subs.get_mut(&serial) {
                        s.sent_ns.get_or_insert(now);
                    }
                }
                Tag::Background => {}
            }
            sent = true;
        }
        if sent {
            KICK.store(true, Ordering::Release);
            wake_reaper();
        }
    }

    /// Queue a request nobody waits for.
    fn background(&mut self, req: Request) {
        let _ = self.background_in(self.epoch, req);
    }

    /// Queue a request nobody waits for that names objects of reset epoch `epoch`. A request
    /// for objects from before a reset decision is queued in their epoch, so the confirmed
    /// reset retires it with them instead of sending it to the restarted device, which no
    /// longer has them and would refuse it as a fault; a failed reset keeps it held.
    fn background_in(&mut self, epoch: u64, mut req: Request) -> Option<u64> {
        if !self.can_queue(&req) {
            self.unsendable(req);
            return None;
        }
        let serial = self.serial();
        req.tag = Tag::Sync(serial);
        self.syncs.insert(serial, SyncReq {
            ctx: 0, sent_ns: None, upload: None, holds: Vec::new(), reading: Vec::new(),
            abandoned: true, failed: false, why: 0, warm: None,
        });
        self.backlog.push_back((epoch.min(self.epoch), req));
        self.flush();
        Some(serial)
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
    fn start_sync(&mut self, serial: u64, req: Request, ctx: u32, holds: &[u32], reading: &[u32]) -> Result<(), i64> {
        if !self.can_queue(&req) {
            return Err(EIO);
        }
        // A reset between preparation and queueing invalidates resource identities.
        if holds.iter().any(|id| self.resources.get(id).is_none_or(|r| r.epoch != self.epoch || r.lost)) {
            return Err(EIO);
        }
        for id in holds {
            if let Some(r) = self.resources.get_mut(id) {
                r.holds += 1;
            }
        }
        let upload = (req.command_type() == wire::cmd::TRANSFER_TO_HOST_3D)
            .then(|| holds.first().copied().zip(req.fence)).flatten();
        self.syncs.insert(serial, SyncReq {
            upload, ctx, sent_ns: None, holds: holds.to_vec(), reading: reading.to_vec(), abandoned: false, failed: false, why: 0,
            warm: None,
        });
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

    /// A request of the warm-up step came back: `ok` is whether the device accepted it, absent
    /// when a confirmed reset ended it. When the step's last request is back, a step the
    /// device accepted warms its pipeline; one it refused counts toward the context's two
    /// refused steps in a row, which quarantine it, and is retried otherwise. A failed
    /// warm-up does not report completion (ruling 33, #1725).
    fn warm_returned(&mut self, ok: Option<bool>) {
        let Some(step) = self.warm_step.as_mut() else { return };
        step.left = step.left.saturating_sub(1);
        match ok {
            Some(false) => step.refused = true,
            None => step.lost = true,
            Some(true) => {}
        }
        if step.left > 0 {
            return;
        }
        let Some(step) = self.warm_step.take() else { return };
        if let Some(ctx) = self.contexts.get_mut(&step.ctx) {
            if step.refused {
                ctx.warm_failures += 1;
                if ctx.state == CtxState::Live && ctx.warm_failures >= 2 {
                    ctx.state = CtxState::Quarantined;
                }
            } else if !step.lost {
                ctx.warm |= 1 << step.pipeline;
                ctx.warm_failures = 0;
            }
        }
        self.settle(step.ctx);
        notify();
    }

    // -----------------------------------------------------------------------------------
    // The device's answers.

    /// `kgpu`'s turn: take every completion the device has returned and the watchdog's
    /// decisions, and send the queued requests that now fit.
    fn pump(&mut self) {
        self.reap_all();
        self.watchdog(now_ns());
        self.flush();
    }

    /// A waiter's turn: take the device's completions and the watchdog's decisions. Queued
    /// requests are left to `kgpu`, which sends them (`pump`), so a waiter never notifies
    /// the device; before `kgpu` starts, the waiter sends them.
    fn poll(&mut self) {
        self.reap_all();
        self.watchdog(now_ns());
        if REAPER.load(Ordering::Acquire) == 0 {
            self.flush();
        }
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
        // The device signals DEVICE_NEEDS_RESET with a configuration change notification
        // (VirtIO 1.3 section 2.1.2), so the status register, an MMIO read, is read only
        // after one. A device that stops answering without it meets the watchdog.
        if !self.reset_wanted
            && CONFIG_CHANGED.swap(false, Ordering::AcqRel)
            && self.dev.as_ref().is_some_and(|d| d.needs_reset())
        {
            self.fault();
        }
        // The answers' handlers notify waiters of what they wait for: a fence published, a
        // synchronous answer, a retired submission's holds, a fault. `kgpu` sends what the
        // answers made room for.
        if any {
            wake_reaper();
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
            Tag::Background => {
                if !virgl::ok(d.answer) {
                    self.fault();
                }
            }
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
        if s.ctx == 0 && d.as_ref().is_some_and(|d| !virgl::ok(d.answer)) {
            self.fault();
        }
        if let Some(fence) = s.warm {
            self.warm_returned(d.as_ref().map(|d| virgl::ok(d.answer) && d.echoed == fence));
        }
        if let Some((id, fence)) = s.upload {
            if let Some(d) = &d {
                let device_error = !virgl::ok(d.answer);
                let ok = !device_error && d.echoed == fence
                    && d.resp.u32_at(wire::HDR_FLAGS) & wire::FLAG_FENCE != 0;
                if !ok {
                    if let Some(r) = self.resources.get_mut(&id) {
                        completion::finish_write(&mut r.invalid, &mut r.invalidated_by, serial, false, false);
                    }
                }
                if let Some(ctx) = self.contexts.get_mut(&s.ctx) {
                    if device_error {
                        ctx.totals.device_errors += 1;
                        if let Some(p) = self.procs.get_mut(&ctx.owner) {
                            p.totals.device_errors += 1;
                        }
                        ctx.errors_in_row += 1;
                        if ctx.state == CtxState::Live && ctx.errors_in_row >= 2 {
                            ctx.state = CtxState::Quarantined;
                        }
                    } else if ok {
                        ctx.errors_in_row = 0;
                    }
                }
            }
        }
        for id in &s.holds {
            self.unhold(*id);
        }
        if s.abandoned {
            for id in s.reading {
                if !self.syncs.values().any(|o| o.reading.contains(&id)) {
                    if let Some(r) = self.resources.get_mut(&id) {
                        r.reading = false;
                    }
                    self.free_if_unused(id);
                }
            }
        } else {
            let d = d.unwrap_or_else(|| Done { tag: Tag::Sync(serial), fence: None, answer: 0, echoed: 0, resp: dma::DmaBox::new(wire::HDR) });
            self.done.insert(serial, d);
        }
        if s.upload.is_some() {
            self.settle(s.ctx);
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
            let submitted = sub.present.is_none();
            // Test point 20, `stale-fence`: the answer claims another submission's fence.
            if submitted && crate::test_control::take(test_point::STALE_FENCE, 0).is_some() {
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
            let withheld = if submitted {
                crate::test_control::take_if(test_point::WITHHOLD_FENCE, 0, |p| p != test_point::WITHHOLD_DEPTH)
            } else {
                None
            };
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
    /// touched: they end after device completion and fence publication.
    fn publish(&mut self, serial: u64, status: u32, answer: u32) {
        let Some(sub) = self.subs.get_mut(&serial) else { return };
        if sub.published {
            return;
        }
        sub.published = true;
        let (ctx_id, n, fence, presenting) = (sub.ctx, sub.n, sub.fence, sub.present);
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
            // Only two consecutive device errors quarantine this context.
            if ctx.state == CtxState::Live && ctx.errors_in_row >= 2 {
                ctx.state = CtxState::Quarantined;
            }
            if presenting.is_some() && ok {
                ctx.flushes.complete(n, answer);
            }
        }
        if let Some(p) = presenting {
            let ns = self.contexts.get(&ctx_id).map_or(0, |c| c.ns);
            self.presented(p, ok, (ns as u64) << 32 | n as u64, answer);
        }
        for id in &written {
            if let Some(r) = self.resources.get_mut(id) {
                r.writers = r.writers.saturating_sub(1);
                completion::finish_write(&mut r.invalid, &mut r.invalidated_by, serial, ok, whole.contains(id));
                if ok {
                    r.generation += 1;
                    r.last = Some(LastWrite { ctx: ctx_id, fence, serial, answer, kind: wire::cmd::SUBMIT_3D });
                }
            }
        }
        for id in written {
            self.free_if_unused(id);
        }
        self.retire_if_done(serial);
        self.settle(ctx_id);
        notify();
    }

    /// End submission `serial`'s holds once the device holds no request of it and its
    /// result has reached callers: a resource it names, its handle destroyed, is freed after
    /// both the device's completion and the submission's fence (section 1.4.1, hold sets),
    /// so a fence kept unpublished keeps its resources too.
    fn retire_if_done(&mut self, serial: u64) {
        let Some(sub) = self.subs.get_mut(&serial) else { return };
        if sub.requests > 0 || !sub.published {
            return;
        }
        let held = core::mem::take(&mut sub.held);
        self.subs.remove(&serial);
        for id in held {
            self.unhold(id);
        }
        notify();
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
        let _ = self.retire_if_unused(id);
    }

    /// Return an unref receipt when removing an unused resource from the core.
    fn retire_if_unused(&mut self, id: u32) -> Option<u64> {
        let r = self.resources.get(&id)?;
        if !r.dropped || r.holds > 0 || r.writers > 0 || r.reading {
            return None;
        }
        let r = self.resources.remove(&id)?;
        gauge(&GPU_RESOURCES, -1);
        // Backing owns its charges, including through other CPU or device holds.
        if r.lost {
            // A confirmed reset ended the device's use of it.
            return None;
        }
        // The device may use the backing until it answers the unref, which keeps it.
        let epoch = r.epoch;
        let req = virgl::resource_unref(Tag::Background, id, Some(r.backing));
        self.background_in(epoch, req)
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
                gauge(&GPU_CONTEXTS, -1);
                for internal in [c.vb, c.scratch, c.scratch_texture] {
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
            .filter(|s| !s.published && s.sent_ns.is_some_and(|at| now.saturating_sub(at) >= limits::WATCHDOG_NS))
            .map(|s| s.ctx)
            .collect();
        for ctx in hung {
            self.quarantine(ctx, now);
        }
        let late: Vec<u64> = self
            .syncs
            .iter()
            .filter(|(_, s)| !s.failed && s.sent_ns.is_some_and(|at| now.saturating_sub(at) >= limits::WATCHDOG_NS))
            .map(|(&serial, _)| serial)
            .collect();
        for serial in late {
            let Some(s) = self.syncs.get_mut(&serial) else { continue };
            s.failed = true;
            s.why = fence_status::DEVICE_HUNG;
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
        let hangs = self.subs.values().filter(|s| !s.published).filter_map(|s| s.sent_ns);
        let waits = self.syncs.values().filter(|s| !s.failed).filter_map(|s| s.sent_ns);
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
            if !s.failed {
                s.failed = true;
                s.why = fence_status::DEVICE_RESET;
            }
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
        self.display_reset();
        self.resetting = false;
        match up {
            Ok(()) => {
                DOORBELL.store(dev.doorbell(), Ordering::Release);
                self.dev = Some(dev);
            }
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
            if !s.failed {
                s.failed = true;
                s.why = fence_status::DEVICE_RESET;
            }
        }
        notify();
    }

    // -----------------------------------------------------------------------------------
    // Process reclamation.

    /// Release everything process `tgid` holds: contexts are destroyed once their work
    /// has completed, resources are freed once nothing holds them.
    fn reclaim(&mut self, tgid: u64) {
        display::ended(self, tgid);
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
            c.poll();
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
        let wake = Some(deadline.unwrap_or(u64::MAX).min(now_ns().saturating_add(EXAMINE_NS)));
        let outcome = WAITERS.prepare_to_wait_checked(ThreadState::BlockedOnIO, wake, || {
            EVENTS.load(Ordering::Acquire) == seen
        });
        if crate::syscall::blocking_io::wait_prepared(&WAITERS, outcome).is_err() && interruptible {
            return Err(Stop::Interrupted);
        }
    }
}

/// Send synchronous request `serial` (built with `Tag::Sync(serial)`) in device context
/// `ctx` (0 for no context), holding `holds` until the device returns it, and wait for its
/// answer. The watchdog fails the wait 2 s after the request was sent (EIO), and a signal
/// or a kill ends it (EINTR); either way the request keeps its holds, and the `reading`
/// marks, until the device returns it or a confirmed reset ends it.
fn exec_req(serial: u64, req: Request, ctx: u32, holds: &[u32], reading: &[u32]) -> Result<Done, i64> {
    exec_req_why(serial, req, ctx, holds, reading).map_err(|(e, _)| e)
}

/// [`exec_req`], with an EIO's cause: `fence_status::DEVICE_HUNG` when the watchdog ended
/// the wait, `DEVICE_RESET` when a reset was decided with the request outstanding or the
/// device is out after one, else 0.
fn exec_req_why(serial: u64, req: Request, ctx: u32, holds: &[u32], reading: &[u32]) -> Result<Done, (i64, u32)> {
    if !ensure_reaper() {
        return Err((ENOMEM, 0));
    }
    with_core(|c| c.start_sync(serial, req, ctx, holds, reading))
        .unwrap_or(Err(ENODEV))
        .map_err(|e| (e, if e == EIO { fence_status::DEVICE_RESET } else { 0 }))?;
    let outcome = wait_for(None, true, |c| {
        if let Some(d) = c.done.remove(&serial) {
            return Some(Ok(d));
        }
        c.syncs.get(&serial).filter(|s| s.failed).map(|s| Err(s.why))
    });
    match outcome {
        Ok(Ok(d)) => Ok(d),
        other => {
            with_core(|c| c.abandon(serial));
            Err(match other {
                Ok(Err(why)) => (EIO, why),
                _ => (EINTR, 0),
            })
        }
    }
}

/// Build and run one synchronous command, requiring a success answer.
fn exec_ok(build: impl FnOnce(Tag) -> Option<Request>, ctx: u32, holds: &[u32]) -> Result<Done, i64> {
    let serial = with_core(|c| c.serial()).ok_or(ENODEV)?;
    let req = build(Tag::Sync(serial)).ok_or(ENOMEM)?;
    let d = exec_req(serial, req, ctx, holds, &[])?;
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
    // The handler's whole run counts toward gpu_irq_masked_max_us (section 1.16).
    let start = crate::tracing::providers::gpu::interrupt_begin();
    let isr = ISR.load(Ordering::Acquire);
    let status = if isr != 0 { virgl::read_isr(isr) } else { 0 };
    if status != 0 {
        // Bit 1 is a configuration change (VirtIO 1.3 section 4.1.4.5).
        if status & 2 != 0 {
            CONFIG_CHANGED.store(true, Ordering::Release);
        }
        IRQS.fetch_add(1, Ordering::AcqRel);
        let tid = REAPER.load(Ordering::Acquire);
        if tid != 0 {
            crate::task::scheduler::wake_thread_any_context(tid);
        } else {
            // Before `kgpu` starts, waiters take the device's answers themselves.
            EVENTS.fetch_add(1, Ordering::AcqRel);
            WAITERS.wake_up();
        }
    }
    crate::tracing::providers::gpu::interrupt_end(start);
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

/// Block `kgpu` until `wake` (or indefinitely) or until a GPU event after `seen`. It
/// blocks for I/O: the scheduler applies a wake from interrupt context, such as
/// `handle_interrupt`'s, only to a thread blocked for I/O, so a timer block would leave
/// the device's answers waiting for the next deadline. Once woken it returns without
/// halting for a further interrupt.
fn reaper_block(tid: u64, seen: u64, wake: Option<u64>) {
    crate::arch_without_interrupts(|| {
        crate::task::scheduler::with_scheduler(|sched| {
            sched.block_current_for_io_with_timeout(wake);
        });
        if kgpu_events() != seen {
            crate::task::scheduler::with_scheduler(|sched| {
                sched.unblock(tid);
            });
        }
    });
    crate::task::scheduler::yield_current();
    crate::arch_halt_while_blocked();
}

/// `kgpu`: takes completions nobody is waiting for, runs the watchdog at its deadlines
/// and at least once per 250 ms while work is outstanding, resets the device when the
/// watchdog or a device fault asks for it, sends the fixed catalogue's warm-up steps, and
/// reclaims what exited processes left, once a second while any process holds GPU state.
fn reaper() {
    // kgpu is a GPU path for its whole life (section 1.16).
    let _path = GpuPath::enter();
    let Some(tid) = crate::task::scheduler::current_thread_id() else { return };
    let mut next_sweep = now_ns();
    loop {
        let seen = kgpu_events();
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
        warm_next();
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
        reaper_block(tid, kgpu_events(), Some(now_ns() + 1_000_000));
    }
    if dev.reset_done() {
        let (held, up) = dev.restart();
        with_core(move |c| c.finish_reset(dev, held, up));
    } else {
        with_core(move |c| c.reset_failed(dev));
    }
}

/// On `kgpu`: send the next step of the fixed rendering catalogue's warm-up (ruling 33,
/// #1725), the lowest-numbered context's first pipeline not yet warmed. One step is on the
/// device at a time, and no step starts while a process is creating a context or resource,
/// or while the device is resetting or suspected hung, so other requests wait behind at
/// most one step. The step takes the context's submission slot while it is encoded and
/// queued, like a SUBMIT, so its objects and the context's submissions reach the device in
/// the order their objects were allocated. Its requests are synchronous requests of the
/// context under the watchdog that nobody waits on; their answers go to `warm_returned`.
fn warm_next() {
    let ready = |c: &Core| {
        c.warm_step.is_none()
            && c.dev.is_some()
            && !c.reset_wanted
            && !c.resetting
            && !c.stalled
            && c.quarantined_at.is_none()
            && !c.procs.values().any(|p| p.creating)
    };
    let warming = |x: &Ctx| x.owner != 0 && x.state == CtxState::Live && x.device_alive && !x.dropped && x.warm != encode::ALL_WARM;
    if !with_core(|c| ready(c) && c.contexts.values().any(warming)).unwrap_or(false) {
        return;
    }
    // A context whose process has exited is not warmed.
    sweep();
    let picked = with_core(|c| {
        if !ready(c) {
            return None;
        }
        let ctx = c.contexts.values_mut().find(|x| warming(x) && !x.busy)?;
        ctx.busy = true;
        let pipeline = (!ctx.warm).trailing_zeros();
        Some((ctx.id, pipeline, ctx.objects.clone(), [ctx.vb, ctx.scratch, ctx.scratch_texture]))
    })
    .flatten();
    let Some((id, pipeline, mut objects, internal)) = picked else { return };
    let [vb, scratch, scratch_texture] = internal;
    let mut enc = encode::Enc::new();
    encode::warm_pipeline(&mut enc, &mut objects, vb, scratch, scratch_texture, wire::format::B8G8R8A8_UNORM, FLIP_Y, pipeline);
    // After an allocation failure no request is sent; the step is tried again later.
    let chunks = enc.finish().filter(|v| !v.is_empty());
    with_core(|c| {
        let live = c.contexts.get(&id).is_some_and(|x| x.state == CtxState::Live && x.device_alive && !x.dropped);
        if let Some(chunks) = chunks.filter(|_| live && ready(c)) {
            if chunks.iter().any(|w| w.page_count() + 2 > c.queue_size) {
                // A step the queue cannot carry is refused like one the device refuses.
                c.warm_step = Some(WarmStep { ctx: id, pipeline, left: 1, refused: true, lost: false });
                c.warm_returned(Some(false));
            } else {
                if let Some(ctx) = c.contexts.get_mut(&id) {
                    ctx.objects = objects;
                }
                c.warm_step = Some(WarmStep { ctx: id, pipeline, left: chunks.len() as u32, refused: false, lost: false });
                for words in chunks {
                    let serial = c.serial();
                    let fence = c.device_fence();
                    let req = virgl::submit(Tag::Sync(serial), id, Some(fence), words);
                    if c.start_sync(serial, req, id, &internal, &[]).is_err() {
                        c.warm_returned(None);
                        continue;
                    }
                    if let Some(sync) = c.syncs.get_mut(&serial) {
                        sync.abandoned = true;
                        sync.warm = Some(fence);
                    }
                }
            }
        }
        if let Some(ctx) = c.contexts.get_mut(&id) {
            ctx.busy = false;
        }
        c.settle(id);
    });
    notify();
}

/// Whether thread group `tgid` has a row, other than `except`, that has not terminated.
fn group_live(m: &crate::process::ProcessManager, tgid: u64, except: Option<crate::process::ProcessId>) -> bool {
    m.iter_processes().any(|(row, p)| {
        Some(row) != except && p.thread_group_id.unwrap_or(row.as_u64()) == tgid && !p.is_terminated()
    })
}

/// Reclaim the GPU state of processes whose threads have exited (PIDs are not reused): a
/// backstop for the exit hooks below.
fn sweep() {
    let tgids: Vec<u64> = with_core(|c| c.procs.keys().copied().collect()).unwrap_or_default();
    if tgids.is_empty() {
        return;
    }
    let dead: Vec<u64> = crate::process::with_process_manager(|m| {
        tgids.into_iter().filter(|&tgid| tgid != 0 && !group_live(m, tgid, None)).collect()
    })
    .unwrap_or_default();
    if !dead.is_empty() {
        with_core(|c| {
            for tgid in dead {
                c.reclaim(tgid);
            }
        });
    }
}

/// Drop the GPU handles process `tgid` holds, without cancelling its work (section 1.16).
fn reclaim_process(tgid: u64) {
    let _path = GpuPath::enter();
    if with_core(|c| {
        let held = c.procs.contains_key(&tgid);
        c.reclaim(tgid);
        held
    })
    .unwrap_or(false)
    {
        notify();
    }
}

/// Decide reclamation while the process-manager lock still protects the exit commit.
/// The row has already terminated and has not yet been reaped.
pub fn exited_locked(m: &crate::process::ProcessManager, pid: crate::process::ProcessId) -> Option<u64> {
    let p = m.get_process(pid)?;
    let tgid = p.thread_group_id.unwrap_or(pid.as_u64());
    (!group_live(m, tgid, None)).then_some(tgid)
}

/// Redeem an exit or exec's captured GPU owner after releasing the process-manager lock.
pub fn owner_ended(tgid: u64) {
    reclaim_process(tgid);
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

/// Test point 2's hold: IRQs masked for `ns` nanoseconds, through the primitive the IRQ
/// gauge times.
fn hold_irqs_masked(ns: u64) {
    crate::arch_without_interrupts(|| {
        let start = now_ns();
        while now_ns().saturating_sub(start) < ns {
            core::hint::spin_loop();
        }
    });
}

/// `gpu(op, arg, len)`.
pub fn sys_gpu(op: u64, arg: u64, len: u64) -> SyscallResult {
    let _path = GpuPath::enter();
    // Test point 3, `emit-diagnostic`: this call writes BreenGL's diagnostic payload, the
    // only kernel output any syscall 416 path makes (section 1.16).
    if crate::test_control::take(test_point::EMIT_DIAGNOSTIC, 0).is_some() {
        crate::serial_println!("BREENGEL-EMIT-DIAGNOSTIC-PAYLOAD");
    }
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
                abi::op::UPLOAD => op_upload(arg, len),
                abi::op::SUBMIT => op_submit(arg, len),
                abi::op::WAIT => op_wait(arg, len),
                abi::op::READBACK => op_readback(arg, len),
                abi::op::PRESENT => display::present(arg, len),
                abi::op::COUNTERS => op_counters(arg, len),
                abi::op::SET_MODE => display::set_mode(arg, len),
                abi::op::MODES => display::modes(arg, len),
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
        scanout: false,
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
    let (handle, warmed) = create_context(tgid)?;
    a.handle = handle;
    a.warmed = warmed;
    if let Err(e) = write_arg(arg, &a) {
        destroy_context(tgid, handle);
        return Err(e);
    }
    Ok(0)
}

fn create_context(tgid: u64) -> Result<(u64, u32), i64> {
    let _creation = CreationGuard::acquire(tgid)?;
    sweep();
    if !ensure_reaper() {
        return Err(ENOMEM);
    }
    // Reserve the context and its internal resources.
    let vb_pages = Arc::new(Pages::new(encode::VERTEX_BUFFER_BYTES as usize / PAGE).ok_or(ENOMEM)?);
    let scratch_pages = Arc::new(Pages::new(1).ok_or(ENOMEM)?);
    let (id, vb, scratch, scratch_texture) = with_core(|c| {
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
        let scratch_texture = c.next_res + 2;
        c.next_res += 3;
        let epoch = c.epoch;
        c.insert_res(
            vb,
            new_res(
                0,
                kind::BUFFER,
                format::B8G8R8A8,
                0,
                0,
                encode::VERTEX_BUFFER_BYTES as u64,
                256,
                vb_pages.clone(),
                epoch,
            ),
        );
        c.insert_res(
            scratch,
            new_res(
                0,
                kind::TARGET,
                format::B8G8R8A8,
                1,
                1,
                0,
                256,
                scratch_pages.clone(),
                epoch,
            ),
        );
        // Device-only: no transfer names it, so it has no guest backing.
        c.insert_res(
            scratch_texture,
            new_res(
                0,
                kind::TEXTURE,
                format::B8G8R8A8,
                1,
                1,
                0,
                256,
                Arc::new(Pages::none()),
                epoch,
            ),
        );
        c.insert_ctx(
            id,
            Ctx {
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
                scratch_texture,
                warm: 0,
                warm_failures: 0,
                errors_in_row: 0,
                depth: None,
                flushes: present_answers::Answers::new(),
            },
        );
        Ok((id, vb, scratch, scratch_texture))
    })
    .ok_or(ENODEV)??;
    let created = create_device_context(
        tgid,
        id,
        [vb, scratch, scratch_texture],
        &vb_pages,
        &scratch_pages,
    )
    .and_then(|()| {
        with_core(|c| {
            // The watchdog or a reset may have ended the context while it was created.
            if !c
                .contexts
                .get(&id)
                .is_some_and(|x| x.state == CtxState::Creating && x.device_alive)
            {
                return Err(EIO);
            }
            let h = c.handle(tgid);
            c.proc_mut(tgid).contexts.insert(h, id);
            let ctx = c.contexts.get_mut(&id).ok_or(EIO)?;
            ctx.state = CtxState::Live;
            Ok((h, ctx.warmed()))
        })
        .unwrap_or(Err(ENODEV))
    });
    let (handle, warmed) = match created {
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
    Ok((handle, warmed))
}

/// Create device context `id` with its internal resources and the process's resources
/// attached, then send its fixed state and wait for its fence. The catalogue's programs
/// are warmed afterwards on `kgpu` (`warm_next`), and `probe_int` by the first PROBE. Each
/// wait here is a synchronous device request under the watchdog, ended by a kill (section
/// 1.4.1, program warm-up).
fn create_device_context(tgid: u64, id: u32, internal: [u32; 3], vb_pages: &Arc<Pages>, scratch_pages: &Arc<Pages>) -> Result<(), i64> {
    let [vb, scratch, scratch_texture] = internal;
    // Counted as alive from the request on, so a failed creation queues CTX_DESTROY
    // behind it whatever the device made of it.
    with_core(|c| {
        if let Some(ctx) = c.contexts.get_mut(&id) {
            ctx.device_alive = true;
        }
    });
    exec_ok(|t| Some(virgl::ctx_create(t, id)), id, &[])?;
    exec_ok(
        |t| {
            Some(virgl::resource_create(
                t,
                vb,
                wire::target::BUFFER,
                wire::format::R8_UNORM,
                wire::bind::VERTEX_BUFFER,
                encode::VERTEX_BUFFER_BYTES,
                1,
            ))
        },
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
    exec_ok(
        |t| {
            Some(virgl::resource_create(
                t,
                scratch_texture,
                wire::target::TEXTURE_2D,
                wire::format::B8G8R8A8_UNORM,
                wire::bind::SAMPLER_VIEW,
                1,
                1,
            ))
        },
        0,
        &[],
    )?;
    exec_ok(|t| Some(virgl::ctx_attach(t, id, scratch_texture)), id, &[])?;
    // The process's live resources, held so none is freed before it is attached.
    let theirs: Vec<u32> = with_core(|c| {
        let ids: Vec<u32> = c
            .procs
            .get(&tgid)
            .map(|p| {
                p.resources
                    .values()
                    .copied()
                    .filter(|rid| c.resources.get(rid).is_some_and(|r| !r.lost && !r.dropped))
                    .collect()
            })
            .unwrap_or_default();
        for rid in &ids {
            if let Some(r) = c.resources.get_mut(rid) {
                r.holds += 1;
            }
        }
        ids
    })
    .unwrap_or_default();
    let mut attached = Ok(());
    for &rid in &theirs {
        if attached.is_ok() {
            attached = exec_ok(|t| Some(virgl::ctx_attach(t, id, rid)), id, &[]).map(drop);
        }
    }
    with_core(|c| {
        for &rid in &theirs {
            c.unhold(rid);
        }
    });
    attached?;
    let mut objects = with_core(|c| c.contexts.get(&id).map(|ctx| ctx.objects.clone())).flatten().ok_or(EIO)?;
    let mut enc = encode::Enc::new();
    encode::warm_base(&mut enc, &mut objects, vb, scratch, wire::format::B8G8R8A8_UNORM);
    let chunks = enc.finish().filter(|v| !v.is_empty()).ok_or(ENOMEM)?;
    with_core(|c| if let Some(ctx) = c.contexts.get_mut(&id) { ctx.objects = objects; });
    for chunk in chunks {
        let (serial, fence) = with_core(|c| {
            if !c
                .contexts
                .get(&id)
                .is_some_and(|ctx| ctx.state == CtxState::Creating && ctx.device_alive)
            {
                return Err(EIO);
            }
            Ok((c.serial(), c.device_fence()))
        })
        .ok_or(ENODEV)??;
        let req = virgl::submit(Tag::Sync(serial), id, Some(fence), chunk);
        let d = exec_req(serial, req, id, &[vb, scratch, scratch_texture], &[])?;
        if !virgl::ok(d.answer) || d.echoed != fence {
            return Err(EIO);
        }
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
    if a.handle != 0 || a.reserved != [0; 3] || a.flags & !abi::resource_flag::SCANOUT != 0
        || (a.flags != 0 && a.kind != kind::TARGET)
    {
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
                    | if a.flags & abi::resource_flag::SCANOUT != 0 { wire::bind::SCANOUT } else { 0 }
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
    // Reserve against mappings while holding the same process lock as mmap/brk.
    // The Pages object carries the reservation until its last DMA hold ends.
    let charged_limits = crate::process::with_process_manager(|m| {
        let p = m.get_process(crate::process::ProcessId::new(tgid)).ok_or(EINVAL)?;
        if !p.reserve_gpu_bytes(charge) {
            return Err(ENOMEM);
        }
        Ok(p.limits.clone())
    }).ok_or(EINVAL)??;
    let Some(mut pages) = Pages::new(frames) else {
        charged_limits.release_gpu_bytes(charge);
        return Err(ENOMEM);
    };
    pages.charge_to(charged_limits);
    let (id, quota_charge) = with_core(|c| {
        let p = c.proc_mut(tgid);
        if p.resources.len() >= RESOURCE_QUOTA {
            return Err(EAGAIN);
        }
        let q = QuotaCharge::try_new(&p.quota, charge, false).ok_or(ENOMEM)?;
        let id = c.next_res;
        c.next_res += 1;
        Ok((id, q))
    }).ok_or(ENODEV)??;
    pages.charge_quota(quota_charge);
    let pages = Arc::new(pages);
    let width = if a.kind == kind::BUFFER { a.size as u32 } else { a.width };
    let height = if a.kind == kind::BUFFER { 1 } else { a.height };
    // The record exists, held by its creation and without a handle, from before the
    // device is asked to create it, so whatever happens the device's resource is unref'd.
    with_core(|c| {
        let mut r = new_res(tgid, a.kind, a.format, a.width, a.height, a.size, pitch, pages.clone(), c.epoch);
        r.scanout = a.flags & abi::resource_flag::SCANOUT != 0;
        r.holds = 1;
        c.insert_res(id, r);
    });
    let mut outcome = exec_ok(|t| Some(virgl::resource_create(t, id, vtarget, vformat, bind, width, height)), 0, &[])
        .map(drop);
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
        let _ = destroy_resource(tgid, a.handle);
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
    let retiring = destroy_resource(tgid, a.handle).ok_or(EINVAL)?;
    // With no work holding the resource, completion of DESTROY includes its unref:
    // the next allocation can reuse the released quota without undercounting DMA backing.
    // A handle destroyed while work holds it still returns immediately.
    if let Some(serial) = retiring {
        match wait_for(None, true, |c| match c.syncs.get(&serial) {
            None => Some(Ok(())),
            Some(s) if s.failed => Some(Err(EIO)),
            Some(_) => None,
        }) {
            Ok(result) => result?,
            Err(_) => return Err(EINTR),
        }
    }
    Ok(0)
}

/// Remove a resource handle and retire storage after its hold set drains (section 1.4.1).
/// An unused resource returns the unref receipt; a held resource has no receipt yet.
fn destroy_resource(tgid: u64, handle: u64) -> Option<Option<u64>> {
    with_core(|c| {
        let id = c.procs.get_mut(&tgid)?.resources.remove(&handle)?;
        if let Some(r) = c.resources.get_mut(&id) {
            r.dropped = true;
        }
        Some(c.retire_if_unused(id))
    })
    .flatten()
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

    /// The entries are in handle order (the process's map is), so a lookup is a binary search.
    fn get(&self, handle: u64) -> Option<&Entry> {
        self.entries.binary_search_by_key(&handle, |e| e.handle).ok().map(|i| &self.entries[i])
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
    /// The copy's charge to the process (section 1.16).
    _charge: Option<Charge>,
    /// The range the last lookup found: commands read their ranges in stream order.
    hint: Cell<usize>,
}

impl Snapshot {
    /// Copy every `DRAW_BUFFER` range of `stream` that lies inside a buffer the process
    /// owns. The copy stops at the first malformed command, where validation refuses the
    /// stream, and takes at most the instance limit's rectangles, which bounds every range
    /// validation reads.
    fn take(stream: &WordBuf, view: &View, tgid: u64) -> Result<Snapshot, i64> {
        let mut snap = Snapshot { ranges: Vec::new(), words: WordBuf::new(), _charge: None, hint: Cell::new(0) };
        let total = stream.len();
        let mut rects = 0u64;
        let mut at = 0usize;
        while total - at >= 2 {
            let code = stream.word(at);
            let len = stream.word(at + 1) as usize;
            if len < 2 || len > total - at {
                break;
            }
            if (code == stream::cmd::DRAW_BUFFER && len == 8)
                || (code == abi::draw::COMMAND && len >= abi::draw::HEADER && stream.word(at+2) == abi::pipeline::COVERAGE_SPANS) {
                let parameters = code == abi::draw::COMMAND;
                let hi = if parameters {6} else {4};
                let handle = stream.word(at + hi) as u64 | (stream.word(at + hi+1) as u64) << 32;
                let offset = if parameters {0} else {stream.word(at + 6) as u64};
                let count = if parameters {0} else {stream.word(at + 7) as u64};
                let bytes = if parameters {stream.word(at+8) as u64} else {16 * count};
                let fits = |e: &&Entry| offset % 16 == 0 && offset + bytes <= e.size && bytes <= limits::BUFFER_BYTES && bytes % 4 == 0 && rects + count <= limits::INSTANCES;
                if view.get(handle).filter(fits).and_then(|e| e.backing.as_ref()).is_some() {
                    snap.ranges.try_reserve(1).map_err(|_| ENOMEM)?;
                    snap.ranges.push(Range { handle, offset, bytes, first: 0 });
                    rects += count;
                }
            }
            at += len;
        }
        // Merge overlapping requests before copying, including repeated parameter buffers.
        snap.ranges.sort_unstable_by_key(|r| (r.handle, r.offset));
        let mut used = 0usize;
        for i in 0..snap.ranges.len() {
            if used > 0 && snap.ranges[used-1].handle == snap.ranges[i].handle
                && snap.ranges[i].offset <= snap.ranges[used-1].offset + snap.ranges[used-1].bytes {
                let end = (snap.ranges[i].offset + snap.ranges[i].bytes)
                    .max(snap.ranges[used-1].offset + snap.ranges[used-1].bytes);
                snap.ranges[used-1].bytes = end - snap.ranges[used-1].offset;
            } else {
                snap.ranges.swap(used, i);
                used += 1;
            }
        }
        snap.ranges.truncate(used);
        let total: u64 = snap.ranges.iter().map(|r| r.bytes).sum();
        if total > 0 {
            snap._charge = Some(Charge::new(tgid, total.div_ceil(PAGE as u64) * PAGE as u64, true)?);
        }
        let mut copied = 0u64;
        for r in &mut snap.ranges {
            copied = copied.checked_add(r.bytes).ok_or(EINVAL)?;
            if copied > limits::BUFFER_BYTES { return Err(EINVAL); }
            r.first = snap.words.len();
            let backing = view.get(r.handle).and_then(|e| e.backing.as_ref()).ok_or(EINVAL)?;
            let mut chunk = [0u8; 256];
            let mut done = 0u64;
            while done < r.bytes {
                let n = (r.bytes-done).min(chunk.len() as u64) as usize;
                backing.read((r.offset+done) as usize, &mut chunk[..n]);
                for w in chunk[..n].chunks_exact(4) {
                    if !snap.words.push(u32::from_le_bytes(w.try_into().unwrap())) { return Err(ENOMEM); }
                }
                done += n as u64;
            }
        }
        Ok(snap)
    }

    /// The range that may hold `offset` of buffer `handle`: the ranges are sorted by buffer
    /// and offset and do not overlap, so it is the last one starting at or before it.
    fn search(&self, handle: u64, offset: u64) -> Option<usize> {
        self.ranges.partition_point(|r| (r.handle, r.offset) <= (handle, offset)).checked_sub(1)
    }

    fn word(&self, handle: u64, offset: u64) -> u32 {
        let inside = |r: &Range| r.handle == handle && offset >= r.offset && offset+4 <= r.offset+r.bytes;
        let hint = self.hint.get();
        let found = self.ranges.get(hint).filter(|r| inside(r)).map(|_| hint)
            .or_else(|| self.search(handle, offset).filter(|&i| inside(&self.ranges[i])));
        let Some(i) = found else { return 0; };
        self.hint.set(i);
        let r = &self.ranges[i];
        self.words.word(r.first+((offset-r.offset)/4) as usize)
    }

    /// The rectangle at byte `offset` of buffer `handle`, from the copy.
    fn rect(&self, handle: u64, offset: u64) -> [i32; 4] {
        let inside = |r: &Range| r.handle == handle && offset >= r.offset && offset + 16 <= r.offset + r.bytes;
        let hint = self.hint.get();
        let found = [hint, hint + 1]
            .into_iter()
            .find(|&i| self.ranges.get(i).is_some_and(inside))
            .or_else(|| self.search(handle, offset).filter(|&i| inside(&self.ranges[i])));
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

/// The VirGL format of a BGL format.
fn virgl_format(f: u32) -> u32 {
    match f {
        format::R8G8B8A8 => wire::format::R8G8B8A8_UNORM,
        format::R8 => wire::format::R8_UNORM,
        _ => wire::format::B8G8R8A8_UNORM,
    }
}

impl Resolver for Resolved<'_> {
    fn target(&self, handle: u64) -> Option<TargetInfo> {
        let e = self.view.get(handle)?;
        (e.kind == kind::TARGET).then_some(TargetInfo { width: e.width, height: e.height, invalid: e.invalid, format: e.format })
    }

    fn source(&self, handle: u64) -> Option<stream::SourceInfo> {
        let e = self.view.get(handle)?;
        matches!(e.kind, kind::TARGET | kind::TEXTURE)
            .then_some(stream::SourceInfo { width: e.width, height: e.height, format: e.format, invalid: e.invalid })
    }

    fn buffer_size(&self, handle: u64) -> Option<u64> {
        let e = self.view.get(handle)?;
        (e.kind == kind::BUFFER).then_some(e.size)
    }

    fn buffer_rect(&self, handle: u64, offset: u64) -> [i32; 4] {
        self.snap.rect(handle, offset)
    }

    fn buffer_word(&self, handle:u64, offset:u64)->u32 { self.snap.word(handle,offset) }
}

impl encode::Bindings for Resolved<'_> {
    fn target(&self, handle: u64) -> Option<encode::TargetRes> {
        let e = self.view.get(handle)?;
        if e.kind != kind::TARGET {
            return None;
        }
        let info = TargetInfo { width: e.width, height: e.height, invalid: e.invalid, format: e.format };
        Some(encode::TargetRes { id: e.id, format: virgl_format(e.format), info })
    }

    fn source(&self, handle: u64) -> Option<encode::SourceRes> {
        let e = self.view.get(handle)?;
        matches!(e.kind, kind::TARGET | kind::TEXTURE)
            .then_some(encode::SourceRes { id: e.id, format: virgl_format(e.format), width: e.width, height: e.height })
    }

    fn buffer_rect(&self, handle: u64, offset: u64) -> [i32; 4] {
        self.snap.rect(handle, offset)
    }

    fn buffer_word(&self, handle:u64, offset:u64)->u32 { self.snap.word(handle,offset) }
}

/// The resources a validated stream names: written targets, every resource, full clears.
fn named(words: &WordBuf, view: &View) -> (Vec<u32>, Vec<u32>, Vec<u32>) {
    let mut written = Vec::new();
    let mut all = Vec::new();
    let mut whole = Vec::new();
    let mut target = None;
    let mut whole_clip = true;
    let mut size = [0u32; 2];
    let mut at = 0;
    while at < words.len() {
        let code = words.word(at);
        let len = words.word(at + 1) as usize;
        let handle = |i: usize| words.word(at + i) as u64 | (words.word(at + i + 1) as u64) << 32;
        let add = |list: &mut Vec<u32>, id: u32| {
            if !list.contains(&id) {
                list.push(id);
            }
        };
        let found = match code {
            stream::cmd::BEGIN => view.get(handle(2)).map(|e| e.id),
            stream::cmd::DRAW_BUFFER => view.get(handle(4)).map(|e| e.id),
            stream::cmd::DRAW_SAMPLED => view.get(handle(3)).map(|e| e.id),
            stream::cmd::COPY => view.get(handle(2)).map(|e| e.id),
            _ => None,
        };
        if code == stream::cmd::BEGIN {
            target = found;
            whole_clip = true;
            if let Some(e) = view.get(handle(2)) {
                size = [e.width, e.height];
            }
        } else if code == stream::cmd::SCISSOR {
            let r = [2, 3, 4, 5].map(|k| words.word(at + k) as i32);
            whole_clip = stream::clipped(
                [0, 0, size[0] as i32, size[1] as i32],
                size[0],
                size[1],
                Some(r),
            ) == Some([0, 0, size[0], size[1]]);
        } else if code == stream::cmd::CLEAR && whole_clip {
            if let Some(id) = target {
                add(&mut whole, id);
            }
        } else if code == stream::cmd::COPY {
            // A copy writes its destination, the whole of it when its clipped rectangle is.
            if let (Some(src), Some(dst)) = (view.get(handle(2)), view.get(handle(8))) {
                add(&mut written, dst.id);
                add(&mut all, dst.id);
                let r = [4, 5, 6, 7, 10, 11].map(|k| words.word(at + k));
                if let Some([_, _, w, h, dx, dy]) = stream::copy_clip(r, [src.width, src.height], [dst.width, dst.height]) {
                    if dx == 0 && dy == 0 && w == dst.width && h == dst.height {
                        add(&mut whole, dst.id);
                    }
                }
            }
        }
        if code == abi::draw::COMMAND {
            for k in [4,6] {
                if let Some(e)=view.get(handle(k)) {add(&mut all,e.id);}
            }
        }
        let writes = (code == abi::draw::COMMAND && words.word(at+9)>0)
            || code == stream::cmd::CLEAR
            || code == stream::cmd::PROBE
            || (code == stream::cmd::DRAW && words.word(at + 4) > 0)
            || (code == stream::cmd::DRAW_BUFFER && words.word(at + 7) > 0)
            || (code == stream::cmd::DRAW_SAMPLED && words.word(at + 5) > 0);
        if writes {
            if let Some(id) = target {
                add(&mut written, id);
            }
        }
        if let Some(id) = found {
            add(&mut all, id);
        }
        at += len.max(2);
    }
    (written, all, whole)
}

/// Read-only validator view for point 24's planted pipeline modifier.
struct ProfileWords<'a> {
    words: &'a WordBuf,
    filtered: Option<usize>,
}
impl stream::Words for ProfileWords<'_> {
    fn len(&self) -> usize {
        self.words.len()
    }
    fn word(&self, i: usize) -> u32 {
        self.words.word(i)
            | if self.filtered == Some(i) {
                0x8000_0000
            } else {
                0
            }
    }
}

fn reserve_profile_violation(
    words: &WordBuf,
) -> Option<(usize, u32, crate::test_control::Reservation<'static>)> {
    if !crate::test_control::enabled() {
        return None;
    }
    let (mut at, mut index) = (0usize, 0u32);
    while words.len() - at >= 2 {
        let code = words.word(at);
        let len = words.word(at + 1) as usize;
        if len < 2 || len > words.len() - at {
            return None;
        }
        if matches!(
            code,
            stream::cmd::DRAW | stream::cmd::DRAW_BUFFER | stream::cmd::DRAW_SAMPLED | abi::draw::COMMAND
        ) && len >= 3
        {
            if let Some(point) = crate::test_control::reserve(test_point::PROFILE_VIOLATION, 0) {
                return Some((at + 2, index, point));
            }
        }
        at += len;
        index += 1;
    }
    None
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
    // The stream's copy and the buffer snapshot are charged to the process while the call
    // holds them (section 1.16); a charge that cannot be made is ENOMEM, code 22 at 0.
    let pages = |bytes: u64| bytes.div_ceil(PAGE as u64) * PAGE as u64;
    let _stream_charge = match Charge::new(tgid, pages(a.length), true) {
        Ok(c) => c,
        Err(ENOMEM) => return refuse(&mut a, 0, stream::error::NO_MEMORY, ENOMEM),
        Err(e) => return Err(e),
    };
    let words = WordBuf::from_user(a.stream, a.length as usize)?;
    let view = with_core(|c| View::of(c, tgid)).ok_or(ENODEV)?;
    let snap = match Snapshot::take(&words, &view, tgid) {
        Ok(snap) => snap,
        Err(ENOMEM) => return refuse(&mut a, 0, stream::error::NO_MEMORY, ENOMEM),
        Err(e) => return Err(e),
    };
    let resolved = Resolved {
        view: &view,
        snap: &snap,
    };
    // Test point 2, `irq-hold`: this validation holds IRQs masked for 2 ms, through the
    // primitive the gauge times (section 1.16).
    if crate::test_control::take(test_point::IRQ_HOLD, 0).is_some() {
        hold_irqs_masked(2_000_000);
    }
    let injection = reserve_profile_violation(&words);
    let validation = ProfileWords {
        words: &words,
        filtered: injection.as_ref().map(|(word, _, _)| *word),
    };
    let plan = match stream::validate(&validation, &resolved) {
        Ok(plan) => plan,
        Err(r) => {
            if let Some((_, index, point)) = injection {
                if index == r.index && r.error == stream::error::FILTERED_SAMPLER {
                    point.fire();
                }
            }
            let errno = if r.is_work_limit() { EDQUOT } else if r.error == stream::error::NO_MEMORY { ENOMEM } else { EINVAL };
            return refuse(&mut a, r.index, r.error, errno);
        }
    };
    let (written, mut named_ids, whole) = named(&words, &view);
    // Take the context's submission slot: one sender at a time, at most four in flight.
    let mut waited = false;
    let admitted = wait_for(None, true, |c| {
        let Some(ctx) = c.contexts.get_mut(&id) else {
            return Some(Err(EIO));
        };
        if ctx.state != CtxState::Live || ctx.dropped {
            return Some(Err(EIO));
        }
        if written.iter().any(|rid| c.resources.get(rid).is_some_and(|r| r.reading)) {
            return None;
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
        for rid in &written {
            if let Some(r) = c.resources.get_mut(rid) {
                r.writers += 1;
            }
        }
        Some(Ok(()))
    });
    match admitted {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err(EINTR),
    }
    let release_slot = |accepted: bool| {
        with_core(|c| {
            if !accepted {
                for rid in &written {
                    if let Some(r) = c.resources.get_mut(rid) {
                        r.writers = r.writers.saturating_sub(1);
                    }
                    c.free_if_unused(*rid);
                }
            }
            if let Some(ctx) = c.contexts.get_mut(&id) {
                ctx.busy = false;
            }
            c.settle(id);
        });
        notify();
    };
    // The submission's own vertex buffer: no other submission writes it, and it is
    // freed when the submission no longer holds it.
    // Test point 22, `corrupt-pixel`, armed for one of the targets this submission writes
    // (or for any): that target gets one extra draw.
    let mut corruption = None;
    let corrupt = view.entries.iter()
        .filter(|e| written.contains(&e.id))
        .map(|e| e.handle)
        .find(|&handle| {
            corruption = crate::test_control::reserve(test_point::CORRUPT_PIXEL, handle);
            corruption.is_some()
        });
    let vertex_buffer = match vertex_buffer_for(tgid, encode::vertex_bytes(plan.drawn + u64::from(corrupt.is_some()))) {
        Ok(vb) => vb,
        Err(e) => {
            release_slot(false);
            return if e == ENOMEM { refuse(&mut a, 0, stream::error::NO_MEMORY, ENOMEM) } else { Err(e) };
        }
    };
    // The submission's scratch textures for copies within one target that overlap
    // (`docs/gui/gpu.md` section 1.3), one per target format, sized for its largest.
    let mut copy_scratch = [0u32; 2];
    for (slot, [w, h]) in plan.scratch.into_iter().enumerate() {
        if w == 0 || h == 0 {
            continue;
        }
        match scratch_for(w, h) {
            Ok(rid) => copy_scratch[slot] = rid,
            Err(e) => {
                with_core(|c| {
                    for rid in vertex_buffer.into_iter().chain(copy_scratch.into_iter().filter(|&r| r != 0)) {
                        c.unhold(rid);
                    }
                });
                release_slot(false);
                return Err(e);
            }
        }
    }
    let data_width = with_core(|c| c.caps.max_width).unwrap_or(0).min(16384);
    let data_width = 1u32 << (31-data_width.leading_zeros());
    let data_texture = if plan.data_bytes > 0 {
        match scratch_for(data_width, (plan.data_bytes/4).div_ceil(data_width)) {
            Ok(rid)=>rid,
            Err(e)=>{
                with_core(|c|for rid in vertex_buffer.into_iter().chain(copy_scratch.into_iter().filter(|&r|r!=0)){c.unhold(rid);});
                release_slot(false);return Err(e);
            }
        }
    } else {0};
    let extras: Vec<u32> = vertex_buffer.into_iter().chain(copy_scratch.into_iter().filter(|&r| r != 0)).chain((data_texture!=0).then_some(data_texture)).collect();
    let drop_vertex_buffer = || {
        with_core(|c| {
            for &rid in &extras {
                c.unhold(rid);
            }
        });
    };
    // The context's busy slot serializes object allocation; translation can allocate
    // and process a large stream without holding the global core lock.
    let state = with_core(|c| {
        c.contexts.get(&id).map(|ctx| {
            (
                ctx.objects.clone(),
                ctx.vb,
                ctx.scratch,
                ctx.scratch_texture,
            )
        })
    })
    .flatten();
    let Some((mut objects, default_vb, scratch, scratch_texture)) = state else {
        drop_vertex_buffer();
        release_slot(false);
        return Err(EIO);
    };
    let stale: Vec<u32> = objects.resources().filter(|&rid| rid != scratch && rid != scratch_texture && !view.has_id(rid)).collect();
    let mut suppression = None;
    let mut suppress = None;
    let mut swapping = None;
    let mut swap = None;
    let mut target = None;
    let mut at = 0;
    while at < words.len() {
        match words.word(at) {
            stream::cmd::BEGIN => {
                target = Some(words.word(at + 2) as u64 | (words.word(at + 3) as u64) << 32)
            }
            stream::cmd::DRAW | stream::cmd::DRAW_BUFFER if suppression.is_none() => {
                if let Some(handle) = target {
                    suppression = crate::test_control::reserve(test_point::SUPPRESS_DRAW, handle);
                    if suppression.is_some() { suppress = Some(handle); }
                }
            }
            // Test point 29, `wrong-texture`: the next DRAW_SAMPLED of the texture it is armed
            // for samples another of the process's live textures, the first created; a process
            // with one texture leaves the point armed.
            stream::cmd::DRAW_SAMPLED if swapping.is_none() && target.is_some() => {
                let named = words.word(at + 3) as u64 | (words.word(at + 4) as u64) << 32;
                let own = view.get(named).map(|e| e.id);
                let other = view.entries.iter()
                    .filter(|e| e.kind == kind::TEXTURE && Some(e.id) != own)
                    .min_by_key(|e| e.id);
                if let Some(e) = other {
                    swapping = crate::test_control::reserve(test_point::WRONG_TEXTURE, named);
                    if swapping.is_some() {
                        swap = Some((at, encode::SourceRes { id: e.id, format: virgl_format(e.format), width: e.width, height: e.height }));
                    }
                }
            }
            _ => {}
        }
        at += words.word(at + 1) as usize;
    }
    let translate = |enc: &mut encode::Enc, objects: &mut encode::Objects| {
        encode::forget_surfaces(enc, objects, &stale);
        encode::translate(enc, objects, &words, &resolved, encode::Extras {
            vertex_buffer: vertex_buffer.unwrap_or(default_vb),
            scratch: copy_scratch,
            data: data_texture,
            data_width,
            flip_y: FLIP_Y,
            corrupt,
            suppress,
            swap,
        });
    };
    let mut sizing = encode::Enc::size_only();
    translate(&mut sizing, &mut objects.clone());
    let chunk_charge = match sizing.backing_bytes().ok_or(ENOMEM)
        .and_then(|bytes| Charge::new(tgid, bytes, true)) {
        Ok(charge) => Some(charge),
        Err(e) => {
            drop_vertex_buffer();
            release_slot(false);
            return if e == ENOMEM { refuse(&mut a, 0, stream::error::NO_MEMORY, ENOMEM) } else { Err(e) };
        }
    };
    // Command frames and device requests come only after the quota reservation.
    let mut enc = encode::Enc::new();
    translate(&mut enc, &mut objects);
    let chunks = match enc.finish() {
        Some(chunks) => Some(chunks),
        None => {
            drop_vertex_buffer();
            release_slot(false);
            return refuse(&mut a, 0, stream::error::NO_MEMORY, ENOMEM);
        }
    };
    let made = (|| {
        if let Some(rid) = vertex_buffer {
            create_private(id, rid, wire::target::BUFFER, wire::format::R8_UNORM,
                wire::bind::VERTEX_BUFFER, encode::vertex_bytes(plan.drawn + u64::from(corrupt.is_some())) as u32, 1)?;
        }
        for ((rid, f), [w, h]) in copy_scratch.into_iter().zip([format::B8G8R8A8, format::R8G8B8A8]).zip(plan.scratch) {
            if rid != 0 {
                create_private(id, rid, wire::target::TEXTURE_2D, virgl_format(f),
                    wire::bind::RENDER_TARGET | wire::bind::SAMPLER_VIEW, w, h)?;
            }
        }
        if data_texture != 0 {
            create_private(id, data_texture, wire::target::TEXTURE_2D, wire::format::R8G8B8A8_UNORM,
                wire::bind::RENDER_TARGET | wire::bind::SAMPLER_VIEW, data_width, (plan.data_bytes / 4).div_ceil(data_width))?;
        }
        Ok::<(), i64>(())
    })();
    if let Err(e) = made {
        drop_vertex_buffer();
        release_slot(false);
        return Err(e);
    }
    // Point 29's texture is named, held and rechecked like the stream's own resources.
    if let Some((_, other)) = swap {
        if !named_ids.contains(&other.id) {
            named_ids.push(other.id);
        }
    }
    let mut held = named_ids.clone();
    held.extend(extras.iter().copied());
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
            && crate::test_control::take_if(test_point::WITHHOLD_FENCE, 0, |p| {
                p == test_point::WITHHOLD_DEPTH
            })
            .is_some()
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
        c.subs.insert(serial, Sub {
            ctx: id,
            n,
            fence,
            sent_ns: None,
            error: None,
            mismatch: false,
            requests: reqs.len() as u32,
            published: false,
            kept,
            result: None,
            written: written.clone(),
            whole,
            held,
            present: None,
            _charge: chunk_charge,
        });
        let epoch = c.epoch;
        c.backlog.extend(reqs.into_iter().map(|r| (epoch, r)));
        // No fallible preparation remains: these faults now affect a queued submission.
        if let Some(fault) = corruption { fault.fire(); }
        if let Some(fault) = suppression { fault.fire(); }
        if let Some(fault) = swapping { fault.fire(); }
        c.flush();
        Ok((ns as u64) << 32 | n as u64)
    })
    .unwrap_or(Err(ENODEV));
    release_slot(prepared.is_ok());
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
fn vertex_buffer_for(tgid: u64, bytes: u64) -> Result<Option<u32>, i64> {
    if bytes == 0 {
        return Ok(None);
    }
    u32::try_from(bytes).map_err(|_| ENOMEM)?;
    // The device's vertex storage is charged to the process until the buffer is freed.
    let charge = Charge::new(tgid, bytes, true)?;
    let mut backing = Pages::none();
    backing.charge_copy(charge);
    let id = with_core(|c| {
        let id = c.next_res;
        c.next_res += 1;
        let mut r = new_res(0, kind::BUFFER, format::B8G8R8A8, 0, 0, bytes, 0, Arc::new(backing), c.epoch);
        r.dropped = true;
        r.lost = true;
        r.holds = 1;
        c.insert_res(id, r);
        id
    })
    .ok_or(ENODEV)?;
    Ok(Some(id))
}

/// Reserve a private scratch-texture record; device creation follows quota admission.
/// It has no guest backing and remains held until the submission retires.
fn scratch_for(w: u32, h: u32) -> Result<u32, i64> {
    let id = with_core(|c| {
        let id = c.next_res;
        c.next_res += 1;
        let mut r = new_res(0, kind::TEXTURE, format::B8G8R8A8, w, h, 0, 0, Arc::new(Pages::none()), c.epoch);
        r.dropped = true;
        r.lost = true;
        r.holds = 1;
        c.insert_res(id, r);
        id
    })
    .ok_or(ENODEV)?;
    Ok(id)
}

/// Start a private resource only after the submission's full quota is reserved.
fn create_private(ctx: u32, id: u32, target: u32, vformat: u32, bind: u32, w: u32, h: u32) -> Result<(), i64> {
    with_core(|c| { if let Some(r) = c.resources.get_mut(&id) { r.lost = false; } });
    exec_ok(|t| Some(virgl::resource_create(t, id, target, vformat, bind, w, h)), 0, &[id])?;
    exec_ok(|t| Some(virgl::ctx_attach(t, ctx, id)), ctx, &[id])?;
    Ok(())
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

/// Source bytes read from the caller at a time by `UPLOAD`, on the stack.
const UPLOAD_CHUNK: usize = 768;

/// `UPLOAD` (`docs/gui/gpu-abi.md`): the caller's pixels into a texture, by a fenced
/// TRANSFER_TO_HOST_3D in the named context, or bytes into a buffer's backing.
fn op_upload(arg: u64, len: u64) -> Result<u64, i64> {
    let a: abi::Upload = read_arg(arg, len)?;
    if a.reserved != 0 || !matches!(a.purpose, abi::purpose::IMAGE | abi::purpose::FIXTURE) || a.width == 0 || a.height == 0 {
        return Err(EINVAL);
    }
    let tgid = caller()?;
    let (ctx, id, rkind, fmt, extent, pitch, backing) = with_core(|c| {
        let ctx = c.ctx_of(tgid, a.context).ok_or(EINVAL)?;
        let x = c.contexts.get(&ctx).ok_or(EINVAL)?;
        if x.state != CtxState::Live || !x.device_alive || x.dropped {
            return Err(EIO);
        }
        let id = c.res_of(tgid, a.resource).ok_or(EINVAL)?;
        let r = c.resources.get(&id).ok_or(EINVAL)?;
        if !abi::upload_source::fits(a.source, r.kind, r.format) {
            return Err(EINVAL);
        }
        if r.lost {
            return Err(EIO);
        }
        let extent = if r.kind == kind::BUFFER { [r.size, 1] } else { [r.width as u64, r.height as u64] };
        Ok((ctx, id, r.kind, r.format, extent, r.pitch as u64, r.backing.clone()))
    })
    .ok_or(ENODEV)??;
    if rkind == kind::BUFFER && (a.y != 0 || a.height != 1) {
        return Err(EINVAL);
    }
    // Clip to the resource; pixels that land outside it are neither read nor written.
    let (x, y) = (a.x as u64, a.y as u64);
    if x >= extent[0] || y >= extent[1] {
        return Err(EINVAL);
    }
    let cw = (a.width as u64).min(extent[0] - x);
    let ch = (a.height as u64).min(extent[1] - y);
    let bpp = abi::upload_source::bytes(a.source) as u64;
    let stride = a.stride as u64;
    let row = cw * bpp;
    if stride < row || a.pixels_len < (ch - 1) * stride + row {
        return Err(EINVAL);
    }
    if !crate::memory::layout::is_valid_user_range(a.pixels, ((ch - 1) * stride + row) as usize) {
        return Err(EFAULT);
    }
    let count = |c: &mut Core| {
        let n = cw * ch * bpp;
        let add = |t: &mut Totals| {
            if a.purpose == abi::purpose::FIXTURE {
                t.uploads_fixture += n;
            } else {
                t.uploads_image += n;
            }
        };
        if let Some(x) = c.contexts.get_mut(&ctx) {
            add(&mut x.totals);
        }
        add(&mut c.proc_mut(tgid).totals);
    };
    // Stage the clipped source rows before changing any backing or sending a request.
    // Frames count against both guest-backing quota and RLIMIT_AS for this call.
    let stage = Staging::new(tgid, (row * ch) as usize)?;
    let mut input = [0u8; UPLOAD_CHUNK];
    for j in 0..ch {
        let mut done = 0usize;
        while done < row as usize {
            let n = (row as usize - done).min(UPLOAD_CHUNK);
            crate::syscall::userptr::read_user_bytes(input.as_mut_ptr(), a.pixels + j * stride + done as u64, n)
                .map_err(|_| EFAULT)?;
            stage.pages.write(j as usize * row as usize + done, &input[..n]);
            done += n;
        }
    }
    // The backing is the upload's alone until its transfer has ended. Submissions of other
    // contexts that name the texture finish first: the device states no order between
    // contexts. This context's own earlier commands are ahead of the transfer in its queue.
    let taken = wait_for(None, true, |c| {
        if c.res_of(tgid, a.resource) != Some(id) {
            return Some(Err(EINVAL));
        }
        let r = c.resources.get(&id)?;
        if r.reading || r.writers > 0 || c.subs.values().any(|s| s.ctx != ctx && s.held.contains(&id)) {
            return None;
        }
        c.resources.get_mut(&id)?.reading = true;
        Some(Ok(()))
    });
    match taken {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err(EINTR),
    }
    let release = || {
        with_core(|c| {
            if !c.syncs.values().any(|s| s.reading.contains(&id)) {
                if let Some(r) = c.resources.get_mut(&id) {
                    r.reading = false;
                }
                c.free_if_unused(id);
            }
        });
        notify();
    };
    if rkind == kind::BUFFER {
        let mut chunk = [0u8; STAGING_CHUNK];
        let mut done = 0usize;
        while done < cw as usize {
            let n = (cw as usize - done).min(STAGING_CHUNK);
            stage.pages.read(done, &mut chunk[..n]);
            backing.write(x as usize + done, &chunk[..n]);
            done += n;
        }
        with_core(count);
        release();
        return Ok(0);
    }
    // Rows into the backing at its pitch, converted into the texture's format by channel.
    let tbpp = format::bytes(fmt) as u64;
    let per = (UPLOAD_CHUNK as u64 / bpp).min(cw);
    let mut output = [0u8; UPLOAD_CHUNK * 4 / 3 + 4];
    for j in 0..ch {
        let mut px = 0u64;
        while px < cw {
            let n = per.min(cw - px);
            stage.pages.read((j * row + px * bpp) as usize, &mut input[..(n * bpp) as usize]);
            for k in 0..n as usize {
                let at = k * bpp as usize;
                if fmt == format::R8 {
                    output[k] = input[at];
                } else {
                    let [b, g, r, al] = abi::upload_source::channels(a.source, &input[at..]);
                    let out = if fmt == format::R8G8B8A8 { [r, g, b, al] } else { [b, g, r, al] };
                    output[k * 4..k * 4 + 4].copy_from_slice(&out);
                }
            }
            backing.write(((y + j) * pitch + (x + px) * tbpp) as usize, &output[..(n * tbpp) as usize]);
            px += n;
        }
    }
    let offset = y * pitch + x * tbpp;
    backing.maintain(offset as usize, ((ch - 1) * pitch + cw * tbpp) as usize, false);
    // Test point 30, `skip-upload-visibility`: the transfer is not sent; the upload returns
    // as a completed one.
    let skip = crate::test_control::take(test_point::SKIP_UPLOAD_VISIBILITY, a.resource).is_some();
    let sent = if skip {
        with_core(|c| (c.serial(), 0, wire::resp::OK_NODATA)).ok_or((ENODEV, 0))
    } else {
        match with_core(|c| (c.serial(), c.device_fence())) {
            None => Err((ENODEV, 0)),
            Some((serial, fence)) => {
                let req = virgl::transfer_to_host(Tag::Sync(serial), ctx, fence, id, backing.clone(), x as u32, y as u32, cw as u32, ch as u32, offset, pitch as u32);
                match exec_req_why(serial, req, ctx, &[id], &[id]) {
                    Ok(d) if virgl::ok(d.answer) && d.echoed == fence && d.resp.u32_at(wire::HDR_FLAGS) & wire::FLAG_FENCE != 0 => {
                        Ok((serial, fence, d.answer))
                    }
                    Ok(_) => Err((EIO, serial)),
                    Err((e @ (EIO | EINTR), _)) => Err((e, serial)),
                    Err((e, _)) => Err((e, 0)),
                }
            }
        }
    };
    // The upload's result is published before its `reading` mark is released: the
    // device-access hold lasts until the fence is published (section 1.4.1, hold sets).
    let whole = cw == extent[0] && ch == extent[1];
    let result = match sent {
        Ok((serial, fence, answer)) => {
            with_core(|c| {
                if let Some(r) = c.resources.get_mut(&id) {
                    completion::finish_write(&mut r.invalid, &mut r.invalidated_by, serial, true, whole);
                    r.generation += 1;
                    r.last = Some(LastWrite { ctx, fence, serial, answer, kind: wire::cmd::TRANSFER_TO_HOST_3D });
                }
                count(c);
            });
            Ok(0)
        }
        Err((e, serial)) => {
            // A transfer refused, interrupted after queueing, or ended by a watchdog/reset leaves the
            // texture's contents unknown: it is invalid until a whole upload completes OK.
            if serial != 0 {
                with_core(|c| {
                    if let Some(r) = c.resources.get_mut(&id) {
                        completion::finish_write(&mut r.invalid, &mut r.invalidated_by, serial, false, whole);
                    }
                });
            }
            Err(e)
        }
    };
    release();
    result
}

fn op_readback(arg: u64, len: u64) -> Result<u64, i64> {
    let mut a: abi::Readback = read_arg(arg, len)?;
    if a.reason != 0 || a.failed_submission != 0 {
        return Err(EINVAL);
    }
    let tgid = caller()?;
    let _scanout = if a.target == 0 {
        if !display::owner(tgid) {
            return Err(abi::errno::EPERM);
        }
        wait_for(None, true, |c| {
            (!console::restoring()).then_some(c.display.known)
        })
        .map_err(|_| EINTR)?;
        Some(display::Transition::for_observation(tgid)?)
    } else {
        None
    };
    if a.target == 0
        && !with_core(|c| c.display.known && c.display.binding.is_some()).unwrap_or(false)
    {
        a.reason = readback_reason::BINDING_UNKNOWN;
        write_arg(arg, &a)?;
        return Err(EIO);
    }
    let (id, fmt, tw, th, pitch, backing) = with_core(|c| {
        let id = if a.target == 0 {
            if !display::owner(tgid) {
                return Err(abi::errno::EPERM);
            }
            c.display.binding.ok_or(EIO)?.id
        } else {
            c.res_of(tgid, a.target).ok_or(EINVAL)?
        };
        let r = c.resources.get(&id).ok_or(EINVAL)?;
        if !matches!(r.kind, kind::TARGET | kind::TEXTURE) {
            return Err(EINVAL);
        }
        Ok((id, r.format, r.width, r.height, r.pitch, r.backing.clone()))
    })
    .ok_or(ENODEV)??;
    let (x, y, w, h) = (a.x as u64, a.y as u64, a.width as u64, a.height as u64);
    if w == 0 || h == 0 || x + w > tw as u64 || y + h > th as u64 {
        return Err(EINVAL);
    }
    // Canonical BGRA, or one byte per pixel of an R8 texture (record `canonical` 1).
    let row = w * format::bytes(fmt) as u64;
    if (a.stride as u64) < row || a.pixels_len < (h - 1) * a.stride as u64 + row {
        return Err(EINVAL);
    }
    // The argument, record and pixel-row pages are checked for mapping and writability
    // before any device work, serial or count (gpu-abi.md, READBACK's EFAULT).
    probe_writable(arg, core::mem::size_of::<abi::Readback>())?;
    probe_writable(a.record, record::SIZE)?;
    // The rows' span is a user range, so the row arithmetic below cannot overflow; only the
    // rows' own pages are probed, excluding the gaps a wider stride leaves between them.
    if !crate::memory::layout::is_valid_user_range(
        a.pixels,
        ((h - 1) * a.stride as u64 + row) as usize,
    ) {
        return Err(EFAULT);
    }
    let mut probed = u64::MAX;
    for ry in 0..h {
        let start = a.pixels + ry * a.stride as u64;
        let first = start / PAGE as u64;
        let from = if first == probed {
            (first + 1) * PAGE as u64
        } else {
            start
        };
        if from < start + row {
            probe_writable(from, (start + row - from) as usize)?;
        }
        probed = (start + row - 1) / PAGE as u64;
    }
    let refuse = |a: &mut abi::Readback, reason: u32, failed: u64| -> Result<u64, i64> {
        a.reason = reason;
        a.failed_submission = failed;
        write_arg(arg, a)?;
        Err(EIO)
    };
    match with_core(|c| display::readable(c, tgid, id)).ok_or(ENODEV)? {
        Ok(()) => {}
        Err(EIO) => return refuse(&mut a, readback_reason::BINDING_UNKNOWN, 0),
        Err(e) => return Err(e),
    }
    let staging = Staging::new(tgid, (row * h) as usize)?;
    // Wait for the target's accepted writes to complete, then take it for reading.
    // The handle is looked up each time: one destroyed meanwhile is target-dropped.
    let taken = wait_for(None, true, |c| {
        if a.target == 0 {
            if !display::owner(tgid)
                || !c.display.known
                || c.display.binding.is_none_or(|b| b.id != id)
            {
                return Some(Err(readback_reason::BINDING_UNKNOWN));
            }
        } else if c.res_of(tgid, a.target) != Some(id) {
            return Some(Err(readback_reason::TARGET_DROPPED));
        }
        let r = c.resources.get_mut(&id)?;
        if r.lost {
            return Some(Err(readback_reason::TARGET_LOST));
        }
        if r.dropped && a.target != 0 {
            return Some(Err(readback_reason::TARGET_DROPPED));
        }
        if r.writers > 0 || r.reading {
            return None;
        }
        r.reading = true;
        Some(Ok((r.invalid, r.invalidated_by, r.generation, r.last)))
    });
    let (invalid, invalidated_by, generation, last) = match taken {
        Ok(Ok(t)) => t,
        Ok(Err(reason)) => return refuse(&mut a, reason, 0),
        Err(_) => return Err(EINTR),
    };
    // The target is ours to poison and read until every transfer into it has ended; a
    // transfer whose waiter left keeps the mark until the device returns it.
    let release = || {
        with_core(|c| {
            if !c.syncs.values().any(|s| s.reading.contains(&id)) {
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
        return refuse(&mut a, readback_reason::TARGET_INVALID, invalidated_by);
    }
    let observation = match observe(tgid, id, last, &backing, pitch, fmt, a, &staging.pages) {
        Ok(o) => o,
        Err((errno, reason)) => {
            release();
            return if errno == EIO {
                refuse(&mut a, reason, 0)
            } else {
                Err(errno)
            };
        }
    };
    let (unwritten, transfer_fence, transfer_answer) =
        (observation.unwritten, observation.fence, observation.answer);
    let mut rec = [0u8; record::SIZE];
    let put32 = |rec: &mut [u8; record::SIZE], at: usize, v: u32| {
        rec[at..at + 4].copy_from_slice(&v.to_le_bytes())
    };
    let put64 = |rec: &mut [u8; record::SIZE], at: usize, v: u64| {
        rec[at..at + 8].copy_from_slice(&v.to_le_bytes())
    };
    let made = with_core(|c| {
        // The binding is read with the serial: a PRESENT sent during the observation
        // leaves the scanout unknown, and the observation is refused naming that.
        let binding = display::binding(c, tgid, id)?;
        let s = c.next_obs;
        c.next_obs += 1;
        Ok((s, c.caps.renderer, binding))
    })
    .ok_or(ENODEV)?;
    let (serial, renderer, binding) = match made {
        Ok(m) => m,
        Err(e) => {
            release();
            return if e == EIO {
                refuse(&mut a, readback_reason::BINDING_UNKNOWN, 0)
            } else {
                Err(e)
            };
        }
    };
    put32(&mut rec, record::VERSION_OFF, record::VERSION);
    put32(&mut rec, record::SIZE_OFF, record::SIZE as u32);
    put64(&mut rec, record::SERIAL, serial);
    put32(&mut rec, record::BACKEND, abi::backend::VIRGL);
    put32(&mut rec, record::BOUNDARY, record::BOUNDARY_HOST);
    if let Some(b) = binding {
        put32(&mut rec, record::FLAGS, 1);
        put64(&mut rec, record::PRESENT_FENCE, b.fence);
        put32(&mut rec, record::PRESENT_STATUS, b.answer);
    }
    put32(&mut rec, record::FORMAT, fmt);
    if fmt == format::R8 {
        put32(&mut rec, record::CANONICAL, record::CANONICAL_R8);
    }
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
        put32(&mut rec, record::RENDER_KIND, l.kind);
    }
    put64(&mut rec, record::READBACK_FENCE, transfer_fence);
    put32(&mut rec, record::READBACK_STATUS, transfer_answer);
    put32(
        &mut rec,
        record::READBACK_KIND,
        wire::cmd::TRANSFER_FROM_HOST_3D,
    );
    put32(&mut rec, record::POISON, POISON);
    put32(&mut rec, record::UNWRITTEN, unwritten);
    rec[record::RENDERER..record::RENDERER + 64].copy_from_slice(&renderer);
    // Publish pixels after the complete rectangle passed both observations.
    let output = (|| {
        if unwritten == 0 {
            let row = row as usize;
            for ry in 0..h as usize {
                let dst = a.pixels + ry as u64 * a.stride as u64;
                staging.pages.try_spans(ry * row, row, |done, ptr, n| {
                    crate::syscall::userptr::write_user_bytes(dst + done as u64, ptr, n)
                        .map_err(|_| EFAULT)
                })?;
            }
        }
        crate::syscall::userptr::write_user_bytes(a.record, rec.as_ptr(), record::SIZE)
            .map_err(|_| EFAULT)
    })();
    release();
    output?;
    if unwritten != 0 {
        return refuse(&mut a, readback_reason::UNWRITTEN, 0);
    }
    with_core(|c| {
        if let Some(l) = last {
            if let Some(ctx) = c.contexts.get_mut(&l.ctx) {
                ctx.totals.readbacks += 1;
            }
        }
        c.proc_mut(tgid).totals.readbacks += 1;
    });
    Ok(0)
}

/// Fail with EFAULT unless the pages of `[addr, addr + len)` are mapped and writable by
/// the caller. Each page is tested with an atomic compare-and-exchange of one of its words
/// with the value it holds, through the recoverable user-access routine: a read-only or
/// unmapped page faults, a copy-on-write page is copied, and no byte the caller's other
/// threads store meanwhile is lost, because the exchange only stores the value it read and
/// retries when the word changed.
fn probe_writable(addr: u64, len: usize) -> Result<(), i64> {
    if len == 0 || !crate::memory::layout::is_valid_user_range(addr, len) {
        return Err(EFAULT);
    }
    let end = addr.checked_add(len as u64).ok_or(EFAULT)?;
    let mut at = addr;
    while at < end {
        if crate::syscall::check_signals_for_wait().is_some() {
            return Err(EINTR);
        }
        let word = at & !3;
        let mut seen = crate::syscall::userptr::read_user_u32(word).map_err(|_| EFAULT)?;
        loop {
            match crate::syscall::userptr::cmpxchg_user_u32(word, seen, seen) {
                Ok(v) if v == seen => break,
                Ok(v) => seen = v,
                Err(_) => return Err(EFAULT),
            }
        }
        at = (at | (PAGE as u64 - 1)).saturating_add(1);
    }
    Ok(())
}

/// Bytes moved through the stack at a time between staging, device backing and the caller.
const STAGING_CHUNK: usize = 512;


/// The bytes that differ between two observations of the same span.
fn differing(a: &[u8], b: &[u8]) -> u32 {
    let (wa, wb) = (a.chunks_exact(8), b.chunks_exact(8));
    let tail = wa.remainder().iter().zip(wb.remainder()).filter(|(p, q)| p != q).count() as u32;
    wa.zip(wb)
        .map(|(x, y)| {
            let d = u64::from_le_bytes(x.try_into().unwrap_or_default())
                ^ u64::from_le_bytes(y.try_into().unwrap_or_default());
            d.to_le_bytes().iter().filter(|&&b| b != 0).count() as u32
        })
        .sum::<u32>()
        + tail
}

struct Observation {
    unwritten: u32,
    fence: u64,
    answer: u32,
}

/// Test point 21's resource: another live target of the process, idle, with the same row
/// pitch, that contains the transfer's box, so the box sent to the device stays
/// within the resource it names (section 1.4.1). It is marked as read for the transfer, so
/// no writer or other readback uses its backing meanwhile, and the request holds the
/// backing the device writes. No matching target leaves the point armed.
fn wrong_resource(tgid: u64, id: u32, pitch: u32, x: u32, y: u32, w: u32, h: u32, target: u64) -> Option<(u32, Arc<Pages>)> {
    with_core(|c| {
        let (&rid, _) = c.resources.iter().find(|(rid, r)| {
            **rid != id && r.owner == tgid && r.kind == kind::TARGET && !r.dropped && !r.lost
                && !r.reading && r.writers == 0 && r.pitch == pitch
                && r.width >= x + w && r.height >= y + h
        })?;
        crate::test_control::take(test_point::WRONG_RESOURCE, target)?;
        let r = c.resources.get_mut(&rid)?;
        r.reading = true;
        Some((rid, r.backing.clone()))
    })
    .flatten()
}

/// Read a rectangle of target `id` into `staging`, band by band: each band is transferred
/// twice, into the poison and into its complement, and the bytes that differ between the
/// two are counted as unwritten (section 1.8). A transfer is a synchronous request
/// under the watchdog that holds the resource it names and the backing the device writes,
/// until the device returns it. Returns that count and the first transfer's fence and
/// answer. The reason of an EIO comes only from what the kernel observes.
#[allow(clippy::too_many_arguments)]
fn observe(
    tgid: u64,
    id: u32,
    last: Option<LastWrite>,
    backing: &Arc<Pages>,
    pitch: u32,
    fmt: u32,
    a: abi::Readback,
    staging: &Pages,
) -> Result<Observation, (i64, u32)> {
    let ctx = with_core(|c| {
        let live = |cid: &u32| {
            c.contexts.get(cid).is_some_and(|x| {
                (a.target == 0 || x.owner == tgid) && x.state == CtxState::Live && x.device_alive
            })
        };
        last.map(|l| l.ctx).filter(live).or_else(|| {
            if a.target == 0 {
                None
            } else {
                c.procs
                    .get(&tgid)
                    .and_then(|p| p.contexts.values().copied().find(live))
            }
        })
    })
    .flatten()
    .ok_or(if a.target == 0 {
        (EIO, readback_reason::TARGET_LOST)
    } else {
        (EINVAL, readback_reason::NONE)
    })?;
    let px = format::bytes(fmt) as usize;
    let row = a.width as usize * px;
    let band_rows = (READBACK_BAND / pitch as usize)
        .max(1)
        .min(a.height as usize);
    let mut seen = [0u8; PAGE];
    let mut chunk = [0u8; STAGING_CHUNK];
    let mut unwritten = 0u32;
    let mut first: Option<(u64, u32)> = None;
    let mut by = 0usize;
    while by < a.height as usize {
        let rows = band_rows.min(a.height as usize - by);
        let ty = a.y as usize + by;
        let offset = ty * pitch as usize + a.x as usize * px;
        for (pass, pattern) in [POISON, !POISON].into_iter().enumerate() {
            for r in 0..rows {
                backing.fill(offset + r * pitch as usize, row, pattern);
            }
            backing.maintain(offset, (rows - 1) * pitch as usize + row, false);
            let skip = crate::test_control::take(test_point::SKIP_TRANSFER, a.target).is_some();
            if !skip {
                let other = wrong_resource(
                    tgid,
                    id,
                    pitch,
                    a.x,
                    ty as u32,
                    a.width,
                    rows as u32,
                    a.target,
                );
                let (named, written) = other.clone().unwrap_or((id, backing.clone()));
                let (serial, fence) = with_core(|c| (c.serial(), c.device_fence()))
                    .ok_or((ENODEV, readback_reason::NONE))?;
                let req = virgl::transfer_from_host(
                    Tag::Sync(serial),
                    ctx,
                    fence,
                    named,
                    written,
                    a.x,
                    ty as u32,
                    a.width,
                    rows as u32,
                    offset as u64,
                    pitch,
                );
                // The target, and the other resource when the transfer names one.
                let both = [id, named];
                let marks = if named == id { &both[..1] } else { &both[..] };
                let done = exec_req_why(serial, req, ctx, marks, marks);
                if let Some((rid, _)) = other {
                    // The mark stays with the request if its waiter left before the answer.
                    with_core(|c| {
                        if !c.syncs.values().any(|s| s.reading.contains(&rid)) {
                            if let Some(r) = c.resources.get_mut(&rid) {
                                r.reading = false;
                            }
                            c.free_if_unused(rid);
                        }
                    });
                    notify();
                }
                let mut d = done.map_err(|(e, status)| match status {
                    fence_status::DEVICE_HUNG => (e, readback_reason::DEVICE_HUNG),
                    fence_status::DEVICE_RESET => (e, readback_reason::DEVICE_RESET),
                    _ => (e, readback_reason::DEVICE_ERROR),
                })?;
                // Test point 20, `stale-fence`: the answer is taken as echoing the next fence.
                if crate::test_control::take(test_point::STALE_FENCE, a.target).is_some() {
                    d.echoed = d.echoed.wrapping_add(1);
                }
                if !virgl::ok(d.answer) {
                    return Err((EIO, readback_reason::DEVICE_ERROR));
                }
                if d.echoed != fence || d.resp.u32_at(wire::HDR_FLAGS) & wire::FLAG_FENCE == 0 {
                    return Err((EIO, readback_reason::FENCE_MISMATCH));
                }
                first.get_or_insert((fence, d.answer));
            }
            // The first observation lands straight in the staging; the second is compared
            // with it a frame at a time.
            for r in 0..rows {
                let src = offset + r * pitch as usize;
                let _ = staging.try_spans((by + r) * row, row, |done, ptr, n| {
                    // SAFETY: the span lies inside one of the staging's frames, which only
                    // this call uses.
                    let kept = unsafe { core::slice::from_raw_parts_mut(ptr, n) };
                    if pass == 0 {
                        observation::read(backing.as_ref(), src + done, kept);
                    } else {
                        observation::read(backing.as_ref(), src + done, &mut seen[..n]);
                        unwritten += differing(&seen[..n], kept);
                    }
                    Ok::<(), ()>(())
                });
            }
        }
        if fmt == format::R8G8B8A8 {
            // Canonical bytes are BGRA (record v2's `canonical` 0).
            let bytes = rows * row;
            let mut done = 0;
            while done < bytes {
                let n = (bytes - done).min(STAGING_CHUNK);
                let at = by * row + done;
                staging.read(at, &mut chunk[..n]);
                for px in chunk[..n].chunks_exact_mut(4) {
                    px.swap(0, 2);
                }
                staging.write(at, &chunk[..n]);
                done += n;
            }
        }
        by += rows;
    }
    let (fence, answer) = first.unwrap_or((0, 0));
    Ok(Observation {
        unwritten,
        fence,
        answer,
    })
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
            out.uploads_image = t.uploads_image;
            out.uploads_fixture = t.uploads_fixture;
        };
        let mut out = abi::Counters::ZERO;
        out.context = a.context;
        if a.context == 0 {
            if let Some(p) = proc_live {
                fill(&mut out, &p.totals);
                out.contexts = p.contexts.len() as u64;
                out.resources = p.resources.len() as u64;
                let (held, copies) = (p.quota.held.load(Ordering::Acquire), p.quota.copies.load(Ordering::Acquire));
                out.resource_bytes = held.saturating_sub(copies);
                out.copy_bytes = copies;
            }
        } else {
            let id = c.ctx_of(tgid, a.context)?;
            let ctx = c.contexts.get(&id)?;
            fill(&mut out, &ctx.totals);
            out.state = match ctx.state {
                CtxState::Quarantined => abi::context_state::QUARANTINED,
                CtxState::Reset => abi::context_state::RESET,
                _ if ctx.warm == encode::ALL_WARM => abi::context_state::LIVE,
                _ => abi::context_state::WARMING,
            };
            out.warmed = ctx.warmed() as u64;
            if let Some(p) = proc_live {
                out.contexts = p.contexts.len() as u64;
                out.resources = p.resources.len() as u64;
                let (held, copies) = (p.quota.held.load(Ordering::Acquire), p.quota.copies.load(Ordering::Acquire));
                out.resource_bytes = held.saturating_sub(copies);
                out.copy_bytes = copies;
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

/// Whether the clean-room device owns this ARM64 display path.
pub fn display_active() -> bool {
    display::active()
}
pub fn take_display() -> SyscallResult {
    let _path = GpuPath::enter();
    let tid = match crate::task::scheduler::current_thread_id() {
        Some(t) => t,
        None => return result(Err(EINVAL)),
    };
    let identity = crate::process::with_process_manager(|m| {
        m.find_process_by_thread(tid).map(|(pid, p)| {
            (
                pid.as_u64(),
                p.thread_group_id.unwrap_or(pid.as_u64()),
                p.cred.euid,
            )
        })
    })
    .flatten();
    let Some((pid, group, uid)) = identity else {
        return result(Err(EINVAL));
    };
    if uid != 0 {
        return result(Err(abi::errno::EPERM));
    }
    let outcome = display::take(group, pid);
    if outcome.is_ok() {
        crate::process::with_process_manager(|m| {
            if let Some(p) = m.get_process_mut(crate::process::ProcessId::new(pid)) {
                p.has_display_ownership = true;
            }
        });
    }
    result(outcome.map(|()| 0))
}
pub fn give_display() -> SyscallResult {
    let _path = GpuPath::enter();
    result(caller().and_then(display::back).map(|()| 0))
}
