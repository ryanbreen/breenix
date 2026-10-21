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
//! after a process-local grace for successful creations, so other requests wait behind
//! at most one step. A step also waits, for at most that grace, for a pause in other
//! requests, so a burst of requests, each sent when the one before it is answered, is
//! split by a step at most once a grace rather than after each request. A draw whose
//! pipeline has not warmed creates its programs in its own submission and waits for no
//! other pipeline (ruling 33, #1725).
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

pub mod window_targets;
mod completion;
mod present_answers;
mod display;
pub mod console;
pub mod panic;
pub mod screen;
mod observation;
mod quota;
mod staging;
mod vertex_arena;
use quota::{Charge, Quota, QuotaCharge};
use staging::Staging;
pub mod dma;
pub mod dev;
pub mod virgl;
pub mod svga;

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
use crate::task::waitqueue::{PrepareOutcome, WaitQueueHead};
use dma::{Pages, WordBuf, PAGE};
use dev::{Device, Done, Reap, Request, Tag};
use virgl::{encode, wire};

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
/// How often kgpu looks for an SVGA3 device's answers while it holds requests.
const POLL_NS: u64 = 250_000;
/// How long a waiter looks for its answer itself, on a device that raises no interrupt,
/// before it sleeps until kgpu's next look. kgpu's timed block ends at the next timer
/// tick, so its 250 us looks came about 0.8 ms late on average, once per request: 5.6 s
/// of the 950-glyph corpus's 9.1 s on VMware (#1980). After a tick, kgpu's look is as
/// timely as the waiter's.
const SPIN_POLL_NS: u64 = 1_000_000;
/// Spin hints between a polling waiter's looks, to leave the core's lock to others.
const SPIN_POLL_PAUSE: u32 = 64;
/// Initial context grace before catalogue warm-up. Resource creation does not extend it.
/// It also bounds how long a warm-up step is held for other requests (`WARM_QUIET_NS`).
const WARM_GRACE_NS: u64 = 20_000_000;
/// Quiet interval after other requests are sent or answered. Traffic between warm-up
/// steps also spaces their starts by at least `WARM_GRACE_NS` (#2103, #2111).
const WARM_QUIET_NS: u64 = 2_000_000;
static CORE: spin::Mutex<Option<Core>> = spin::Mutex::new(None);
/// Counts interrupts and the state changes waiters wait for; a waiter sleeps only while it
/// is unchanged.
static EVENTS: AtomicU64 = AtomicU64::new(0);
static WAITERS: WaitQueueHead = WaitQueueHead::new();
/// Counts sends to the device. `kgpu` blocks only while it is unchanged since kgpu chose its
/// deadline: a send made after that would otherwise wait for that deadline, because an
/// SVGA3 device raises no interrupt and only kgpu polls it for the answers.
static SENT: AtomicU64 = AtomicU64::new(0);
static ISR: AtomicU64 = AtomicU64::new(0);
/// The control queue's notification address, and whether requests were queued since it
/// was last written: see [`ring_doorbell`].
static DOORBELL: AtomicU64 = AtomicU64::new(0);
static KICK: AtomicBool = AtomicBool::new(false);
/// The highest request number (`Device::made`) a completed doorbell write covered: the
/// requests up to it were published before the device was notified.
static RUNG: AtomicU64 = AtomicU64::new(0);
/// A configuration change was signalled: the device may have set DEVICE_NEEDS_RESET.
static CONFIG_CHANGED: AtomicBool = AtomicBool::new(false);
/// The level interrupt is masked until normal GPU work reads its ISR byte.
static IRQ_PENDING: AtomicBool = AtomicBool::new(false);
/// The control queue's interrupt (or the INTx line), and with MSI-X the configuration
/// change interrupt.
static INTID: AtomicU32 = AtomicU32::new(0);
static CONFIG_INTID: AtomicU32 = AtomicU32::new(0);
static REAPER: AtomicU64 = AtomicU64::new(0);
static REAPER_STARTING: AtomicBool = AtomicBool::new(false);

fn now_ns() -> u64 {
    crate::signal::monotonic_nanos()
}

fn with_core<R>(f: impl FnOnce(&mut Core) -> R) -> Option<R> {
    crate::per_cpu::preempt_disable();
    let (r, kick) = {
        let mut core = panic::lock(&CORE);
        let r = core.as_mut().map(f);
        // Claim this operation's notification before another CPU can enter the core, with
        // the requests made available so far, which the notification covers.
        let kick = KICK.swap(false, Ordering::AcqRel).then(|| {
            core.as_ref().and_then(|c| c.dev.as_ref()).and_then(|d| match d {
                Device::Virgl(d) => Some(d.made()),
                // SVGA submits command buffers directly, without a deferred doorbell.
                Device::Svga(_) => None,
            })
        }).flatten();
        (r, kick)
    };
    if let Some(made) = kick {
        ring_doorbell();
        RUNG.fetch_max(made, Ordering::AcqRel);
    }
    crate::per_cpu::preempt_enable();
    r
}

/// Notify the device of the requests [`Core::flush`] made available, after the core's lock
/// is released. The notification is an MMIO write, and a hypervisor may hold the writing
/// CPU until its device model accepts it; no thread waits for the lock meanwhile. The
/// requests' descriptors and ring index were published before `KICK` was set, and the
/// caller claimed that kick while still holding the core's lock.
fn ring_doorbell() {
    let _access = panic::Access::enter();
    let at = DOORBELL.load(Ordering::Acquire);
    if at != 0 {
        // SAFETY: the control queue's mapped notification address; queue 0.
        unsafe { core::ptr::write_volatile(at as *mut u16, 0) };
    }
}

/// Tell the waiters and `kgpu` that something changed. The waiters come off the queue
/// before they are woken, so no wake waits for the scheduler's lock while the queue's lock
/// is held with interrupts masked (section 1.16, #2003); `wait_event` publishes a wait
/// that this order cannot miss.
fn notify() {
    EVENTS.fetch_add(1, Ordering::AcqRel);
    let mut taken = [0u64; 8];
    loop {
        let n = WAITERS.take_waiters(&mut taken);
        for &tid in &taken[..n] {
            crate::task::scheduler::wake_waitqueue_thread(tid);
        }
        if n < taken.len() {
            break;
        }
    }
    wake_reaper();
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
    /// `PRESENT`s whose flush completed OK (`/proc/breenix/stage-composites`).
    presents: u64,
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
    /// The SVGA3D DX backend's objects of this context.
    dx: svga::encode::Objects,
    vb: u32,
    vertex_arena: Option<VertexArena>,
    private_scratch: Vec<PrivateScratch>,
    scratch: u32,
    /// The 1x1 texture the warm-up's sampling draws read.
    scratch_texture: u32,
    /// The rendering pipelines whose warm-up step completed, one bit each
    /// (`encode::ALL_WARM` when the catalogue's warm-up is complete).
    warm: u32,
    /// Initial grace deadline; later resource creations do not postpone it.
    warm_after: u64,
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

struct PrivateScratch { id: u32, class: u32, accepted: bool }

struct VertexArena {
    id: u32,
    ready: bool,
    accepted: bool,
    ranges: vertex_arena::Ranges,
}

impl Ctx {
    /// Programs whose warm-up completed: two per warmed rendering pipeline, and each
    /// `probe_int` routine's pair from its first PROBE.
    fn warmed(&self) -> u32 {
        2 * self.warm.count_ones() + if dev::is_svga() { self.dx.probe_programs() } else { self.objects.probe_programs() }
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
    /// A submission kept by test point 31's fence-depth mode: its result, until it is
    /// published.
    result: Option<(u32, u32)>,
    writer: Option<Arc<window_targets::Writer>>,
    written: Vec<u32>,
    whole: Vec<u32>,
    /// Resources it holds until the device has returned its requests and its result has
    /// reached callers, or a confirmed reset has ended them.
    held: Vec<u32>,
    /// A `PRESENT`'s bind and flush rather than a `SUBMIT`.
    present: Option<display::Presenting>,
    /// The charge for its translated device chunks, released when it retires.
    _charge: Option<alloc::sync::Arc<Charge>>,
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
    device_ctx: Option<u32>,
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
    /// Admission identities survive translation, completion publication and sealing.
    accepted: Vec<Arc<window_targets::Writer>>,
    writers: u32,
    invalid: bool,
    invalidated_by: u64,
    dropped: bool,
    holds: u32,
    lost: bool,
    reading: bool,
    window_sealed: bool,
    /// The kernel screen's context this resource is attached to for observing it as a
    /// retained binding (`screen::observation_context`), or 0.
    observed_in: u32,
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
    /// When a request other than a warm-up step's was last sent or answered.
    traffic_ns: u64,
    /// Start of the last warm-up step, to space steps interrupted by other traffic.
    warm_started_ns: Option<u64>,
    /// Since when the next warm-up step has been held for that traffic.
    warm_held: Option<u64>,
    /// The watchdog's accounts of the requests it failed, for `kgpu` to post, and how
    /// many found no room.
    hung: [Option<Hung>; HUNG_KEPT],
    hung_dropped: u32,
    /// The console's holds off the kernel screen's backing that interrupted observations
    /// left, each with its resource, kept until no transfer reads that resource: the device
    /// answered each, or a confirmed reset ended them (`end_reading`).
    canvas: Vec<(u32, crate::console::CanvasHold)>,
}

// ---------------------------------------------------------------------------------------
// Initialisation and the profile.

/// Find and initialise the virtio-gpu PCI device (QEMU's virtio-gpu-gl-pci, or Parallels'
/// virtio-gpu when the launcher selects the core), read its profile, and route its
/// interrupts. Called once from driver initialisation; an absent or failing device leaves
/// a core that refuses its ops naming why.
pub fn init() {
    let mut core = Core::empty();
    match virgl::Device::probe() {
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
            DOORBELL.store(dev.doorbell(), Ordering::Release);
            use crate::arch_impl::aarch64::gic;
            match dev.irq {
                virgl::Irq::Intx(intid) => {
                    ISR.store(dev.isr, Ordering::Release);
                    INTID.store(intid, Ordering::Release);
                    gic::configure_spi_level_triggered(intid);
                }
                virgl::Irq::Msix { queue, config } => {
                    crate::serial_println!("[gpu] MSI-X: queue SPI {} config SPI {}", queue, config);
                    INTID.store(queue, Ordering::Release);
                    CONFIG_INTID.store(config, Ordering::Release);
                    for spi in [queue, config] {
                        gic::configure_spi_edge_triggered(spi);
                        gic::clear_spi_pending(spi);
                    }
                }
            }
            core.queue_size = dev.queue_size();
            core.dev = Some(Device::Virgl(dev));
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
    #[cfg(any(feature = "capture_panic_oracle", feature = "gpu_fault_acceptance"))]
    panic::initialized();
}

/// Find and initialise VMware's SVGA3 device for the SVGA3D DX backend, read its profile
/// from the device-capability entries, and take its programs. The device raises no
/// interrupt this kernel can take, so `kgpu` polls it while work is outstanding. An absent
/// or failing device leaves a core that refuses its ops, naming why.
pub fn init_vmware() {
    console::retire_firmware();
    let mut core = Core::empty();
    dev::select_svga();
    match svga::Device::probe() {
        Ok(device) => {
            core.profile_svga(&device);
            crate::serial_println!(
                "[gpu] SVGA3D DX: dx={:#x} bgra={:#x} rgba={:#x} r8={:#x} max={}x{} screen={}x{} contexts={} surfaces={} status={} {}",
                device.profile.dx, device.profile.bgra, device.profile.rgba, device.profile.r8,
                device.profile.width, device.profile.height, device.profile.screen_width, device.profile.screen_height,
                device.profile.contexts, device.profile.surfaces, core.status, core.reason
            );
            core.queue_size = device.queue_size();
            core.dev = Some(Device::Svga(device));
        }
        Err(error) => {
            crate::serial_println!("[gpu] SVGA3 initialization refused: {:?}", error);
            match error {
                svga::Error::NoDevice => {
                    core.status = abi::status::NO_DEVICE;
                    core.reason = "no SVGA3 PCI device";
                }
                svga::Error::Missing(id) => {
                    core.status = abi::status::MISSING_CAPABILITY;
                    core.caps.missing = id;
                    core.reason = capability::name(id);
                }
                svga::Error::Unsupported(what) => {
                    core.status = abi::status::DEVICE_FAILED;
                    core.reason = what;
                }
                _ => {
                    core.status = abi::status::DEVICE_FAILED;
                    core.reason = "SVGA3 initialization failed";
                }
            }
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
            traffic_ns: 0,
            warm_started_ns: None,
            warm_held: None,
            hung: [None; HUNG_KEPT],
            hung_dropped: 0,
            canvas: Vec::new(),
        }
    }

    /// Read the profile from the device's capability set (`docs/gui/gpu.md` section 1.5,
    /// item 8) into the CAPS record and the capability table.
    fn profile(&mut self, dev: &virgl::Device) {
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

    /// Read the profile from the SVGA3 device-capability entries (section 1.6.3, item 1)
    /// into the CAPS record and the capability table. The device has no capability set of
    /// VirGL's kind: its capability entries are the set the profile is read from, recorded
    /// as set 1 version 1, and its renderer string is empty.
    fn profile_svga(&mut self, dev: &svga::Device) {
        use svga::wire::cap::{RENDER, SAMPLE, SUPPORTED};
        let p = dev.profile;
        let c = &mut self.caps;
        c.backend = abi::backend::SVGA3D_DX;
        if p.dx != 0 {
            c.features |= abi::feature::DX_CONTEXT;
        }
        c.capset_id = 1;
        c.capset_version = 1;
        c.capset_count = 1;
        c.capset_ids[0] = 1;
        c.scanout_count = 1;
        let (width, height) = dev.scanout_size();
        c.scanouts[0] = abi::ScanoutInfo { enabled: u32::from(width != 0 && height != 0), width, height, reserved: 0 };
        c.pipelines = (1 << abi::pipeline::COUNT) - 1;
        let flags = |v: u32| {
            let mut f = 0;
            if v & (SUPPORTED | SAMPLE) == SUPPORTED | SAMPLE {
                f |= abi::format_flag::SAMPLE;
            }
            if v & (SUPPORTED | RENDER) == SUPPORTED | RENDER {
                f |= abi::format_flag::RENDER;
            }
            f
        };
        c.formats = [
            abi::FormatCaps { format: format::B8G8R8A8, flags: flags(p.bgra) },
            abi::FormatCaps { format: format::R8G8B8A8, flags: flags(p.rgba) },
            abi::FormatCaps { format: format::R8, flags: flags(p.r8) },
        ];
        // DX contexts run shader model 4 programs, whose integer instructions, integer texel
        // load (`ld`) and region copies (`DX_PRED_COPY_REGION`) the profile uses; the level
        // reported is the shader model, 40.
        c.profile = abi::profile::INTEGER_OPS | abi::profile::TEXEL_FETCH | abi::profile::REGION_COPY;
        c.shading_level = 40;
        let side = p.width.min(p.height).min(limits::SIDE);
        c.max_width = side;
        c.max_height = side;
        let f = |i: usize, flag: u32| c.formats[i].flags & flag != 0;
        self.have[capability::FEATURE_3D as usize] = p.dx != 0;
        self.have[capability::CAPABILITY_SET as usize] = true;
        self.have[capability::BGRA_RENDER as usize] = f(0, abi::format_flag::RENDER);
        self.have[capability::BGRA_SAMPLE as usize] = f(0, abi::format_flag::SAMPLE);
        self.have[capability::RGBA_RENDER as usize] = f(1, abi::format_flag::RENDER);
        self.have[capability::RGBA_SAMPLE as usize] = f(1, abi::format_flag::SAMPLE);
        self.have[capability::R8_SAMPLE as usize] = f(2, abi::format_flag::SAMPLE);
        self.have[capability::INTEGER_OPS as usize] = true;
        self.have[capability::TEXEL_FETCH as usize] = true;
        self.have[capability::REGION_COPY as usize] = true;
        self.have[capability::TARGET_SIZE as usize] = side >= 2560;
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
    fn handle(&mut self) -> u64 {
        let key = self.key;
        loop {
            self.handle_seq += 1;
            let seq = self.handle_seq;
            // Odd multiplication and the xor/multiply mix form a permutation.
            // One global sequence makes resource/context/snapshot/alias values unique
            // across processes without retaining an unbounded tombstone table.

            let mut z = key ^ seq.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            let top = z >> 56;
            if top != 0 && top != 0xFF {
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
            presents: 0,
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
            let tag = req.tag();
            if let Err(req) = dev.send(req) {
                self.backlog.push_front((epoch, req));
                break;
            }
            let now = now_ns();
            match tag {
                Tag::Sync(serial) => {
                    if let Some(s) = self.syncs.get_mut(&serial) {
                        s.sent_ns = Some(now);
                        if s.warm.is_none() {
                            self.traffic_ns = now;
                        }
                    }
                }
                Tag::Submit { serial, .. } => {
                    if let Some(s) = self.subs.get_mut(&serial) {
                        s.sent_ns.get_or_insert(now);
                    }
                    self.traffic_ns = now;
                }
                Tag::Background => {}
            }
            sent = true;
        }
        if sent {
            KICK.store(true, Ordering::Release);
            // A kgpu that has chosen its deadline but not yet blocked is running, and this
            // wake does not reach it; the count does (`reaper_block`).
            SENT.fetch_add(1, Ordering::AcqRel);
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
        req.set_tag(Tag::Sync(serial));
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

    /// Register synchronous request `serial`, take its holds and queue it. A request of the
    /// warm-up step carries the step's fence in `warm`; nobody waits for it.
    fn start_sync(&mut self, serial: u64, req: Request, ctx: u32, holds: &[u32], reading: &[u32], warm: Option<u64>) -> Result<(), i64> {
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
        let upload = req.is_upload().then(|| holds.first().copied().zip(req.fence())).flatten();
        self.syncs.insert(serial, SyncReq {
            upload, ctx, sent_ns: None, holds: holds.to_vec(), reading: reading.to_vec(), abandoned: warm.is_some(), failed: false,
            why: 0, warm,
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

    /// `kgpu`'s turn: take the completions the device has returned and the watchdog's
    /// decisions, and send the queued requests that now fit.
    fn pump(&mut self) {
        self.reap_all();
        self.watchdog(now_ns());
        self.flush();
    }

    /// A waiter's turn: take the device's completions and the watchdog's decisions. Queued
    /// requests are left to `kgpu`, which sends them (`pump`) and notifies the device; only
    /// before `kgpu` starts does a waiter send them.
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
        // The interrupt woke the waiters, and the answers' handlers notify them of what they
        // wait for: a fence published, a synchronous answer, a retired submission's holds, a
        // fault. A chunk of a submission still in progress wakes nobody else. `kgpu` sends
        // what the answers made room for.
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
        let warm = matches!(d.tag, Tag::Sync(serial) if self.syncs.get(&serial).is_some_and(|s| s.warm.is_some()));
        if !warm {
            self.traffic_ns = now_ns();
        }
        match d.tag {
            Tag::Sync(serial) => self.sync_ended(serial, Some(d)),
            Tag::Submit { serial, last, .. } => self.submit_returned(serial, last, d.answer, d.echoed),
            Tag::Background => {
                if !dev::ok(d.answer) {
                    self.fault();
                }
            }
        }
    }

    /// A request the device will never answer: never sent before a confirmed reset, or
    /// held by the device across one.
    fn ended(&mut self, req: Request) {
        match req.tag() {
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
        if s.ctx == 0 && d.as_ref().is_some_and(|d| !dev::ok(d.answer)) {
            self.fault();
        }
        if let Some(fence) = s.warm {
            self.warm_returned(d.as_ref().map(|d| dev::ok(d.answer) && d.echoed == fence));
        }
        if let Some((id, fence)) = s.upload {
            if let Some(d) = &d {
                let device_error = !dev::ok(d.answer);
                let ok = !device_error && d.echoed == fence && d.fenced;
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
                self.end_reading(id);
            }
        } else {
            let d = d.unwrap_or_else(|| Done::none(Tag::Sync(serial)));
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
        if !dev::ok(answer) {
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
            let ctx = sub.ctx;
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
                    // Fence depth keeps the answer until a fifth SUBMIT waits for room. Only
                    // the depth that kept this submission holds it: an answer that comes after
                    // that depth ended is published, even if a later depth has begun.
                    let keep = self
                        .contexts
                        .get(&ctx)
                        .and_then(|c| c.depth.as_ref())
                        .is_some_and(|d| d.kept.contains(&serial));
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
        if let Some(writer) = &sub.writer { writer.publish(status); }
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
                    r.last = Some(LastWrite { ctx: ctx_id, device_ctx: dev::context_id(ctx_id), fence, serial, answer, kind: dev::draw_kind() });
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
        if let Some(arena) = self.contexts.get_mut(&sub.ctx).and_then(|ctx| ctx.vertex_arena.as_mut()) {
            arena.ranges.release(serial);
        }
        if let Some(writer) = &sub.writer { writer.end_access(); }
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

    /// Once no request reads resource `id`, clear its `reading` mark, end the console hold
    /// an interrupted observation of it left, and free it when it is otherwise unused.
    fn end_reading(&mut self, id: u32) {
        if self.syncs.values().any(|s| s.reading.contains(&id)) {
            return;
        }
        if let Some(r) = self.resources.get_mut(&id) {
            r.reading = false;
        }
        self.canvas.retain(|(held, _)| *held != id);
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
        let req = dev::resource_unref(Tag::Background, id, Some(r.backing));
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
            let req = dev::ctx_destroy(Tag::Background, id);
            self.background(req);
        }
        if dropped {
            if let Some(c) = self.contexts.remove(&id) {
                gauge(&GPU_CONTEXTS, -1);
                if let Some(arena) = c.vertex_arena { self.unhold(arena.id); }
                for scratch in c.private_scratch { self.unhold(scratch.id); }
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
        let hung: Vec<(u64, u32, u64)> = self
            .subs
            .iter()
            .filter(|(_, s)| !s.published && s.sent_ns.is_some_and(|at| now.saturating_sub(at) >= limits::WATCHDOG_NS))
            .map(|(&serial, s)| (serial, s.ctx, s.sent_ns.unwrap_or(now)))
            .collect();
        for (serial, ctx, sent) in hung {
            self.note_hung(serial, true, ctx, sent, now);
            self.quarantine(ctx, now);
        }
        let late: Vec<u64> = self
            .syncs
            .iter()
            .filter(|(_, s)| !s.failed && s.sent_ns.is_some_and(|at| now.saturating_sub(at) >= limits::WATCHDOG_NS))
            .map(|(&serial, _)| serial)
            .collect();
        for serial in late {
            if let Some(s) = self.syncs.get(&serial) {
                let (ctx, sent) = (s.ctx, s.sent_ns.unwrap_or(now));
                self.note_hung(serial, false, ctx, sent, now);
            }
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

    /// Keep the watchdog's account of request `serial` (a submission when `submit`) of
    /// device context `ctx`, sent at `sent`, for `kgpu` to post.
    fn note_hung(&mut self, serial: u64, submit: bool, ctx: u32, sent: u64, now: u64) {
        let Some(dev) = self.dev.as_ref() else { return };
        let q = match dev {
            Device::Virgl(d) => Some(d.view(|t| match *t {
                Tag::Sync(s) => !submit && s == serial,
                Tag::Submit { serial: s, .. } => submit && s == serial,
                Tag::Background => false,
            })),
            // SVGA has command buffers, not a VirtIO descriptor/used ring.
            Device::Svga(_) => None,
        };
        let (oldest_serial, oldest_sent) = match q.and_then(|q| q.oldest_tag) {
            Some(Tag::Sync(s)) => (s, self.syncs.get(&s).and_then(|x| x.sent_ns)),
            Some(Tag::Submit { serial: s, .. }) => (s, self.subs.get(&s).and_then(|x| x.sent_ns)),
            _ => (0, None),
        };
        let h = Hung {
            serial,
            submit,
            ctx,
            age_ms: now.saturating_sub(sent) / 1_000_000,
            quiet_ms: now.saturating_sub(self.last_progress) / 1_000_000,
            rung: RUNG.load(Ordering::Acquire),
            backlog: self.backlog.len() as u32,
            warm: self.warm_step.as_ref().map(|w| (w.ctx, w.pipeline)),
            withheld: self.stalled,
            oldest_serial,
            oldest_age_ms: oldest_sent.map_or(0, |at| now.saturating_sub(at) / 1_000_000),
            q,
        };
        match self.hung.iter_mut().find(|x| x.is_none()) {
            Some(slot) => *slot = Some(h),
            None => self.hung_dropped += 1,
        }
    }

    /// Take the watchdog's accounts and the count of those that found no room.
    fn take_hung(&mut self) -> ([Option<Hung>; HUNG_KEPT], u32) {
        (core::mem::replace(&mut self.hung, [None; HUNG_KEPT]), core::mem::take(&mut self.hung_dropped))
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
        // The SVGA3 device raises no interrupt here: while it holds requests, kgpu looks
        // for their answers at POLL_NS intervals.
        let poll = (dev::is_svga() && self.dev.as_ref().is_some_and(|d| d.outstanding() > 0) && !self.stalled)
            .then(|| now + POLL_NS);
        // Grace deadlines are independent across processes.
        let grace = self.contexts.values()
            .filter(|x| x.state == CtxState::Live && !x.dropped && x.warm != encode::ALL_WARM)
            .map(|x| x.warm_after).filter(|&g| g > now).min();
        let watchdog = hangs.chain(waits).chain(progress).min().map(|at| (at + limits::WATCHDOG_NS).min(now + EXAMINE_NS));
        watchdog.into_iter().chain(grace).chain(poll).min()
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
        panic::invalidate();
        self.epoch += 1;
        if dev::is_svga() {
            svga::reset_decided();
        }
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
        self.reason = "GPU device reset did not complete";
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
// The watchdog's attribution (#2065).

/// Accounts the core keeps between two `kgpu` passes.
const HUNG_KEPT: usize = 4;
/// Notice lines per account.
const HUNG_LINES: usize = 4;

/// The watchdog's account of a request it failed: which request, its place in the control
/// queue (queue 0), whether a completed doorbell write covered it, and what the device did
/// while it waited.
#[derive(Clone, Copy)]
struct Hung {
    serial: u64,
    submit: bool,
    ctx: u32,
    age_ms: u64,
    /// Since the device last returned a request.
    quiet_ms: u64,
    /// `RUNG` when the watchdog failed it.
    rung: u64,
    backlog: u32,
    /// The catalogue warm-up step on the device: its context and pipeline.
    warm: Option<(u32, u32)>,
    /// Test point 31 is withholding the device's answers.
    withheld: bool,
    /// The serial of the oldest request the device holds, and its age.
    oldest_serial: u64,
    oldest_age_ms: u64,
    q: Option<virgl::QueueView>,
}

/// The nodes the accounts are posted through; a node is reused once its line is written.
// Summaries use a separate node from account lines. If this node is still
// posted, kgpu retains the new counts and retries them after its next wake.
static HUNG_LOSS_NODE: crate::log_notice::Node = crate::log_notice::Node::new();
static HUNG_NODES: [crate::log_notice::Node; HUNG_KEPT * HUNG_LINES] =
    [const { crate::log_notice::Node::new() }; HUNG_KEPT * HUNG_LINES];

/// One notice line, composed on the stack.
struct NoticeText {
    bytes: [u8; 80],
    len: usize,
}

impl core::fmt::Write for NoticeText {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = s.len().min(self.bytes.len() - self.len);
        self.bytes[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

/// Post the watchdog's accounts as log notices, from `kgpu` outside the core's lock: the
/// UART is written later by a thread that logs, or by the idle loop, rather than by this
/// GPU path with its interrupts masked (section 1.16). `accounts` found no room in the core; a line
/// that finds no free node is dropped too, and both are counted.
fn post_hung(hung: &[Option<Hung>; HUNG_KEPT], accounts: u32, pending: &mut (u64, u64)) {
    use core::fmt::Write;
    let mut lines = 0u32;
    let mut post = |f: &dyn Fn(&mut NoticeText) -> core::fmt::Result| {
        let mut t = NoticeText { bytes: [0; 80], len: 0 };
        let _ = f(&mut t);
        let text = core::str::from_utf8(&t.bytes[..t.len]).unwrap_or("[gpu] watchdog");
        if !HUNG_NODES.iter().any(|n| n.post_again(text)) {
            lines += 1;
        }
    };
    for h in hung.iter().flatten() {
        let q = h.q.as_ref();
        let what = if h.submit { "submission" } else { "sync" };
        post(&|t| {
            write!(t, "[gpu] watchdog: {} {} ctx {} failed after {} ms", what, h.serial, h.ctx, h.age_ms)?;
            if let Some(q) = q.filter(|q| q.entry != 0) { write!(t, " cmd {:#x}", q.cmd) } else { Ok(()) }
        });
        let Some(q) = q else {
            post(&|t| write!(t, "[gpu]  SVGA command buffer quiet {} ms backlog {}", h.quiet_ms, h.backlog));
            continue;
        };
        let notified = match q.entry {
            0 => "returned",
            e if h.rung >= e => "notified",
            _ => "NOT notified",
        };
        post(&|t| {
            write!(t, "[gpu]  queue 0 entry {} head {} {} rung {}", q.entry, q.head, notified, h.rung)?;
            if let Some((c, p)) = h.warm {
                write!(t, " warm {c}.{p}")?;
            }
            if h.withheld { write!(t, " withheld") } else { Ok(()) }
        });
        post(&|t| {
            write!(t, "[gpu]  made {} answered {} used {}/{} quiet {} ms", q.made, q.answered, q.used, q.taken, h.quiet_ms)
        });
        post(&|t| {
            write!(t, "[gpu]  out {} oldest {} cmd {:#x} serial {} age {} ms bl {}",
                q.outstanding, q.oldest, q.oldest_cmd, h.oldest_serial, h.oldest_age_ms, h.backlog)
        });
    }
    pending.0 += u64::from(accounts);
    pending.1 += u64::from(lines);
    if *pending != (0, 0) {
        let mut t = NoticeText { bytes: [0; 80], len: 0 };
        let _ = write!(t, "[gpu] watchdog: {} accounts and {} lines dropped", pending.0, pending.1);
        if HUNG_LOSS_NODE.post_again(core::str::from_utf8(&t.bytes[..t.len]).unwrap_or("")) {
            *pending = (0, 0);
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
    let start = now_ns();
    loop {
        let seen = EVENTS.load(Ordering::Acquire);
        let (found, polled) = with_core(|c| {
            c.poll();
            // The device's answers reach this waiter only when someone looks for them.
            let polled = dev::is_svga() && !c.stalled && c.dev.as_ref().is_some_and(|d| d.outstanding() > 0);
            (f(c), polled)
        })
        .unwrap_or((None, false));
        if let Some(v) = found {
            return Ok(v);
        }
        if deadline.is_some_and(|d| now_ns() >= d) {
            return Err(Stop::Timeout);
        }
        if interruptible && crate::syscall::check_signals_for_wait().is_some() {
            return Err(Stop::Interrupted);
        }
        if polled && now_ns().saturating_sub(start) < SPIN_POLL_NS {
            for _ in 0..SPIN_POLL_PAUSE {
                core::hint::spin_loop();
            }
            continue;
        }
        let wake = Some(deadline.unwrap_or(u64::MAX).min(now_ns().saturating_add(EXAMINE_NS)));
        // A queued wait holds the one preemption level syscall entry takes, and
        // `wait_prepared` releases it only for the halt. A kernel thread (the console's
        // render thread) runs at count 0, so it takes the level here. Released unheld,
        // the count underflows, the halt sees a caller that cannot be preempted and keeps
        // the CPU, and the request this wait needs stays queued for a kgpu waiting on
        // that CPU (#1826).
        let unheld = crate::per_cpu::preempt_count() == 0;
        if unheld {
            crate::per_cpu::preempt_disable();
        }
        let woke = wait_event(seen, wake);
        if unheld {
            crate::per_cpu::preempt_enable();
        }
        if woke.is_err() && interruptible {
            return Err(Stop::Interrupted);
        }
    }
}

/// Sleep until a GPU event after `seen`, the absolute `wake`, or a signal, holding a
/// preemption level. The thread joins `WAITERS`, publishes its blocked state, then reads
/// `EVENTS` again, each step in a section of its own, so no step waits for the scheduler's
/// lock while it holds the queue's lock with interrupts masked (section 1.16, #2003).
/// `notify` advances `EVENTS` before it takes waiters off the queue: a thread that joins
/// after the take reads the new count, and one taken before it publishes reads it too,
/// because its wake reached it running.
fn wait_event(seen: u64, wake: Option<u64>) -> Result<(), i32> {
    let Some(tid) = crate::task::scheduler::current_thread_id() else {
        return Err(crate::syscall::errno::ESRCH);
    };
    if !WAITERS.add_waiter(tid) {
        return Err(crate::syscall::errno::ENOMEM);
    }
    let published = crate::task::scheduler::with_scheduler(|sched| sched.block_current_for_io_with_timeout(wake)).unwrap_or(false);
    if !published {
        WAITERS.finish_wait_for(tid);
        return Err(crate::syscall::errno::ESRCH);
    }
    if EVENTS.load(Ordering::Acquire) != seen {
        WAITERS.finish_wait_for(tid);
        return Ok(());
    }
    crate::syscall::blocking_io::wait_prepared(&WAITERS, PrepareOutcome::Queued)
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
    send_sync(serial, req, ctx, holds, reading)?;
    await_sync(serial)
}

/// The sending half of [`exec_req_why`]: queue synchronous request `serial` with its holds
/// and `reading` marks. Its answer is taken with [`await_sync`], or left with
/// `Core::abandon` by a caller that stops waiting.
fn send_sync(serial: u64, req: Request, ctx: u32, holds: &[u32], reading: &[u32]) -> Result<(), (i64, u32)> {
    if !ensure_reaper() {
        return Err((ENOMEM, 0));
    }
    with_core(|c| c.start_sync(serial, req, ctx, holds, reading, None))
        .unwrap_or(Err(ENODEV))
        .map_err(|e| (e, if e == EIO { fence_status::DEVICE_RESET } else { 0 }))
}

/// The waiting half of [`exec_req_why`]: the answer to sent request `serial`, or its EIO
/// cause, or EINTR. A request that ends without its answer is abandoned.
fn await_sync(serial: u64) -> Result<Done, (i64, u32)> {
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
    if dev::ok(d.answer) {
        Ok(d)
    } else {
        Err(EIO)
    }
}

/// Attach resources to device contexts, one synchronous `(context, resource)` command a
/// pair, queued together in one batch so the device answers them back to back: a new
/// context takes the process's resources, and a new resource the process's contexts, in
/// one wait instead of a round trip a pair (#2103). Each pair has its own outcome, a
/// success answer or EIO; a signal or a kill ends the wait, and the pairs not yet answered
/// then fail with EINTR. A context that ends also fails its unanswered pairs with EIO,
/// including requests still in the backlog, whose watchdog clock has not started.
fn attach_all(pairs: &[(u32, u32)]) -> Vec<Result<(), i64>> {
    if pairs.is_empty() {
        return Vec::new();
    }
    if !ensure_reaper() {
        return alloc::vec![Err(ENOMEM); pairs.len()];
    }
    let Some(serials) = with_core(|c| pairs.iter().map(|_| c.serial()).collect::<Vec<u64>>()) else {
        return alloc::vec![Err(ENODEV); pairs.len()];
    };
    let reqs: Vec<Request> =
        pairs.iter().zip(&serials).map(|(&(ctx, id), &serial)| dev::ctx_attach(Tag::Sync(serial), ctx, id)).collect();
    // A pair that could not be queued has its outcome already.
    let mut out: Vec<Option<Result<(), i64>>> = with_core(|c| {
        reqs.into_iter()
            .zip(pairs.iter().zip(&serials))
            .map(|(req, (&(ctx, _), &serial))| c.start_sync(serial, req, ctx, &[], &[], None).err().map(Err))
            .collect()
    })
    .unwrap_or_else(|| alloc::vec![Some(Err(ENODEV)); pairs.len()]);
    let _ = wait_for(None, true, |c| {
        for ((slot, serial), &(ctx, _)) in out.iter_mut().zip(&serials).zip(pairs) {
            if slot.is_some() {
                continue;
            }
            if let Some(d) = c.done.remove(serial) {
                *slot = Some(if dev::ok(d.answer) { Ok(()) } else { Err(EIO) });
            } else if c.syncs.get(serial).is_some_and(|s| s.failed)
                || !c.contexts.get(&ctx).is_some_and(|x| matches!(x.state, CtxState::Creating | CtxState::Live) && !x.dropped)
            {
                *slot = Some(Err(EIO));
            }
        }
        out.iter().all(Option::is_some).then_some(())
    });
    if out.iter().any(|o| !matches!(o, Some(Ok(())))) {
        // The waits that failed or were interrupted leave; the device's answers to them are
        // dropped when they come.
        with_core(|c| {
            for &serial in &serials {
                c.abandon(serial);
            }
        });
    }
    out.into_iter().map(|o| o.unwrap_or(Err(EINTR))).collect()
}

// ---------------------------------------------------------------------------------------
// The interrupt and the reaper thread.

/// The GIC interrupt id of the GPU's INTx line, or of its control queue's MSI-X vector,
/// once the device is up.
pub fn irq() -> Option<u32> {
    match INTID.load(Ordering::Acquire) {
        0 => None,
        id => Some(id),
    }
}

/// The GIC interrupt id of the GPU's configuration-change MSI-X vector, when it uses MSI-X.
pub fn config_irq() -> Option<u32> {
    match CONFIG_INTID.load(Ordering::Acquire) {
        0 => None,
        id => Some(id),
    }
}

/// Defer INTx acknowledgement to the IRQ-enabled worker; MSI-X needs no ISR read
/// (VirtIO 1.3 section 4.1.4.5.2). Wake consumers with bounded, nonblocking queue work.
pub fn handle_interrupt() {
    let start = crate::tracing::providers::gpu::interrupt_begin();
    if ISR.load(Ordering::Acquire) != 0 {
        if let Some(id) = irq() {
            crate::arch_impl::aarch64::gic::disable_spi(id);
        }
        IRQ_PENDING.store(true, Ordering::Release);
    }
    signal(1);
    crate::tracing::providers::gpu::interrupt_end(start);
}

/// The configuration-change MSI-X vector needs no ISR read.
pub fn handle_config_interrupt() {
    let start = crate::tracing::providers::gpu::interrupt_begin();
    signal(2);
    crate::tracing::providers::gpu::interrupt_end(start);
}

/// Wake for a used buffer or configuration change, using the ISR byte's bit meanings.
fn signal(status: u8) {
    if status & 2 != 0 {
        CONFIG_CHANGED.store(true, Ordering::Release);
    }
    EVENTS.fetch_add(1, Ordering::AcqRel);
    WAITERS.try_wake_up_n_from_irq(limits::IN_FLIGHT as u32);
    let tid = REAPER.load(Ordering::Acquire);
    if tid != 0 {
        crate::task::scheduler::wake_thread_any_context(tid);
    }
}

fn acknowledge_interrupt() {
    if !IRQ_PENDING.swap(false, Ordering::AcqRel) {
        return;
    }
    let _access = panic::Access::enter();
    let isr = ISR.load(Ordering::Acquire);
    let status = if isr != 0 { virgl::read_isr(isr) } else { 0 };
    if status & 2 != 0 {
        CONFIG_CHANGED.store(true, Ordering::Release);
    }
    if let Some(id) = irq() {
        crate::arch_impl::aarch64::gic::unmask_spi(id);
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
                    for id in [irq(), config_irq()].into_iter().flatten() {
                        crate::arch_impl::aarch64::gic::enable_spi_on_cpu(id, 0);
                    }
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

/// Block `kgpu` until `wake` (or indefinitely) or until a GPU event after `seen` or a send
/// after `sent`. It
/// blocks for I/O: the scheduler applies a wake from interrupt context, such as
/// `handle_interrupt`'s, only to a thread blocked for I/O, so a timer block would leave
/// the device's answers waiting for the next deadline. Once woken it returns without
/// halting for a further interrupt.
fn reaper_block(tid: u64, seen: u64, sent: u64, wake: Option<u64>) {
    // One scheduler section publishes the block and checks for an event since `seen`, so
    // the lock is taken once and waited for with interrupts enabled (section 1.16, #2003).
    crate::task::scheduler::with_scheduler(|sched| {
        sched.block_current_for_io_with_timeout(wake);
        if EVENTS.load(Ordering::Acquire) != seen || SENT.load(Ordering::Acquire) != sent {
            sched.unblock(tid);
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
    let mut hung_losses = (0, 0);
    loop {
        let seen = EVENTS.load(Ordering::Acquire);
        acknowledge_interrupt();
        let now = now_ns();
        // Sends happen under the core's lock, so the deadline accounts for the sends counted here.
        let (deadline, busy, reset, hung, sent) = with_core(|c| {
            c.pump();
            (c.next_deadline(now), !c.procs.is_empty(), c.reset_wanted, c.take_hung(), SENT.load(Ordering::Acquire))
        })
        .unwrap_or((None, false, false, ([None; HUNG_KEPT], 0), SENT.load(Ordering::Acquire)));
        crate::window::scene::invalidate_stale();
        crate::window::ops::publish_ready();
        post_hung(&hung.0, hung.1, &mut hung_losses);
        if reset {
            reset_device(tid);
            continue;
        }
        if busy && now >= next_sweep {
            sweep();
            next_sweep = now + 1_000_000_000;
        }
        let deadline = match (deadline, warm_next()) {
            (Some(d), Some(w)) => Some(d.min(w)),
            (d, w) => d.or(w),
        };
        let wake = match (deadline, busy) {
            (Some(d), true) => Some(d.min(next_sweep)),
            (Some(d), false) => Some(d),
            (None, true) => Some(next_sweep),
            (None, false) => None,
        };
        let wake = if hung_losses != (0, 0) {
            Some(wake.unwrap_or(u64::MAX).min(now + EXAMINE_NS))
        } else {
            wake
        };
        reaper_block(tid, seen, sent, wake);
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
        reaper_block(tid, EVENTS.load(Ordering::Acquire), SENT.load(Ordering::Acquire), Some(now_ns() + 1_000_000));
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
/// device at a time, after the context's process-local grace and while the device is
/// neither resetting nor suspected hung, so other requests wait behind at most one step.
/// A step waits for `WARM_QUIET_NS` of send/answer quiet, for at most `WARM_GRACE_NS`.
/// If other traffic followed the preceding step's start, the next start is also at least
/// `WARM_GRACE_NS` after it; while held this returns when `kgpu` should look again.
/// The step takes the context's submission slot while it is encoded and
/// queued, like a SUBMIT, so its objects and the context's submissions reach the device in
/// the order their objects were allocated. Its requests are synchronous requests of the
/// context under the watchdog that nobody waits on; their answers go to `warm_returned`.
fn warm_next() -> Option<u64> {
    let ready = |c: &Core| {
        c.warm_step.is_none()
            && c.dev.is_some()
            && !c.reset_wanted
            && !c.resetting
            && !c.stalled
            && c.quarantined_at.is_none()
    };
    let warming = |x: &Ctx| x.owner != 0 && x.state == CtxState::Live && x.device_alive && !x.dropped && x.warm != encode::ALL_WARM && now_ns() >= x.warm_after;
    let due = with_core(|c| {
        if !(ready(c) && c.contexts.values().any(warming)) {
            c.warm_held = None;
            return Err(None);
        }
        let now = now_ns();
        let held = *c.warm_held.get_or_insert(now);
        let due = c.traffic_ns.saturating_add(WARM_QUIET_NS).min(held.saturating_add(WARM_GRACE_NS));
        let due = c.warm_started_ns.filter(|&start| c.traffic_ns >= start)
            .map_or(due, |start| due.max(start.saturating_add(WARM_GRACE_NS)));
        if now < due {
            return Err(Some(due));
        }
        c.warm_held = None;
        Ok(())
    })
    .unwrap_or(Err(None));
    if let Err(wake) = due {
        return wake;
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
        Some((ctx.id, pipeline, ctx.objects.clone(), ctx.dx.clone(), [ctx.vb, ctx.scratch, ctx.scratch_texture]))
    })
    .flatten();
    let Some((id, pipeline, mut objects, mut dx_objects, internal)) = picked else { return None };
    let [vb, scratch, scratch_texture] = internal;
    // After an allocation failure no request is sent; the step is tried again later.
    let chunks = if dev::is_svga() {
        let target = encode::TargetRes {
            id: scratch,
            format: wire::format::B8G8R8A8_UNORM,
            info: TargetInfo { width: 1, height: 1, invalid: false, format: format::B8G8R8A8 },
        };
        let texel = encode::SourceRes { id: scratch_texture, format: wire::format::B8G8R8A8_UNORM, width: 1, height: 1 };
        svga::encode::warm_pipeline(&mut dx_objects, &target, &texel, pipeline).map(Chunks::Svga)
    } else {
        let mut enc = encode::Enc::new();
        encode::warm_pipeline(&mut enc, &mut objects, vb, scratch, scratch_texture, wire::format::B8G8R8A8_UNORM, FLIP_Y, pipeline);
        enc.finish().filter(|v| !v.is_empty()).map(Chunks::Virgl)
    };
    with_core(|c| {
        let live = c.contexts.get(&id).is_some_and(|x| x.state == CtxState::Live && x.device_alive && !x.dropped);
        if let Some(chunks) = chunks.filter(|_| live && ready(c)) {
            let too_big = matches!(&chunks, Chunks::Virgl(v) if v.iter().any(|w| w.page_count() + 2 > c.queue_size));
            if too_big {
                // A step the queue cannot carry is refused like one the device refuses.
                c.warm_step = Some(WarmStep { ctx: id, pipeline, left: 1, refused: true, lost: false });
                c.warm_returned(Some(false));
            } else {
                if let Some(ctx) = c.contexts.get_mut(&id) {
                    ctx.objects = objects;
                    ctx.dx = dx_objects;
                }
                c.warm_started_ns = Some(now_ns());
                let count = match &chunks {
                    Chunks::Virgl(v) => v.len(),
                    Chunks::Svga(v) => v.len(),
                };
                c.warm_step = Some(WarmStep { ctx: id, pipeline, left: count as u32, refused: false, lost: false });
                let reqs: Vec<(u64, u64, Option<Request>)> = match chunks {
                    Chunks::Virgl(v) => v.into_iter().map(|words| {
                        let (serial, fence) = (c.serial(), c.device_fence());
                        (serial, fence, Some(dev::submit_virgl(Tag::Sync(serial), id, Some(fence), words)))
                    }).collect(),
                    Chunks::Svga(v) => v.into_iter().map(|chunk| {
                        let (serial, fence) = (c.serial(), c.device_fence());
                        (serial, fence, dev::submit_svga(Tag::Sync(serial), id, Some(fence), chunk))
                    }).collect(),
                };
                for (serial, fence, req) in reqs {
                    let Some(req) = req else {
                        c.warm_returned(None);
                        continue;
                    };
                    if c.start_sync(serial, req, id, &internal, &[], Some(fence)).is_err() {
                        c.warm_returned(None);
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
    None
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
struct CreationGuard {
    owner: u64,
}

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
        Ok(Self { owner: tgid })
    }
}

impl Drop for CreationGuard {
    fn drop(&mut self) {
        with_core(|c| {
            if let Some(p) = c.procs.get_mut(&self.owner) {
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
        accepted: Vec::new(),
        writers: 0,
        invalid: false,
        invalidated_by: 0,
        dropped: false,
        holds: 0,
        lost: false,
        reading: false,
        window_sealed: false,
        observed_in: 0,
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
                dx: svga::encode::Objects::new(0),
                vb,
                vertex_arena: None,
                private_scratch: Vec::new(),
                scratch,
                scratch_texture,
                warm: 0,
                warm_after: 0,
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
            let h = c.handle();
            c.proc_mut(tgid).contexts.insert(h, id);
            let ctx = c.contexts.get_mut(&id).ok_or(EIO)?;
            ctx.state = CtxState::Live;
            ctx.warm_after = now_ns().saturating_add(WARM_GRACE_NS);
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
/// are warmed afterwards on `kgpu` (`warm_next`), and each `probe_int` routine by its first PROBE. Each
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
    exec_ok(|t| dev::ctx_create(t, id), id, &[])?;
    if dev::is_svga() {
        let cid = svga::cid_of(id).ok_or(EIO)?;
        with_core(|c| {
            if let Some(ctx) = c.contexts.get_mut(&id) {
                ctx.dx = svga::encode::Objects::new(cid);
            }
        });
    }
    exec_ok(|t| dev::resource_create(t, vb, kind::BUFFER, format::R8, encode::VERTEX_BUFFER_BYTES, 1, false), 0, &[])?;
    exec_ok(|t| dev::attach_backing(t, vb, vb_pages.clone()), 0, &[])?;
    exec_ok(|t| Some(dev::ctx_attach(t, id, vb)), id, &[])?;
    exec_ok(|t| dev::resource_create(t, scratch, kind::TARGET, format::B8G8R8A8, 1, 1, false), 0, &[])?;
    exec_ok(|t| dev::attach_backing(t, scratch, scratch_pages.clone()), 0, &[])?;
    exec_ok(|t| Some(dev::ctx_attach(t, id, scratch)), id, &[])?;
    exec_ok(|t| dev::resource_create(t, scratch_texture, kind::TEXTURE, format::B8G8R8A8, 1, 1, false), 0, &[])?;
    exec_ok(|t| Some(dev::ctx_attach(t, id, scratch_texture)), id, &[])?;
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
    let pairs: Vec<(u32, u32)> = theirs.iter().map(|&rid| (id, rid)).collect();
    let attached = attach_all(&pairs).into_iter().find(Result::is_err).unwrap_or(Ok(()));
    with_core(|c| {
        for &rid in &theirs {
            c.unhold(rid);
        }
    });
    attached?;
    let chunks: Vec<Request> = if dev::is_svga() {
        let (serial, fence) = with_core(|c| (c.serial(), c.device_fence())).ok_or(ENODEV)?;
        let chunk = svga::encode::chunk(svga::encode::warm_base()).ok_or(ENOMEM)?;
        alloc::vec![dev::submit_svga(Tag::Sync(serial), id, Some(fence), chunk).ok_or(EIO)?]
    } else {
        let mut objects = with_core(|c| c.contexts.get(&id).map(|ctx| ctx.objects.clone())).flatten().ok_or(EIO)?;
        let mut enc = encode::Enc::new();
        encode::warm_base(&mut enc, &mut objects, vb, scratch, wire::format::B8G8R8A8_UNORM);
        let chunks = enc.finish().filter(|v| !v.is_empty()).ok_or(ENOMEM)?;
        with_core(|c| if let Some(ctx) = c.contexts.get_mut(&id) { ctx.objects = objects; });
        let mut reqs = Vec::new();
        for words in chunks {
            let (serial, fence) = with_core(|c| (c.serial(), c.device_fence())).ok_or(ENODEV)?;
            reqs.push(dev::submit_virgl(Tag::Sync(serial), id, Some(fence), words));
        }
        reqs
    };
    for req in chunks {
        let Tag::Sync(serial) = req.tag() else { return Err(EIO) };
        let fence = req.fence();
        let live = with_core(|c| {
            c.contexts
                .get(&id)
                .is_some_and(|ctx| ctx.state == CtxState::Creating && ctx.device_alive)
        })
        .ok_or(ENODEV)?;
        if !live {
            return Err(EIO);
        }
        let d = exec_req(serial, req, id, &[vb, scratch, scratch_texture], &[])?;
        if !dev::ok(d.answer) || Some(d.echoed) != fence {
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
    let (bytes, pitch) = match a.kind {
        kind::TARGET | kind::TEXTURE => {
            match (a.kind, a.format) {
                (_, format::B8G8R8A8) | (_, format::R8G8B8A8) | (kind::TEXTURE, format::R8) => {}
                _ => return Err(EINVAL),
            }
            if a.width == 0 || a.height == 0 || a.width > max || a.height > max || a.size != 0 {
                return Err(EINVAL);
            }
            // VirGL transfers use rows aligned to 256 bytes; VMware's version 1 surfaces keep
            // packed rows (section 1.3, backing and row pitch).
            let packed = a.width * format::bytes(a.format);
            let pitch = if dev::is_svga() { packed } else { packed.next_multiple_of(256) };
            (pitch as u64 * a.height as u64, pitch)
        }
        kind::BUFFER => {
            if a.format != 0 || a.width != 0 || a.height != 0 || a.size == 0 || a.size > BUFFER_MAX {
                return Err(EINVAL);
            }
            (a.size, 0)
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
    let scanout = a.flags & abi::resource_flag::SCANOUT != 0;
    let mut outcome = exec_ok(|t| dev::resource_create(t, id, a.kind, a.format, width, height, scanout), 0, &[])
        .map(drop);
    if outcome.is_ok() {
        outcome = exec_ok(|t| dev::attach_backing(t, id, pages.clone()), 0, &[]).map(drop);
    }
    let ctxs: Vec<u32> = with_core(|c| {
        c.procs.get(&tgid).map(|p| p.contexts.values().copied().collect()).unwrap_or_default()
    })
    .unwrap_or_default();
    if outcome.is_ok() {
        let pairs: Vec<(u32, u32)> = ctxs.iter().map(|&ctx| (ctx, id)).collect();
        for (&ctx, attached) in ctxs.iter().zip(attach_all(&pairs)) {
            if let Err(e) = attached {
                // A context that ended meanwhile needs no attachment.
                let ended = with_core(|c| !c.contexts.get(&ctx).is_some_and(|x| x.state == CtxState::Live && x.device_alive && !x.dropped))
                    .unwrap_or(true);
                if !ended {
                    outcome = Err(e);
                    break;
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
            let h = c.handle();
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

/// A translated submission's command buffers, in its backend's form.
enum Chunks {
    Virgl(Vec<WordBuf>),
    Svga(Vec<svga::encode::Chunk>),
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
        if named_ids.iter().any(|rid| !c.resources.get(rid).is_some_and(|r| !r.dropped && !r.lost)) { return Some(Err(EINVAL)); }
        let Some(ctx) = c.contexts.get_mut(&id) else {
            return Some(Err(EIO));
        };
        if ctx.state != CtxState::Live || ctx.dropped {
            return Some(Err(EIO));
        }
        if written.iter().any(|rid| c.resources.get(rid).is_some_and(|r| r.window_sealed)) {
            return Some(Err(breengel_abi::errno::EBUSY as i64));
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
        ctx.issued += 1;
        let n = ctx.issued;
        let serial = c.serial();
        let fence = c.device_fence();
        let writer = Arc::new(window_targets::Writer::new(id, n, serial, fence));
        for rid in &named_ids {
            let Some(r) = c.resources.get_mut(rid) else { return Some(Err(EINVAL)); };
            r.holds += 1;
        }
        for rid in &written {
            if let Some(r) = c.resources.get_mut(rid) {
                r.writers += 1;
                r.accepted.retain(|w| !w.ended());
                r.accepted.push(writer.clone());
            }
        }
        Some(Ok(writer))
    });
    let writer = match admitted {
        Ok(Ok(writer)) => writer,
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err(EINTR),
    };
    let admitted_ids = named_ids.clone();
    let release_slot = |accepted: bool| {
        with_core(|c| {
            if !accepted {
                // This admission sent no device work: a seal that already captured it must
                // treat it as cancelled, rather than as a failed accepted GPU write.
                writer.cancel();
                let abandoned = c.contexts.get_mut(&id).and_then(|ctx| {
                    let arena = ctx.vertex_arena.as_mut()?;
                    arena.ranges.release(writer.serial);
                    if !arena.accepted { ctx.vertex_arena.take() } else { None }
                });
                if let Some(arena) = abandoned { c.unhold(arena.id); }
                let unused = if let Some(ctx) = c.contexts.get_mut(&id) {
                    let unused: Vec<u32> = ctx.private_scratch.iter().filter(|s| !s.accepted).map(|s| s.id).collect();
                    ctx.private_scratch.retain(|s| s.accepted);
                    unused
                } else { Vec::new() };
                for rid in unused { c.unhold(rid); }
                if let Some(ctx) = c.contexts.get_mut(&id) {
                    ctx.outcomes.insert(writer.n, (fence_status::DEVICE_ERROR, 0));
                    ctx.early.insert(writer.n);
                    while ctx.early.remove(&(ctx.done_upto + 1)) { ctx.done_upto += 1; }
                }
                for rid in &written {
                    if let Some(r) = c.resources.get_mut(rid) {
                        r.writers = r.writers.saturating_sub(1);
                        r.accepted.retain(|w| !Arc::ptr_eq(w, &writer));
                    }
                    c.free_if_unused(*rid);
                }
                for rid in &admitted_ids { c.unhold(*rid); }
            }
            if let Some(ctx) = c.contexts.get_mut(&id) {
                ctx.busy = false;
            }
            c.settle(id);
        });
        notify();
    };
    // Each submission reserves a distinct range in the context's reusable vertex arena.
    // The batched upload precedes drawing; completion and fence publication release ranges.
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
    let svga = dev::is_svga();
    // The SVGA3D DX translation makes its own vertex, constant, data and scratch surfaces.
    let vertex_range = match if svga { Ok(None) } else { vertex_buffer_for(tgid, id, writer.serial, encode::vertex_bytes(plan.drawn + u64::from(corrupt.is_some()))) } {
        Ok(vb) => vb,
        Err(e) => {
            release_slot(false);
            return if e == ENOMEM { refuse(&mut a, 0, stream::error::NO_MEMORY, ENOMEM) } else { Err(e) };
        }
    };
    let vertex_buffer = vertex_range.map(|(rid, _, _)| rid);
    // The submission's scratch textures for copies within one target that overlap
    // (`docs/gui/gpu.md` section 1.3), one per target format, sized for its largest.
    let mut copy_scratch = [0u32; 2];
    for (slot, [w, h]) in plan.scratch.into_iter().enumerate() {
        if w == 0 || h == 0 || svga {
            continue;
        }
        match scratch_for(tgid, id, slot as u32, w, h) {
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
    let data_texture = if plan.data_bytes > 0 && !svga {
        match scratch_for(tgid, id, 2, data_width, (plan.data_bytes/4).div_ceil(data_width)) {
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
                ctx.dx.clone(),
                ctx.vb,
                ctx.scratch,
                ctx.scratch_texture,
            )
        })
    })
    .flatten();
    let Some((mut objects, mut dx_objects, default_vb, scratch, scratch_texture)) = state else {
        drop_vertex_buffer();
        release_slot(false);
        return Err(EIO);
    };
    let private_ids: Vec<u32> = with_core(|c| c.contexts.get(&id).map(|ctx| ctx.private_scratch.iter().map(|s| s.id).collect())).flatten().unwrap_or_default();
    let keeps = |rid: &u32| *rid != scratch && *rid != scratch_texture && !view.has_id(*rid) && !private_ids.contains(rid);
    let stale: Vec<u32> = if svga {
        dx_objects.resources().filter(keeps).collect()
    } else {
        objects.resources().filter(keeps).collect()
    };
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
        encode::translate_at(enc, objects, &words, &resolved, encode::Extras {
            vertex_buffer: vertex_buffer.unwrap_or(default_vb),
            scratch: copy_scratch,
            data: data_texture,
            data_width,
            flip_y: FLIP_Y,
            corrupt,
            suppress,
            swap,
        }, vertex_range.map_or(0, |(_, offset, _)| offset));
    };
    let dx_extras = || svga::encode::Extras { scratch: plan.scratch, data_width, corrupt, suppress, swap };
    // Count surface bytes, then size the complete device buffers including the surfaces'
    // lifecycle packets. Reserve the charge before allocating their backing.
    let (chunk_charge, chunks) = if svga {
        let sized = svga::encode::translate(&mut dx_objects.clone(), &stale, &words, &resolved, dx_extras(), None, None);
        let Some((sized, _)) = sized else {
            release_slot(false);
            return refuse(&mut a, 0, stream::error::NO_MEMORY, ENOMEM);
        };
        // Surface sizes are known now. Recount with their definitions, uploads and
        // destruction present, so chunk boundaries match the allocation pass.
        let sized = svga::encode::translate(&mut dx_objects.clone(), &stale, &words, &resolved,
            dx_extras(), None, Some(sized.sizes));
        let Some((sized, _)) = sized else {
            release_slot(false);
            return refuse(&mut a, 0, stream::error::NO_MEMORY, ENOMEM);
        };
        let charge = match Charge::new(tgid, sized.sizes.bytes(), true) {
            Ok(charge) => charge,
            Err(e) => {
                release_slot(false);
                return if e == ENOMEM { refuse(&mut a, 0, stream::error::NO_MEMORY, ENOMEM) } else { Err(e) };
            }
        };
        let translated = svga::encode::Backing::allocate(&sized.sizes)
            .and_then(|backing| svga::encode::translate(&mut dx_objects, &stale, &words, &resolved, dx_extras(), Some(backing), None));
        let Some((out, _)) = translated else {
            release_slot(false);
            return refuse(&mut a, 0, stream::error::NO_MEMORY, ENOMEM);
        };
        (alloc::sync::Arc::new(charge), Some(Chunks::Svga(out.chunks)))
    } else {
        // Size first-use programs and user commands before sending either.
        // The sizing object state includes the programs the warm-up will create.
        let mut sized_objects = objects.clone();
        let warm_bytes = (|| -> Result<u64, i64> {
            let mut bytes = 0u64;
            let mut at = 0;
            while at < words.len() {
                if words.word(at) == stream::cmd::PROBE
                    && sized_objects.probe[words.word(at + 2) as usize].1 == 0 {
                    let mut enc = encode::Enc::size_only();
                    encode::warm_probe(&mut enc, &mut sized_objects, default_vb, scratch, &words, at, FLIP_Y);
                    bytes = bytes.checked_add(enc.backing_bytes().ok_or(ENOMEM)?).ok_or(ENOMEM)?;
                }
                at += words.word(at + 1) as usize;
            }
            Ok(bytes)
        })();
        let mut sizing = encode::Enc::size_only();
        translate(&mut sizing, &mut sized_objects);
        let chunk_charge = match warm_bytes.and_then(|warm| {
            sizing.backing_bytes().and_then(|bytes| bytes.checked_add(warm))
                .and_then(|bytes| bytes.checked_add(pages(encode::vertex_bytes(plan.drawn + u64::from(corrupt.is_some()))))).ok_or(ENOMEM)
        }).and_then(|bytes| Charge::new(tgid, bytes, true)) {
            Ok(charge) => alloc::sync::Arc::new(charge),
            Err(e) => {
                drop_vertex_buffer();
                release_slot(false);
                return if e == ENOMEM { refuse(&mut a, 0, stream::error::NO_MEMORY, ENOMEM) } else { Err(e) };
            }
        };
        // Warm each pair separately; command backing retains the reservation even if the
        // waiter leaves while the device still owns it. Publish programs after their chunks succeed.
        let mut warm_sent = false;
        let prewarm = (|| -> Result<(), i64> {
            let mut at = 0;
            while at < words.len() {
                if words.word(at) == stream::cmd::PROBE
                    && objects.probe[words.word(at + 2) as usize].1 == 0 {
                    let mut enc = encode::Enc::new();
                    encode::warm_probe(&mut enc, &mut objects, default_vb, scratch, &words, at, FLIP_Y);
                    let chunks = enc.finish().filter(|v| !v.is_empty()).ok_or(ENOMEM)?;
                    for mut chunk in chunks {
                        chunk.charge_copy(chunk_charge.clone());
                        let (serial, fence) = with_core(|c| (c.serial(), c.device_fence())).ok_or(ENODEV)?;
                        let req = virgl::submit(Tag::Sync(serial), id, Some(fence), chunk);
                        warm_sent = true;
                        let done = exec_req(serial, Request::Virgl(req), id, &[default_vb, scratch, scratch_texture], &[])?;
                        if !virgl::ok(done.answer) || done.echoed != fence {
                            return Err(EIO);
                        }
                    }
                    with_core(|c| if let Some(ctx) = c.contexts.get_mut(&id) { ctx.objects = objects.clone(); });
                }
                at += words.word(at + 1) as usize;
            }
            Ok(())
        })();
        if let Err(error) = prewarm {
            if warm_sent {
                // A partial shader or an interrupted fence has ambiguous device state.
                // Do not let a subsequent call bind it or reuse its object handles.
                with_core(|c| c.quarantine(id, now_ns()));
            }
            drop_vertex_buffer();
            release_slot(false);
            return Err(error);
        }
        // Command frames and device requests come only after the quota reservation.
        let mut enc = encode::Enc::new();
        translate(&mut enc, &mut objects);
        let chunks = match enc.finish() {
            Some(chunks) => Some(Chunks::Virgl(chunks)),
            None => {
                drop_vertex_buffer();
                release_slot(false);
                return refuse(&mut a, 0, stream::error::NO_MEMORY, ENOMEM);
            }
        };
        (chunk_charge, chunks)
    };
    let made = (|| {
        if svga {
            return Ok(());
        }
        if let Some((rid, _, capacity)) = vertex_range.filter(|&(_, _, capacity)| capacity != 0) {
            create_private(id, rid, wire::target::BUFFER, wire::format::R8_UNORM,
                wire::bind::VERTEX_BUFFER, capacity, 1)?;
            with_core(|c| {
                if let Some(arena) = c.contexts.get_mut(&id).and_then(|ctx| ctx.vertex_arena.as_mut()) {
                    if arena.id == rid { arena.ready = true; }
                }
            });
        }
        for ((rid, f), [w, h]) in copy_scratch.into_iter().zip([format::B8G8R8A8, format::R8G8B8A8]).zip(plan.scratch) {
            if rid != 0 && with_core(|c| c.resources.get(&rid).is_some_and(|r| r.lost)).unwrap_or(false) {
                create_private(id, rid, wire::target::TEXTURE_2D, virgl_format(f),
                    wire::bind::RENDER_TARGET | wire::bind::SAMPLER_VIEW, w, h)?;
            }
        }
        if data_texture != 0 && with_core(|c| c.resources.get(&data_texture).is_some_and(|r| r.lost)).unwrap_or(false) {
            create_private(id, data_texture, wire::target::TEXTURE_2D, wire::format::R8G8B8A8_UNORM,
                wire::bind::RENDER_TARGET | wire::bind::SAMPLER_VIEW, data_width, (plan.data_bytes / 4).div_ceil(data_width))?;
        }
        Ok::<(), i64>(())
    })();
    if let Err(e) = made {
        with_core(|c| {
            let ctx = c.contexts.get_mut(&id);
            let failed = ctx.and_then(|ctx| {
                if ctx.vertex_arena.as_ref().is_some_and(|a| !a.ready) { ctx.vertex_arena.take() } else { None }
            });
            if let Some(arena) = failed { c.unhold(arena.id); }
        });
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
        if let Chunks::Virgl(chunks) = &chunks {
            if chunks.iter().any(|w| w.page_count() + 2 > c.queue_size) || c.queue_size < 2 {
                return Err(ENOMEM);
            }
        }
        // Test point 27, `device-error`: the chunks are followed by a fenced command of a
        // type VirtIO does not define, which the device can only refuse, and that command
        // carries the fence. Taken only once the submission is certain to be sent.
        let forced = crate::test_control::take(test_point::DEVICE_ERROR, 0).is_some();
        let serial = writer.serial;
        let fence = writer.fence;
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
        if let Some(d) = ctx.depth.as_mut().filter(|d| d.left > 0) {
            d.left -= 1;
            d.kept.push(serial);
        }
        let mut reqs: Vec<Request> = Vec::new();
        match chunks {
            Chunks::Virgl(chunks) => {
                let count = chunks.len();
                for (i, words) in chunks.into_iter().enumerate() {
                    let last = i + 1 == count && !forced;
                    reqs.push(dev::submit_virgl(Tag::Submit { ctx: id, serial, last }, id, last.then_some(fence), words));
                }
                if reqs.is_empty() && !forced {
                    // An empty translation still needs a fenced command to complete in order.
                    reqs.push(dev::submit_virgl(Tag::Submit { ctx: id, serial, last: true }, id, Some(fence), WordBuf::new()));
                }
            }
            Chunks::Svga(chunks) => {
                let count = chunks.len();
                for (i, chunk) in chunks.into_iter().enumerate() {
                    let last = i + 1 == count && !forced;
                    reqs.push(dev::submit_svga(Tag::Submit { ctx: id, serial, last }, id, last.then_some(fence), chunk).ok_or(EIO)?);
                }
            }
        }
        if forced {
            reqs.push(dev::undefined_command(Tag::Submit { ctx: id, serial, last: true }, id, fence).ok_or(EIO)?);
        }
        let ctx = c.contexts.get_mut(&id).ok_or(EINVAL)?;
        if svga {
            ctx.dx = dx_objects;
        } else {
            ctx.objects = objects;
        }
        let n = writer.n;
        let ns = ctx.ns;
        ctx.inflight.push_back(serial);
        if let Some(arena) = ctx.vertex_arena.as_mut() { arena.accepted = true; }
        for scratch in &mut ctx.private_scratch { scratch.accepted = true; }
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
        for rid in named_ids.iter().filter(|rid| !admitted_ids.contains(rid)) {
            if let Some(r) = c.resources.get_mut(rid) { r.holds += 1; }
        }
        c.subs.insert(serial, Sub {
            writer: Some(writer.clone()),
            ctx: id,
            n,
            fence,
            sent_ns: None,
            error: None,
            mismatch: false,
            requests: reqs.len() as u32,
            published: false,
            result: None,
            written: written.clone(),
            whole,
            held,
            present: None,
            _charge: Some(chunk_charge),
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

/// Reserve vertices in the context's arena. The context holds its resource until
/// destruction/replacement; each submission adds a hold through genuine completion.
/// A growth keeps the old arena charged and alive for its outstanding submissions.
/// Return (resource, byte offset, capacity needing device creation, or 0 if ready).
fn vertex_buffer_for(tgid: u64, ctx: u32, serial: u64, bytes: u64) -> Result<Option<(u32, u32, u32)>, i64> {
    if bytes == 0 { return Ok(None); }
    let requested = u32::try_from(bytes).map_err(|_| ENOMEM)?;
    let reused = with_core(|c| {
        let arena = c.contexts.get_mut(&ctx)?.vertex_arena.as_mut()?;
        let offset = arena.ranges.reserve(serial, requested)?;
        let rid = arena.id;
        let capacity = if arena.ready { 0 } else { arena.ranges.capacity };
        c.resources.get_mut(&rid)?.holds += 1;
        Some((rid, offset, capacity))
    }).flatten();
    if let Some(range) = reused { return Ok(Some(range)); }
    let mut ranges = vertex_arena::Ranges::new(bytes).ok_or(ENOMEM)?;
    let offset = ranges.reserve(serial, requested).ok_or(ENOMEM)?;
    let capacity = ranges.capacity;
    let charge = Charge::new(tgid, capacity as u64, true)?;
    let mut backing = Pages::none();
    backing.charge_copy(charge);
    let rid = with_core(|c| {
        if !c.contexts.get(&ctx).is_some_and(|x| x.state == CtxState::Live && !x.dropped) { return Err(EIO); }
        let rid = c.next_res;
        c.next_res += 1;
        let mut r = new_res(0, kind::BUFFER, format::B8G8R8A8, 0, 0, capacity as u64, 0, Arc::new(backing), c.epoch);
        r.dropped = true;
        r.lost = true;
        r.holds = 2; // context ownership and this submission's access
        c.insert_res(rid, r);
        let old = c.contexts.get_mut(&ctx).ok_or(EIO)?.vertex_arena.replace(VertexArena { id: rid, ready: false, accepted: false, ranges });
        if let Some(old) = old { c.unhold(old.id); }
        Ok(rid)
    }).ok_or(ENODEV)??;
    Ok(Some((rid, offset, capacity)))
}

/// Reuse an idle context-private copy/data texture. At most four per class can be
/// needed by the four in-flight submissions; the owner pays their page-rounded storage.
/// A submission's hold prevents reuse until device access and fence publication end.
fn scratch_for(tgid: u64, ctx: u32, class: u32, w: u32, h: u32) -> Result<u32, i64> {
    let reused = with_core(|c| {
        let context = c.contexts.get(&ctx)?;
        let rid = context.private_scratch.iter().find_map(|s| {
            let r = c.resources.get(&s.id)?;
            (s.class == class && !r.lost && r.holds == 1 && r.width >= w && r.height >= h).then_some(s.id)
        })?;
        c.resources.get_mut(&rid)?.holds += 1;
        Some(rid)
    }).flatten();
    if let Some(rid) = reused { return Ok(rid); }
    // Retire idle entries that cannot fit before growing the class. Busy entries keep
    // their submission holds and remain in the cache until they become idle.
    with_core(|c| {
        let idle: Vec<u32> = c.contexts.get(&ctx).map(|context| context.private_scratch.iter()
            .filter(|s| s.class == class && c.resources.get(&s.id).is_some_and(|r| r.holds == 1))
            .map(|s| s.id).collect()).unwrap_or_default();
        if let Some(context) = c.contexts.get_mut(&ctx) { context.private_scratch.retain(|s| !idle.contains(&s.id)); }
        for rid in idle { c.unhold(rid); }
    });
    let bytes = (w as u64 * 4).next_multiple_of(256).checked_mul(h as u64).ok_or(ENOMEM)?;
    let charge = Charge::new(tgid, bytes.div_ceil(PAGE as u64) * PAGE as u64, true)?;
    let mut backing = Pages::none(); backing.charge_copy(charge);
    with_core(|c| {
        let context = c.contexts.get(&ctx).ok_or(EIO)?;
        if context.state != CtxState::Live || context.dropped { return Err(EIO); }
        if context.private_scratch.iter().filter(|s| s.class == class).count() >= limits::IN_FLIGHT { return Err(ENOMEM); }
        let rid = c.next_res; c.next_res += 1;
        let mut r = new_res(0, kind::TEXTURE, format::B8G8R8A8, w, h, 0, 0, Arc::new(backing), c.epoch);
        r.dropped = true; r.lost = true; r.holds = 2;
        c.insert_res(rid, r);
        c.contexts.get_mut(&ctx).ok_or(EIO)?.private_scratch.push(PrivateScratch { id: rid, class, accepted: false });
        Ok(rid)
    }).ok_or(ENODEV)?
}

/// Start a private resource only after the submission's full quota is reserved.
fn create_private(ctx: u32, id: u32, target: u32, vformat: u32, bind: u32, w: u32, h: u32) -> Result<(), i64> {
    with_core(|c| { if let Some(r) = c.resources.get_mut(&id) { r.lost = false; } });
    exec_ok(|t| Some(dev::Request::Virgl(virgl::resource_create(t, id, target, vformat, bind, w, h))), 0, &[id])?;
    exec_ok(|t| Some(dev::ctx_attach(t, ctx, id)), ctx, &[id])?;
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
    let device_ctx = dev::context_id(ctx);
    let sent = if skip {
        with_core(|c| (c.serial(), 0, dev::success())).ok_or((ENODEV, 0))
    } else {
        match with_core(|c| (c.serial(), c.device_fence())) {
            None => Err((ENODEV, 0)),
            Some((serial, fence)) => {
                match dev::transfer_to_host(Tag::Sync(serial), ctx, fence, id, backing.clone(), x as u32, y as u32, cw as u32, ch as u32, offset, pitch as u32) {
                    None => Err((EIO, 0)),
                    Some(req) => match exec_req_why(serial, req, ctx, &[id], &[id]) {
                        Ok(d) if dev::ok(d.answer) && d.echoed == fence && d.fenced => {
                            Ok((serial, fence, d.answer))
                        }
                        Ok(_) => Err((EIO, serial)),
                        Err((e @ (EIO | EINTR), _)) => Err((e, serial)),
                        Err((e, _)) => Err((e, 0)),
                    },
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
                    r.last = Some(LastWrite { ctx, device_ctx, fence, serial, answer, kind: dev::upload_kind() });
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
    match read_back(tgid, &mut a, Observer::Owner, Some(arg)) {
        Ok(_) => Ok(0),
        Err(Refused::Errno(e)) => Err(e),
        Err(Refused::Eio(reason, failed)) => {
            a.reason = reason;
            a.failed_submission = failed;
            write_arg(arg, &a)?;
            Err(EIO)
        }
    }
}

/// `SCANOUT_READBACK` (syscall 414, op 22) for an observer the window layer authorized, root
/// or the compositor claimant: the whole bound scanout into `dst`, with record version 2 at
/// `record`, through the pinned-binding observation READBACK of target 0 uses, without
/// display ownership (gpu-abi.md, the window front handoff). `dst_len` holds at least the
/// current mode. Returns the observation serial, or the errno and, for EIO, its reason.
pub fn observe_scanout(dst: u64, dst_len: u64, record: u64) -> Result<u64, (i64, u32)> {
    // A kernel screen the console still owes a repaint is observed once it is drawn.
    crate::console::await_repaint();
    let _path = GpuPath::enter();
    if !matches!(with_core(|c| c.admission()), Some(Ok(()))) {
        return Err((ENODEV, readback_reason::NONE));
    }
    let tgid = caller().map_err(|e| (e, readback_reason::NONE))?;
    let mode = with_core(|c| display::MODES[c.display.mode as usize])
        .ok_or((ENODEV, readback_reason::NONE))?;
    if dst_len < mode.width as u64 * mode.height as u64 * 4 {
        return Err((EINVAL, readback_reason::NONE));
    }
    let mut a = abi::Readback::ZERO;
    a.pixels = dst;
    a.pixels_len = dst_len;
    a.record = record;
    match read_back(tgid, &mut a, Observer::Window, None) {
        Ok(serial) => Ok(serial),
        Err(Refused::Errno(e)) => Err((e, readback_reason::NONE)),
        Err(Refused::Eio(reason, _)) => Err((EIO, reason)),
    }
}

/// Who observes target 0, the scanout.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Observer {
    /// READBACK: the current display owner, of a rectangle it names.
    Owner,
    /// SCANOUT_READBACK: an observer the window layer authorized, of the whole bound
    /// target. It neither owns the display nor names the target, and an EIO leaves its
    /// record untouched.
    Window,
}

/// Why an observation was refused: an errno, or EIO with its reason and, for reason 4,
/// the invalidating submission.
enum Refused {
    Errno(i64),
    Eio(u32, u64),
}

impl From<i64> for Refused {
    fn from(e: i64) -> Self {
        Refused::Errno(e)
    }
}

/// Observe `a`'s rectangle of `a.target` (0, the scanout) for `tgid`, write the pixels and
/// the record, and return the observation serial. `arg`, when given, is READBACK's argument,
/// checked writable before any device work.
fn read_back(tgid: u64, a: &mut abi::Readback, observer: Observer, arg: Option<u64>) -> Result<u64, Refused> {
    let window = observer == Observer::Window;
    let _scanout = if a.target == 0 {
        if !window && !display::owner(tgid) {
            return Err(Refused::Errno(abi::errno::EPERM));
        }
        wait_for(None, true, |c| {
            (!console::restoring(c.display.owner_sequence)).then_some(c.display.known)
        })
        .map_err(|_| EINTR)?;
        Some(if window {
            display::Transition::for_window_observer()?
        } else {
            display::Transition::for_observation(tgid)?
        })
    } else {
        None
    };
    if a.target == 0
        && !with_core(|c| c.display.known && c.display.binding.is_some()).unwrap_or(false)
    {
        return Err(Refused::Eio(readback_reason::BINDING_UNKNOWN, 0));
    }
    let (id, fmt, tw, th, pitch, backing) = with_core(|c| {
        let id = if a.target == 0 {
            if !window && !display::owner(tgid) {
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
    // The console writes the kernel screen's backing, which a VirGL observation poisons and
    // transfers into. READBACK's display owner does not meet the console there: the console
    // stops drawing while a process holds the display.
    let canvas = core::cell::Cell::new((window && screen::is_target(id)).then(crate::console::hold_canvas));
    if window {
        // The whole bound target, in canonical rows.
        a.x = 0;
        a.y = 0;
        a.width = tw;
        a.height = th;
        a.stride = tw * format::bytes(fmt);
    }
    let (x, y, w, h) = (a.x as u64, a.y as u64, a.width as u64, a.height as u64);
    if w == 0 || h == 0 || x + w > tw as u64 || y + h > th as u64 {
        return Err(Refused::Errno(EINVAL));
    }
    // Canonical BGRA, or one byte per pixel of an R8 texture (record `canonical` 1).
    let row = w * format::bytes(fmt) as u64;
    if (a.stride as u64) < row || a.pixels_len < (h - 1) * a.stride as u64 + row {
        return Err(Refused::Errno(EINVAL));
    }
    // The argument, record and pixel-row pages are checked for mapping and writability
    // before any device work, serial or count (gpu-abi.md, READBACK's EFAULT).
    if let Some(arg) = arg {
        probe_writable(arg, core::mem::size_of::<abi::Readback>())?;
    }
    probe_writable(a.record, record::SIZE)?;
    // The rows' span is a user range, so the row arithmetic below cannot overflow; only the
    // rows' own pages are probed, excluding the gaps a wider stride leaves between them.
    if !crate::memory::layout::is_valid_user_range(
        a.pixels,
        ((h - 1) * a.stride as u64 + row) as usize,
    ) {
        return Err(Refused::Errno(EFAULT));
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
    let refuse = |reason: u32, failed: u64| -> Result<u64, Refused> { Err(Refused::Eio(reason, failed)) };
    match with_core(|c| display::readable(c, tgid, id, window)).ok_or(ENODEV)? {
        Ok(()) => {}
        Err(EIO) => return refuse(readback_reason::BINDING_UNKNOWN, 0),
        Err(e) => return Err(Refused::Errno(e)),
    }
    let staging = Staging::new(tgid, (row * h) as usize)?;
    // Wait for the target's accepted writes to complete, then take it for reading.
    // The handle is looked up each time: one destroyed meanwhile is target-dropped.
    let taken = wait_for(None, true, |c| {
        if a.target == 0 {
            if (!window && !display::owner(tgid))
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
        Ok(Err(reason)) => return refuse(reason, 0),
        Err(_) => return Err(Refused::Errno(EINTR)),
    };
    // The target is ours to poison and read until its transfers have ended; a transfer
    // whose waiter left keeps the mark until the device returns it, and with it the
    // console's hold, since the device may still write the backing the console draws in.
    let release = || {
        with_core(|c| {
            if let Some(hold) = canvas.take() {
                if c.syncs.values().any(|s| s.reading.contains(&id)) {
                    c.canvas.push((id, hold));
                }
            }
            c.end_reading(id);
        });
        notify();
    };
    if invalid {
        release();
        return refuse(readback_reason::TARGET_INVALID, invalidated_by);
    }
    let observation = match observe(tgid, id, last, &backing, pitch, fmt, *a, &staging.pages, None) {
        Ok(o) => o,
        Err((errno, reason)) => {
            release();
            return if errno == EIO {
                refuse(reason, 0)
            } else {
                Err(Refused::Errno(errno))
            };
        }
    };
    let (unwritten, transfer_fence, transfer_answer) =
        (observation.unwritten, observation.fence, observation.answer);
    let readback_surface = (observation.surface, observation.mob);
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
        let binding = display::binding(c, tgid, id, window)?;
        let s = c.next_obs;
        c.next_obs += 1;
        let resource = dev::resource_id(id).ok_or(EIO)?;
        let context = last.map_or(Some(0), |l| l.device_ctx).ok_or(EIO)?;
        Ok((s, c.caps.renderer, binding, resource, context))
    })
    .ok_or(ENODEV)?;
    let (serial, renderer, binding, device_resource, device_context) = match made {
        Ok(m) => m,
        Err(e) => {
            release();
            return if e == EIO {
                refuse(readback_reason::BINDING_UNKNOWN, 0)
            } else {
                Err(Refused::Errno(e))
            };
        }
    };
    put32(&mut rec, record::VERSION_OFF, record::VERSION);
    put32(&mut rec, record::SIZE_OFF, record::SIZE as u32);
    put64(&mut rec, record::SERIAL, serial);
    put32(&mut rec, record::BACKEND, dev::backend());
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
    put64(&mut rec, record::RESOURCE, device_resource as u64);
    put64(&mut rec, record::GENERATION, generation);
    if let Some(l) = last {
        put64(&mut rec, record::CONTEXT, device_context as u64);
        put64(&mut rec, record::RENDER_SERIAL, l.serial);
        put64(&mut rec, record::RENDER_FENCE, l.fence);
        put32(&mut rec, record::RENDER_STATUS, l.answer);
        put32(&mut rec, record::RENDER_KIND, l.kind);
    }
    put64(&mut rec, record::READBACK_FENCE, transfer_fence);
    put32(&mut rec, record::READBACK_STATUS, transfer_answer);
    if dev::is_svga() {
        // The host copy into the readback surface answered as the readback did: both run
        // in the same command buffer, whose status is the answer.
        put32(&mut rec, record::READBACK_KIND, svga::wire::cmd::READBACK_SURFACE);
        put64(&mut rec, record::READBACK_SURFACE, readback_surface.0);
        put64(&mut rec, record::READBACK_MOB, readback_surface.1);
        put32(&mut rec, record::COPY_KIND, svga::wire::cmd::SURFACE_COPY);
        put32(&mut rec, record::COPY_STATUS, transfer_answer);
    } else {
        put32(&mut rec, record::READBACK_KIND, wire::cmd::TRANSFER_FROM_HOST_3D);
    }
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
        // READBACK's record carries the unwritten count; a window observation's EIO
        // writes only its reason.
        if unwritten != 0 && window {
            return Ok(());
        }
        crate::syscall::userptr::write_user_bytes(a.record, rec.as_ptr(), record::SIZE)
            .map_err(|_| EFAULT)
    })();
    release();
    output?;
    if unwritten != 0 {
        return refuse(readback_reason::UNWRITTEN, 0);
    }
    with_core(|c| {
        if let Some(l) = last {
            if let Some(ctx) = c.contexts.get_mut(&l.ctx) {
                ctx.totals.readbacks += 1;
            }
        }
        c.proc_mut(tgid).totals.readbacks += 1;
    });
    Ok(serial)
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
/// Equal words, the case of a written target, cost one comparison each; only a word that
/// differs has its bytes counted.
fn differing(a: &[u8], b: &[u8]) -> u32 {
    let (wa, wb) = (a.chunks_exact(8), b.chunks_exact(8));
    let tail = wa.remainder().iter().zip(wb.remainder()).filter(|(p, q)| p != q).count() as u32;
    let mut count = tail;
    for (x, y) in wa.zip(wb) {
        let d = u64::from_le_bytes(x.try_into().unwrap_or_default())
            ^ u64::from_le_bytes(y.try_into().unwrap_or_default());
        if d != 0 {
            count += d.to_le_bytes().iter().filter(|&&b| b != 0).count() as u32;
        }
    }
    count
}

struct Observation {
    unwritten: u32,
    fence: u64,
    answer: u32,
    /// SVGA3D DX: the first pass's dedicated readback surface and its MOB.
    surface: u64,
    mob: u64,
}

/// Frames a device writes for one call, such as a readback surface's MOB: charged like
/// staging before they are allocated, and kept, with their charge, by each request that
/// may still write them.
fn device_frames(tgid: u64, bytes: usize) -> Result<Arc<Pages>, i64> {
    let frames = bytes.div_ceil(PAGE);
    let charge = Charge::new(tgid, (frames * PAGE) as u64, false)?;
    let mut pages = Pages::new(frames).ok_or(ENOMEM)?;
    pages.charge_copy(charge);
    Ok(Arc::new(pages))
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

/// A transfer of [`observe`] the device has: the request, the fence it carries, its place
/// in band-then-pass order, and test point 21's other resource when it names one.
#[derive(Clone, Copy)]
struct Sent {
    serial: u64,
    fence: u64,
    order: usize,
    other: Option<u32>,
}

/// A band of [`observe`]'s rectangle: its first row and row count, the pass it is on
/// (0 the poison, 1 its complement) and that pass's transfer, unless test point 22
/// skipped it.
#[derive(Clone, Copy)]
struct Band {
    index: usize,
    by: usize,
    rows: usize,
    pass: usize,
    sent: Option<Sent>,
}

/// Read a rectangle of target `id` into `staging`, band by band: each band is transferred
/// twice, into the poison and into its complement, and the bytes that differ between the
/// two are counted as unwritten (section 1.8). A transfer is a synchronous request
/// under the watchdog that holds the resource it names and the backing the device writes,
/// until the device returns it. SVGA copies into each band's dedicated readback surface;
/// VirGL transfers into the target backing. Two bands are observed at once, so the CPU fills, reads and
/// compares one band while the device transfers the other; within a band the two passes stay
/// in order. Bands begin on row boundaries and `op_resource_create` rounds a pitch up to a
/// multiple of 256 bytes, so two bands share no cache line. Returns the unwritten count and the fence and answer of the
/// first transfer in band-then-pass order. The reason of an EIO comes only from what the
/// kernel observes.
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
    preferred_ctx: Option<u32>,
) -> Result<Observation, (i64, u32)> {
    let ctx = with_core(|c| {
        // The scanout's writer may belong to any process, but not to a context being destroyed.
        let live = |cid: &u32| {
            c.contexts.get(cid).is_some_and(|x| {
                (if a.target == 0 { !x.dropped } else { x.owner == tgid })
                    && x.state == CtxState::Live
                    && x.device_alive
            })
        };
        if let Some(ctx) = preferred_ctx { return (live(&ctx).then_some(ctx), false); }
        let found = last.map(|l| l.ctx).filter(live).or_else(|| {
            if a.target == 0 {
                None
            } else {
                c.procs
                    .get(&tgid)
                    .and_then(|p| p.contexts.values().copied().find(live))
            }
        });
        // A target of this epoch outlives the contexts that wrote it while the binding
        // retains it; one from before a reset is lost.
        let current = !c.resetting && c.resources.get(&id).is_some_and(|r| !r.lost && r.epoch == c.epoch);
        (found, current)
    })
    .ok_or((ENODEV, readback_reason::NONE))?;
    let ctx = match ctx {
        (Some(ctx), _) => ctx,
        // The scanout's binding retains the target after the context that last wrote it
        // ended, and it stays observable (gpu-abi.md, op 22): the transfer runs in the
        // kernel screen's context.
        (None, true) if a.target == 0 => screen::observation_context(id).map_err(|e| {
            (e, if e == EIO { readback_reason::DEVICE_ERROR } else { readback_reason::NONE })
        })?,
        (None, _) if a.target == 0 => return Err((EIO, readback_reason::TARGET_LOST)),
        (None, _) => return Err((EINVAL, readback_reason::NONE)),
    };
    let px = format::bytes(fmt) as usize;
    let row = a.width as usize * px;
    let height = a.height as usize;
    let svga = dev::is_svga();
    let stride = if svga { row } else { pitch as usize };
    let band_rows = (READBACK_BAND / stride).max(1).min(height);
    // Each in-flight SVGA band has its own GPU-copy readback backing.
    let readbacks = if svga {
        Some([
            device_frames(tgid, band_rows * row).map_err(|e| (e, readback_reason::NONE))?,
            device_frames(tgid, band_rows * row).map_err(|e| (e, readback_reason::NONE))?,
        ])
    } else {
        None
    };
    let memory = |b: &Band| -> &Pages {
        match readbacks.as_ref() {
            Some(pages) => &pages[b.index % 2],
            None => backing,
        }
    };
    let surface = core::cell::Cell::new((0u64, 0u64));
    let bands = height.div_ceil(band_rows);
    let offset = |b: &Band| if svga { 0 } else { (a.y as usize + b.by) * pitch as usize + a.x as usize * px };
    // Poison band `b` with its pass's pattern and send its transfer, unless test point 22
    // skips it.
    let start = |b: &mut Band| -> Result<(), (i64, u32)> {
        let pattern = if b.pass == 0 { POISON } else { !POISON };
        let at = offset(b);
        for r in 0..b.rows {
            memory(b).fill(at + r * stride, row, pattern);
        }
        memory(b).maintain(at, (b.rows - 1) * stride + row, false);
        b.sent = None;
        if crate::test_control::take(test_point::SKIP_TRANSFER, a.target).is_some() {
            return Ok(());
        }
        let ty = (a.y as usize + b.by) as u32;
        let other = wrong_resource(tgid, id, pitch, a.x, ty, a.width, b.rows as u32, a.target);
        let other_id = other.as_ref().map(|(rid, _)| *rid);
        let (named, written) = other.unwrap_or((id, backing.clone()));
        let (serial, fence) = with_core(|c| (c.serial(), c.device_fence()))
            .ok_or((ENODEV, readback_reason::NONE))?;
        let req = match readbacks.as_ref() {
            Some(pages) => {
                let made = svga::readback(
                    Tag::Sync(serial), ctx, fence, id, (named != id).then_some(named),
                    fmt, a.x, ty, a.width, b.rows as u32, pages[b.index % 2].clone(),
                );
                let Some((req, sid, mob)) = made else {
                    release_other(other_id);
                    return Err((EIO, readback_reason::DEVICE_ERROR));
                };
                if surface.get().0 == 0 {
                    surface.set((sid as u64, mob as u64));
                }
                dev::Request::Svga(req)
            }
            None => dev::Request::Virgl(virgl::transfer_from_host(
                Tag::Sync(serial), ctx, fence, named, written, a.x, ty,
                a.width, b.rows as u32, at as u64, pitch,
            )),
        };
        // The target, and the other resource when the transfer names one.
        let both = [id, named];
        let marks = if named == id { &both[..1] } else { &both[..] };
        let sent = send_sync(serial, req, ctx, marks, marks);
        if sent.is_err() {
            release_other(other_id);
        }
        sent.map_err(device_reason)?;
        b.sent = Some(Sent { serial, fence, order: 2 * b.index + b.pass, other: other_id });
        Ok(())
    };
    let mut first: Option<(usize, u64, u32)> = None;
    // Take band `b`'s transfer back from the device and check its answer.
    let mut finish = |b: &mut Band| -> Result<(), (i64, u32)> {
        let Some(s) = b.sent.take() else { return Ok(()) };
        let done = await_sync(s.serial);
        release_other(s.other);
        let mut d = done.map_err(device_reason)?;
        // Test point 20, `stale-fence`: the answer is taken as echoing the next fence.
        if crate::test_control::take(test_point::STALE_FENCE, a.target).is_some() {
            d.echoed = d.echoed.wrapping_add(1);
        }
        if !dev::ok(d.answer) {
            return Err((EIO, readback_reason::DEVICE_ERROR));
        }
        if d.echoed != s.fence || !d.fenced {
            return Err((EIO, readback_reason::FENCE_MISMATCH));
        }
        if first.is_none_or(|(order, _, _)| s.order < order) {
            first = Some((s.order, s.fence, d.answer));
        }
        Ok(())
    };
    let mut seen = [0u8; PAGE];
    let mut chunk = [0u8; STAGING_CHUNK];
    let mut unwritten = 0u32;
    // The first observation lands straight in the staging; the second is compared with it
    // a frame at a time.
    let mut read = |b: &Band| {
        let at = offset(b);
        for r in 0..b.rows {
            let src = at + r * stride;
            let _ = staging.try_spans((b.by + r) * row, row, |done, ptr, n| {
                // SAFETY: the span lies inside one of the staging's frames, which only
                // this call uses.
                let kept = unsafe { core::slice::from_raw_parts_mut(ptr, n) };
                if b.pass == 0 {
                    observation::read(memory(b), src + done, kept);
                } else {
                    observation::read(memory(b), src + done, &mut seen[..n]);
                    unwritten += differing(&seen[..n], kept);
                }
                Ok::<(), ()>(())
            });
        }
    };
    let mut lanes: [Option<Band>; 2] = [None, None];
    let mut next = 0usize;
    let outcome = (|| -> Result<(), (i64, u32)> {
        // Start the next band, if any, in `lane`.
        let mut open = |lane: &mut Option<Band>| -> Result<(), (i64, u32)> {
            if next < bands {
                let by = next * band_rows;
                let b = lane.insert(Band { index: next, by, rows: band_rows.min(height - by), pass: 0, sent: None });
                next += 1;
                start(b)?;
            }
            Ok(())
        };
        for lane in lanes.iter_mut() {
            open(lane)?;
        }
        loop {
            let mut active = false;
            for lane in lanes.iter_mut() {
                let Some(b) = lane.as_mut() else { continue };
                active = true;
                finish(b)?;
                read(b);
                if b.pass == 0 {
                    b.pass = 1;
                    start(b)?;
                    continue;
                }
                if fmt == format::R8G8B8A8 {
                    // Canonical bytes are BGRA (record v2's `canonical` 0).
                    let bytes = b.rows * row;
                    let mut done = 0;
                    while done < bytes {
                        let n = (bytes - done).min(STAGING_CHUNK);
                        let at = b.by * row + done;
                        staging.read(at, &mut chunk[..n]);
                        for px in chunk[..n].chunks_exact_mut(4) {
                            px.swap(0, 2);
                        }
                        staging.write(at, &chunk[..n]);
                        done += n;
                    }
                }
                *lane = None;
                open(lane)?;
            }
            if !active {
                return Ok(());
            }
        }
    })();
    if let Err(e) = outcome {
        // A transfer still with the device keeps its marks and holds until it returns.
        for b in lanes.iter_mut().flatten() {
            if let Some(s) = b.sent.take() {
                with_core(|c| c.abandon(s.serial));
                release_other(s.other);
            }
        }
        return Err(e);
    }
    let (_, fence, answer) = first.unwrap_or((0, 0, 0));
    Ok(Observation {
        unwritten,
        fence,
        answer,
        surface: surface.get().0,
        mob: surface.get().1,
    })
}

/// An EIO's readback reason from a synchronous request's failure status.
fn device_reason((e, status): (i64, u32)) -> (i64, u32) {
    match status {
        fence_status::DEVICE_HUNG => (e, readback_reason::DEVICE_HUNG),
        fence_status::DEVICE_RESET => (e, readback_reason::DEVICE_RESET),
        _ => (e, readback_reason::DEVICE_ERROR),
    }
}

/// Clear test point 21's mark on resource `rid` once no request reads it. The mark stays
/// with a request whose waiter left before the answer.
fn release_other(rid: Option<u32>) {
    let Some(rid) = rid else { return };
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

/// Each process's completed presents, one `<tgid> <count>` line per process that has
/// presented (`/proc/breenix/stage-composites`): the kernel's record of when the suite's run
/// stage changed the screen, independent of the runner's own timestamps.
pub fn present_records() -> alloc::string::String {
    use core::fmt::Write;
    let owners: Vec<(u64, u64)> = with_core(|c| {
        c.procs.iter().filter(|(_, p)| p.presents > 0).map(|(&tgid, p)| (tgid, p.presents)).collect()
    })
    .unwrap_or_default();
    let mut out = alloc::string::String::new();
    for (tgid, presents) in owners {
        let _ = writeln!(out, "{tgid} {presents}");
    }
    out
}

/// Whether the clean-room device owns this ARM64 display path.
pub fn display_active() -> bool {
    display::active()
}
pub fn take_display(vt: usize) -> SyscallResult {
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
    let outcome = display::take(group, pid, vt);
    if outcome.is_ok() {
        crate::process::with_process_manager(|m| {
            if let Some(p) = m.get_process_mut(crate::process::ProcessId::new(pid)) {
                p.has_display_ownership = true;
            }
        });
        // The screen console stops drawing until the display comes back (`screen::restore`).
        crate::console::take_display();
    }
    result(outcome.map(|()| 0))
}
pub fn give_display() -> SyscallResult {
    let _path = GpuPath::enter();
    result(caller().and_then(display::back).map(|()| 0))
}
