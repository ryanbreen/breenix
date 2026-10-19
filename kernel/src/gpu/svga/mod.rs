//! The SVGA3D DX backend's device: SVGA3 command buffers, guest-backed objects and DX
//! contexts on VMware Fusion (`docs/gui/gpu.md` section 1.6.3).
//!
//! Written from VMware's published interface headers (`svga_reg.h`, `svga3d_cmd.h`,
//! `svga3d_dx.h`, `svga3d_types.h`, `svga3d_devcaps.h`, `svga3d_limits.h`; pins and digests
//! in `docs/gui/provenance/gpu-core.md`). The GPU core drives it the way it drives the
//! VirGL device: it builds requests, queues them in order and takes their answers back. A
//! request here is one command buffer on queue 0. Its answer is the buffer's terminal
//! status; a fenced request also carries `DX_MOB_FENCE_64` into its own slot of the fence
//! MOB and is answered only once that slot holds its fence, which the device writes after
//! the buffer's earlier commands are finished.
//!
//! The device raises no interrupt this kernel can take on ARM64 Fusion (no GICv2m, and no
//! ITS or INTx support), so `kgpu` polls while requests are outstanding.
//!
//! The backend tracks which objects the device has. Each command that defines or destroys
//! a surface, MOB, DX context or the screen target records its byte offset in its buffer;
//! a COMPLETED answer applies the recorded effects, and a COMMAND_ERROR answer, which names
//! the failing command's offset, applies those before it. An object's ids go back to their
//! pools once it is unowned and the device no longer has it, so an id is not reused while
//! the device may still name it; an object a failed buffer left on the device is destroyed
//! by a buffer of the backend's own. A reset destroys the objects made before the reset
//! decision, in one buffer, and is confirmed when that buffer completes.

pub mod wire;
pub mod commands;
pub mod memory;
pub mod objects;
pub mod sm4;
pub mod encode;

use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::Arc;
use alloc::vec::Vec;
use crate::drivers::pci;
use crate::gpu::dma::{DmaBox, Pages, WordBuf, PAGE};
use crate::gpu::virgl::Tag;
use commands::Commands;
use memory::PageTree;

/// Fence slots of 8 bytes in the one-page fence MOB. Each fenced request takes the next;
/// at most `QUEUE_CAPACITY` buffers are outstanding, far fewer than the slots.
const FENCE_SLOTS: u64 = (PAGE / 8) as u64;

/// Entries in each context's view tables: the process's resource quota (256), the
/// context's own targets and textures, and views whose resources were destroyed since the
/// context's last submission.
pub const VIEWS: u32 = 1024;

#[derive(Clone, Copy, Debug)]
pub enum Error {
    NoDevice,
    Missing(u32),
    Refused(u32),
    Timeout,
    InvalidCommand,
    NoMemory,
    DeviceFault,
    /// The device lacks something this backend needs that is not a profile capability.
    Unsupported(&'static str),
}

/// Capability values read through SVGA_REG_DEV_CAP (gpu.md section 1.6.3, admission).
#[derive(Clone, Copy, Debug)]
pub struct Profile {
    pub dx: u32,
    pub bgra: u32,
    pub rgba: u32,
    pub r8: u32,
    pub width: u32,
    pub height: u32,
    pub screen_width: u32,
    pub screen_height: u32,
    pub contexts: u32,
    pub surfaces: u32,
}

impl Profile {
    pub fn missing(&self) -> Option<u32> {
        use gpu_abi::capability as abi;
        use wire::cap::*;
        if self.dx == 0 { Some(abi::FEATURE_3D) }
        else if self.bgra & (SUPPORTED | RENDER) != SUPPORTED | RENDER { Some(abi::BGRA_RENDER) }
        else if self.bgra & (SUPPORTED | SAMPLE) != SUPPORTED | SAMPLE { Some(abi::BGRA_SAMPLE) }
        else if self.rgba & (SUPPORTED | RENDER) != SUPPORTED | RENDER { Some(abi::RGBA_RENDER) }
        else if self.rgba & (SUPPORTED | SAMPLE) != SUPPORTED | SAMPLE { Some(abi::RGBA_SAMPLE) }
        else if self.r8 & (SUPPORTED | SAMPLE) != SUPPORTED | SAMPLE { Some(abi::R8_SAMPLE) }
        else if self.width < 2560 || self.height < 1600 || self.screen_width < 2560 || self.screen_height < 1600 { Some(abi::TARGET_SIZE) }
        else { None }
    }
}

// ---------------------------------------------------------------------------------------
// Object ids and the objects the device has.

/// Ids `lo..hi`, reused after release.
struct Pool {
    next: u32,
    hi: u32,
    free: Vec<u32>,
}

impl Pool {
    fn new(lo: u32, hi: u32) -> Pool {
        Pool { next: lo, hi, free: Vec::new() }
    }
    fn take(&mut self) -> Option<u32> {
        if let Some(id) = self.free.pop() {
            return Some(id);
        }
        (self.next < self.hi).then(|| {
            self.next += 1;
            self.next - 1
        })
    }
    fn give(&mut self, id: u32) {
        self.free.push(id);
    }
}

/// A MOB over guest pages, and whether the device has it.
struct Mob {
    id: u32,
    tree: Arc<PageTree>,
    bytes: u32,
    defined: bool,
}

/// The surface a core resource is on the device. A buffer has no surface: its bytes reach
/// the device through a submission's own vertex, constant and data surfaces.
struct Surface {
    sid: u32,
    format: u32,
    flags: u32,
    gen: u64,
    /// The core still has the resource.
    owned: bool,
    defined: bool,
    mob: Option<Mob>,
    /// The MOB is bound to the surface.
    bound: bool,
}

/// A core context's DX context and the MOBs behind it.
struct Dx {
    cid: u32,
    gen: u64,
    owned: bool,
    defined: bool,
    /// Which of the context's eight MOBs the device has, one bit each.
    mobs_defined: u8,
    objects: objects::Context,
}

/// A surface one submission or observation defines and destroys itself, with its MOB.
struct Transient {
    gen: u64,
    owned: bool,
    defined: bool,
    mob: Option<Mob>,
}

struct Ids {
    sids: Pool,
    mobs: Pool,
    cids: Pool,
    /// Counts reset decisions; objects made before the latest are destroyed by its reset.
    gen: u64,
    res: BTreeMap<u32, Surface>,
    ctxs: BTreeMap<u32, Dx>,
    transient: BTreeMap<u32, Transient>,
    /// The screen target as the device has it, and as the requests built so far leave it.
    screen: Option<(u32, u32)>,
    screen_plan: Option<(u32, u32)>,
    screen_sid: Option<u32>,
    /// The largest screen target the device takes (`SVGA_REG_SCREENTARGET_MAX_WIDTH` and
    /// `_HEIGHT`); a larger surface cannot carry the screen-target flag.
    screen_max: (u32, u32),
    /// An unowned object is still on the device.
    orphans: bool,
}

static IDS: spin::Mutex<Option<Ids>> = spin::Mutex::new(None);

fn with_ids<R>(f: impl FnOnce(&mut Ids) -> R) -> Option<R> {
    IDS.lock().as_mut().map(f)
}

impl Ids {
    /// Return the ids of the unowned objects the device does not have.
    fn collect(&mut self) {
        let gone: Vec<u32> = self.res.iter()
            .filter(|(_, s)| !s.owned && !s.defined && !s.mob.as_ref().is_some_and(|m| m.defined))
            .map(|(&id, _)| id).collect();
        for id in gone {
            if let Some(s) = self.res.remove(&id) {
                self.sids.give(s.sid);
                if let Some(m) = s.mob {
                    self.mobs.give(m.id);
                }
            }
        }
        let gone: Vec<u32> = self.ctxs.iter()
            .filter(|(_, d)| !d.owned && !d.defined && d.mobs_defined == 0)
            .map(|(&id, _)| id).collect();
        for id in gone {
            if let Some(d) = self.ctxs.remove(&id) {
                self.cids.give(d.cid);
                self.mobs.give(d.objects.state.id);
                for (_, m) in &d.objects.cotables {
                    self.mobs.give(m.id);
                }
            }
        }
        let gone: Vec<u32> = self.transient.iter()
            .filter(|(_, t)| !t.owned && !t.defined && !t.mob.as_ref().is_some_and(|m| m.defined))
            .map(|(&sid, _)| sid).collect();
        for sid in gone {
            if let Some(t) = self.transient.remove(&sid) {
                self.sids.give(sid);
                if let Some(m) = t.mob {
                    self.mobs.give(m.id);
                }
            }
        }
        self.orphans = self.res.values().any(|s| !s.owned)
            || self.ctxs.values().any(|d| !d.owned)
            || self.transient.values().any(|t| !t.owned);
    }

    /// Destroy commands for the objects `pick` selects that the device has, in the order
    /// that keeps each command valid: contexts (their views name surfaces), the screen
    /// target's binding, then surfaces and their MOBs.
    fn destroy(&self, b: &mut Builder, pick: impl Fn(u64, bool) -> bool) {
        for (&ctx, d) in &self.ctxs {
            if !pick(d.gen, d.owned) {
                continue;
            }
            if d.defined {
                b.c.command(wire::cmd::DX_BIND_CONTEXT, &[d.cid, wire::INVALID, 0]);
                b.mark(What::Context(ctx, false));
                b.c.command(wire::cmd::DX_DESTROY_CONTEXT, &[d.cid]);
            }
            let ids = core::iter::once(d.objects.state.id).chain(d.objects.cotables.iter().map(|(_, m)| m.id));
            for (i, id) in ids.enumerate() {
                if d.mobs_defined & (1 << i) != 0 {
                    b.mark(What::ContextMob(ctx, i as u8, false));
                    b.c.destroy_mob(id);
                }
            }
        }
        let screen_bound = |sid: u32| self.screen_sid == Some(sid);
        let mut unbound_screen = false;
        for (&id, s) in &self.res {
            if !pick(s.gen, s.owned) {
                continue;
            }
            if s.defined && screen_bound(s.sid) && !unbound_screen {
                b.mark(What::ScreenSid(None));
                b.c.unbind_screen();
                unbound_screen = true;
            }
            if s.defined && s.bound {
                b.mark(What::Bound(id, false));
                b.c.bind_surface(s.sid, wire::INVALID);
            }
            if s.defined {
                b.mark(What::Surface(id, false));
                b.c.destroy_surface(s.sid);
            }
            if let Some(m) = s.mob.as_ref().filter(|m| m.defined) {
                b.mark(What::Mob(id, false));
                b.c.destroy_mob(m.id);
            }
        }
        for (&sid, t) in &self.transient {
            if !pick(t.gen, t.owned) {
                continue;
            }
            if t.defined {
                if t.mob.as_ref().is_some_and(|m| m.defined) {
                    b.c.bind_surface(sid, wire::INVALID);
                }
                b.mark(What::Transient(sid, false));
                b.c.destroy_surface(sid);
            }
            if let Some(m) = t.mob.as_ref().filter(|m| m.defined) {
                b.mark(What::TransientMob(sid, false));
                b.c.destroy_mob(m.id);
            }
        }
    }
}

/// The device's program MOB: the catalogue programs' SM4 tokens, translated once.
pub struct Programs {
    pub mob: u32,
    /// Byte offset and SM4 byte size of each program: the vertex program, then the fragment
    /// programs in `encode::fragment_texts` order.
    pub at: Vec<(u32, u32)>,
}

static PROGRAMS: spin::Once<Programs> = spin::Once::new();

pub fn programs() -> Option<&'static Programs> {
    PROGRAMS.get()
}

/// The device's one-texel copy targets, one per surface format: an upload's command buffer
/// copies a texel of the uploaded surface into the one of its format, which makes the device
/// read the surface's MOB before the buffer completes (section 1.6.3).
static TOUCH: spin::Once<[(u32, u32); 3]> = spin::Once::new();

fn touch_sid(format: u32) -> Option<u32> {
    TOUCH.get()?.iter().find(|&&(f, _)| f == format).map(|&(_, sid)| sid)
}

/// The sid of core resource `id`, while it has one.
pub fn sid_of(id: u32) -> Option<u32> {
    with_ids(|ids| ids.res.get(&id).map(|s| s.sid)).flatten()
}

/// The DX context id of core context `ctx`.
pub fn cid_of(ctx: u32) -> Option<u32> {
    with_ids(|ids| ids.ctxs.get(&ctx).map(|d| d.cid)).flatten()
}

/// The reset decision: objects made from now on belong to the device after the reset.
pub fn reset_decided() {
    with_ids(|ids| ids.gen += 1);
}

/// Reserve a sid for a surface a request defines and destroys itself, with a MOB over
/// `pages` when given. The ids go back once the request that owns them is dropped and the
/// device no longer has the surface.
pub fn transient(pages: Option<Arc<Pages>>) -> Option<(u32, Option<u32>)> {
    let tree = match pages {
        Some(p) => Some((Arc::new(PageTree::new(p.clone())?), p.bytes() as u32)),
        None => None,
    };
    with_ids(|ids| {
        let sid = ids.sids.take()?;
        let mob = match tree {
            Some((tree, bytes)) => match ids.mobs.take() {
                Some(id) => Some(Mob { id, tree, bytes, defined: false }),
                None => {
                    ids.sids.give(sid);
                    return None;
                }
            },
            None => None,
        };
        let mob_id = mob.as_ref().map(|m| m.id);
        ids.transient.insert(sid, Transient { gen: ids.gen, owned: true, defined: false, mob });
        Some((sid, mob_id))
    })
    .flatten()
}

/// Give transient `sid` a MOB over `pages`; returns the MOB id.
pub fn transient_pages(sid: u32, pages: Arc<Pages>) -> Option<u32> {
    let bytes = pages.bytes() as u32;
    let tree = Arc::new(PageTree::new(pages)?);
    with_ids(|ids| {
        let id = ids.mobs.take()?;
        match ids.transient.get_mut(&sid) {
            Some(t) if t.mob.is_none() => {
                t.mob = Some(Mob { id, tree, bytes, defined: false });
                Some(id)
            }
            _ => {
                ids.mobs.give(id);
                None
            }
        }
    })
    .flatten()
}

/// Define a transient's MOB (when it has one) and surface, binding them, into `b`.
pub fn define_transient(b: &mut Builder, sid: u32, flags: u32, format: u32, width: u32, height: u32) {
    let mob = with_ids(|ids| ids.transient.get(&sid).and_then(|t| t.mob.as_ref().map(|m| (m.id, m.tree.clone(), m.bytes)))).flatten();
    if let Some((id, tree, bytes)) = &mob {
        b.mark(What::TransientMob(sid, true));
        b.c.mob(*id, tree, *bytes);
    }
    b.mark(What::Transient(sid, true));
    b.c.surface(sid, flags, format, width, height);
    if let Some((id, _, _)) = mob {
        b.c.bind_surface(sid, id);
    }
}

/// Destroy a transient's surface and MOB into `b`.
pub fn destroy_transient(b: &mut Builder, sid: u32) {
    let mob = with_ids(|ids| ids.transient.get(&sid).and_then(|t| t.mob.as_ref().map(|m| m.id))).flatten();
    if mob.is_some() {
        b.c.bind_surface(sid, wire::INVALID);
    }
    b.mark(What::Transient(sid, false));
    b.c.destroy_surface(sid);
    if let Some(id) = mob {
        b.mark(What::TransientMob(sid, false));
        b.c.destroy_mob(id);
    }
}

/// Give back a transient no request took.
pub fn forget_transient(sid: u32) {
    with_ids(|ids| {
        if let Some(t) = ids.transient.get_mut(&sid) {
            t.owned = false;
        }
        ids.collect();
    });
}

// ---------------------------------------------------------------------------------------
// Requests.

/// What a command establishes once the device has run it.
#[derive(Clone, Copy)]
pub enum What {
    Surface(u32, bool),
    Mob(u32, bool),
    Bound(u32, bool),
    Context(u32, bool),
    ContextMob(u32, u8, bool),
    Transient(u32, bool),
    TransientMob(u32, bool),
    Screen(Option<(u32, u32)>),
    ScreenSid(Option<u32>),
}

/// A command buffer being built: its commands, and the byte offset of each command whose
/// effect the backend tracks.
pub struct Builder {
    pub c: Commands,
    effects: Vec<(u32, What)>,
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

impl Builder {
    pub fn new() -> Builder {
        Builder { c: Commands::new(), effects: Vec::new() }
    }
    /// The next command has effect `what`.
    pub fn mark(&mut self, what: What) {
        self.effects.push((self.c.bytes(), what));
    }
    pub fn len_bytes(&self) -> usize {
        self.c.bytes() as usize
    }
    /// Split off the commands and effects so far into a chunk.
    pub fn take(&mut self) -> (Vec<u32>, Vec<(u32, What)>) {
        let c = core::mem::take(&mut self.c);
        (c.into_words(), core::mem::take(&mut self.effects))
    }
}

/// What a request holds for the device until it is dropped.
pub enum Keep {
    Tree(Arc<PageTree>),
    Pages(Arc<Pages>),
}

/// Ownership a request gives up when it is dropped.
#[derive(Clone, Copy)]
enum Release {
    Resource(u32),
    Context(u32),
    Transient(u32),
}

/// A command buffer's commands: built in place, or a translation's chunk in frames.
enum Payload {
    Words(Vec<u32>),
    Frames(WordBuf),
}

impl Payload {
    fn len(&self) -> usize {
        match self {
            Payload::Words(w) => w.len(),
            Payload::Frames(f) => f.len(),
        }
    }
    fn word(&self, i: usize) -> u32 {
        match self {
            Payload::Words(w) => w[i],
            Payload::Frames(f) => f.word(i),
        }
    }
}

/// One command buffer and everything the device may use for it.
pub struct Request {
    pub tag: Tag,
    pub fence: Option<u64>,
    /// The DX context the buffer's DX commands run in.
    dx: Option<u32>,
    words: Payload,
    keep: Vec<Keep>,
    effects: Vec<(u32, What)>,
    release: Vec<Release>,
    upload: bool,
    /// The backend's own buffer: its answer goes to no caller.
    internal: bool,
}

impl Request {
    fn new(tag: Tag, fence: Option<u64>, dx: Option<u32>, b: Builder) -> Request {
        Request {
            tag, fence, dx, words: Payload::Words(b.c.into_words()), keep: Vec::new(), effects: b.effects, release: Vec::new(),
            upload: false, internal: false,
        }
    }
    /// A request with no device work: it is answered COMPLETED in queue order.
    fn null(tag: Tag) -> Request {
        Request::new(tag, None, None, Builder::new())
    }
    pub fn is_upload(&self) -> bool {
        self.upload
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        if self.release.is_empty() {
            return;
        }
        let release = core::mem::take(&mut self.release);
        with_ids(|ids| {
            for r in release {
                match r {
                    Release::Resource(id) => {
                        if let Some(s) = ids.res.get_mut(&id) {
                            s.owned = false;
                        }
                    }
                    Release::Context(ctx) => {
                        if let Some(d) = ids.ctxs.get_mut(&ctx) {
                            d.owned = false;
                        }
                    }
                    Release::Transient(sid) => {
                        if let Some(t) = ids.transient.get_mut(&sid) {
                            t.owned = false;
                        }
                    }
                }
            }
            ids.collect();
        });
    }
}

/// Apply the effects of commands before byte `upto` of a buffer.
fn apply(effects: &[(u32, What)], upto: u32) {
    if effects.is_empty() {
        return;
    }
    with_ids(|ids| {
        for &(at, what) in effects {
            if at >= upto {
                continue;
            }
            match what {
                What::Surface(id, v) => {
                    if let Some(s) = ids.res.get_mut(&id) {
                        s.defined = v;
                    }
                }
                What::Mob(id, v) => {
                    if let Some(m) = ids.res.get_mut(&id).and_then(|s| s.mob.as_mut()) {
                        m.defined = v;
                    }
                }
                What::Bound(id, v) => {
                    if let Some(s) = ids.res.get_mut(&id) {
                        s.bound = v;
                    }
                }
                What::Context(ctx, v) => {
                    if let Some(d) = ids.ctxs.get_mut(&ctx) {
                        d.defined = v;
                    }
                }
                What::ContextMob(ctx, i, v) => {
                    if let Some(d) = ids.ctxs.get_mut(&ctx) {
                        if v {
                            d.mobs_defined |= 1 << i;
                        } else {
                            d.mobs_defined &= !(1 << i);
                        }
                    }
                }
                What::Transient(sid, v) => {
                    if let Some(t) = ids.transient.get_mut(&sid) {
                        t.defined = v;
                    }
                }
                What::TransientMob(sid, v) => {
                    if let Some(m) = ids.transient.get_mut(&sid).and_then(|t| t.mob.as_mut()) {
                        m.defined = v;
                    }
                }
                What::Screen(s) => ids.screen = s,
                What::ScreenSid(s) => ids.screen_sid = s,
            }
        }
        ids.collect();
    });
}

/// A request the device has answered: its buffer's status, and for a fenced request the
/// value its fence slot holds.
pub struct Done {
    pub tag: Tag,
    pub fence: Option<u64>,
    pub answer: u32,
    pub echoed: u64,
    pub fenced: bool,
}

pub enum Reap {
    Empty,
    Done(Done),
    Fault,
}

/// A request on the device: its command-buffer header and payload.
struct Sent {
    req: Request,
    header: Option<DmaBox>,
    _payload: Option<DmaBox>,
    id: u64,
    slot: Option<u64>,
}

pub struct Device {
    registers: u64,
    pub profile: Profile,
    pending: VecDeque<Sent>,
    tables: Vec<PageTree>,
    fence: Option<(u32, Arc<PageTree>)>,
    next_id: u64,
    next_slot: u64,
    stopped: bool,
    /// A reset in progress: the buffer destroying the old objects, once sent.
    resetting: Option<Sent>,
    /// A buffer destroying orphaned objects is on the device.
    cleaning: bool,
}

impl Device {
    pub fn probe() -> Result<Self, Error> {
        if !crate::platform_config::is_vmware() { return Err(Error::NoDevice); }
        let pci = pci::find_device(wire::VENDOR, wire::DEVICE).ok_or(Error::NoDevice)?;
        let bar = pci.bars[0];
        if bar.is_io || bar.address == 0 || bar.size < 4 * (wire::reg::IRQ_STATUS as u64 + 1) { return Err(Error::DeviceFault); }
        pci.enable_memory_space();
        pci.enable_bus_master();
        let registers = crate::memory::physical_memory_offset().as_u64() + bar.address;
        let mut dev = Self {
            registers,
            profile: Profile { dx: 0, bgra: 0, rgba: 0, r8: 0, width: 0, height: 0, screen_width: 0, screen_height: 0, contexts: 0, surfaces: 0 },
            pending: VecDeque::new(), tables: Vec::new(), fence: None, next_id: 1, next_slot: 0, stopped: true,
            resetting: None, cleaning: false,
        };
        dev.write(wire::reg::ID, wire::ID_3);
        if dev.read(wire::reg::ID) != wire::ID_3 { return Err(Error::DeviceFault); }
        let caps = dev.read(wire::reg::CAPABILITIES);
        if caps & wire::COMMAND_BUFFERS == 0 { return Err(Error::Missing(gpu_abi::capability::CAPABILITY_SET)); }
        if caps & wire::GB_OBJECTS == 0 { return Err(Error::Missing(gpu_abi::capability::CAPABILITY_SET)); }
        use wire::cap;
        dev.profile = Profile {
            dx: dev.cap(cap::DX_CONTEXT), bgra: dev.cap(cap::BGRA), rgba: dev.cap(cap::RGBA), r8: dev.cap(cap::R8),
            width: dev.cap(cap::MAX_TEXTURE_WIDTH).min(16384), height: dev.cap(cap::MAX_TEXTURE_HEIGHT).min(16384),
            screen_width: dev.read(wire::reg::SCREEN_MAX_WIDTH), screen_height: dev.read(wire::reg::SCREEN_MAX_HEIGHT),
            contexts: dev.cap(cap::MAX_CONTEXTS), surfaces: dev.cap(cap::MAX_SURFACES),
        };
        if let Some(reason) = dev.profile.missing() { return Err(Error::Missing(reason)); }
        // The vertex streams: positions as R32G32_FLOAT, constant records as R32G32B32A32_UINT.
        if dev.cap(cap::FLOAT2) & cap::VERTEX_BUFFER == 0 || dev.cap(cap::UINT4) & cap::VERTEX_BUFFER == 0 {
            return Err(Error::Unsupported("vertex stream formats"));
        }
        dev.write(wire::reg::IRQMASK, 0);
        dev.write(wire::reg::ENABLE, 1);
        dev.write(wire::reg::CONFIG_DONE, 1);
        dev.stopped = false;
        if let Err(e) = dev.start_queue().and_then(|_| dev.init_tables()).and_then(|_| dev.init_objects()) {
            // Initialization may have exposed a table to DMA. Failure is not evidence
            // that access ended, so retain it rather than freeing an ambiguous allocation.
            core::mem::forget(dev);
            return Err(e);
        }
        Ok(dev)
    }

    /// The current device mode, distinct from its screen-target size limits.
    pub fn scanout_size(&self) -> (u32, u32) {
        (self.read(wire::reg::WIDTH), self.read(wire::reg::HEIGHT))
    }

    fn read(&self, reg: u32) -> u32 {
        // SAFETY: BAR0 is mapped device memory, and reg is a published register index.
        unsafe { core::ptr::read_volatile((self.registers + 4 * reg as u64) as *const u32) }
    }
    fn write(&self, reg: u32, value: u32) {
        // SAFETY: as in read; accesses to paired registers are serialized by the owner.
        unsafe { core::ptr::write_volatile((self.registers + 4 * reg as u64) as *mut u32, value); }
    }
    fn cap(&self, index: u32) -> u32 {
        self.write(wire::reg::DEV_CAP, index);
        self.read(wire::reg::DEV_CAP)
    }
    fn issue(&self, header: &DmaBox, queue: u32) {
        let address = header.addr();
        assert_eq!(address & 63, 0, "SVGA command header alignment");
        assert!(queue <= 63, "SVGA queue selector");
        self.write(wire::reg::COMMAND_HIGH, (address >> 32) as u32);
        self.write(wire::reg::COMMAND_LOW, address as u32 | queue);
    }
    fn take_id(&mut self) -> u64 { let id = self.next_id; self.next_id += 1; id }

    /// A command buffer's header and payload, published for the device.
    fn buffer(&mut self, dx: Option<u32>, words: &[u32]) -> Result<(DmaBox, DmaBox, u64), Error> {
        self.buffer_of(dx, &Payload::Words(Vec::new()), words)
    }

    /// A command buffer of `body` followed by `tail`.
    fn buffer_of(&mut self, dx: Option<u32>, body: &Payload, tail: &[u32]) -> Result<(DmaBox, DmaBox, u64), Error> {
        let n = body.len() + tail.len();
        if n == 0 || n * 4 > wire::CB_MAX_BYTES { return Err(Error::InvalidCommand); }
        let id = self.take_id();
        let mut payload = DmaBox::new(n * 4);
        for i in 0..body.len() { payload.put_u32(i * 4, body.word(i)); }
        for (i, &word) in tail.iter().enumerate() { payload.put_u32((body.len() + i) * 4, word); }
        let mut header = DmaBox::new(wire::CB_HEADER);
        header.put_u64(8, id);
        header.put_u32(16, wire::CB_NO_IRQ | if dx.is_some() { wire::CB_DX_CONTEXT } else { 0 });
        header.put_u32(20, payload.len() as u32);
        header.put_u64(24, payload.addr());
        header.put_u32(36, dx.unwrap_or(0));
        payload.publish();
        header.publish();
        Ok((header, payload, id))
    }

    /// A device-context command: answered synchronously (`svga_reg.h`, device context).
    fn device_command(&mut self, words: &[u32]) -> Result<(), Error> {
        let (header, payload, _) = self.buffer(None, words)?;
        self.issue(&header, wire::CB_DEVICE);
        header.observe();
        match header.u32_at(0) {
            wire::status::COMPLETED => Ok(()),
            wire::status::PENDING => {
                core::mem::forget(header);
                core::mem::forget(payload);
                Err(Error::DeviceFault)
            }
            s => Err(Error::Refused(s)),
        }
    }

    /// Start (or restart after an error stopped it) command queue 0.
    fn start_queue(&mut self) -> Result<(), Error> {
        self.device_command(&[wire::dc::START_STOP_CONTEXT, 1, 0])
    }

    /// Run one buffer to completion (initialization only, before the core owns the device).
    fn exec_init(&mut self, words: &[u32]) -> Result<(), Error> {
        let (header, payload, id) = self.buffer(None, words)?;
        self.issue(&header, 0);
        let start = crate::signal::monotonic_nanos();
        loop {
            header.observe();
            let status = header.u32_at(0);
            if header.u64_at(8) != id { return Err(Error::DeviceFault); }
            if status == wire::status::COMPLETED { return Ok(()); }
            if status != wire::status::PENDING {
                return Err(Error::Refused(status));
            }
            if crate::signal::monotonic_nanos().saturating_sub(start) >= 2_000_000_000 {
                core::mem::forget(header);
                core::mem::forget(payload);
                return Err(Error::Timeout);
            }
            core::hint::spin_loop();
        }
    }

    /// The global object tables. Capacities: one MOB per surface; per DX context its state
    /// and seven context tables; the device's own fence and program MOBs.
    fn init_tables(&mut self) -> Result<(), Error> {
        let contexts = self.profile.contexts.min(256) as usize;
        let surfaces = self.profile.surfaces.min(32 * 1024) as usize;
        if contexts == 0 || surfaces < 256 { return Err(Error::DeviceFault); }
        let mobs = surfaces + contexts * 8 + 8;
        let capacities = [mobs, surfaces, 1, 1, 1, contexts];
        let widths = [wire::entry::MOB, wire::entry::SURFACE, wire::entry::CONTEXT,
            wire::entry::SHADER, wire::entry::SCREEN, wire::entry::DX_CONTEXT];
        for (kind, (&capacity, &width)) in capacities.iter().zip(widths.iter()).enumerate() {
            let bytes = capacity * width;
            let data = Arc::new(Pages::new(bytes.div_ceil(PAGE)).ok_or(Error::NoMemory)?);
            let tree = PageTree::new(data).ok_or(Error::NoMemory)?;
            let mut commands = Commands::new();
            commands.table(kind as u32, &tree, bytes as u32);
            // Publish ownership before issuing the table definition.
            self.tables.push(tree);
            self.exec_init(commands.words())?;
        }
        *IDS.lock() = Some(Ids {
            sids: Pool::new(1, surfaces as u32),
            mobs: Pool::new(1, mobs as u32),
            cids: Pool::new(0, contexts as u32),
            gen: 0,
            res: BTreeMap::new(),
            ctxs: BTreeMap::new(),
            transient: BTreeMap::new(),
            screen: None,
            screen_plan: None,
            screen_sid: None,
            screen_max: (self.profile.screen_width, self.profile.screen_height),
            orphans: false,
        });
        Ok(())
    }

    /// The device's own objects: the fence MOB, the program MOB with the catalogue's
    /// programs translated, and the one-texel copy targets.
    fn init_objects(&mut self) -> Result<(), Error> {
        let mut c = Commands::new();
        let fence_id = with_ids(|i| i.mobs.take()).flatten().ok_or(Error::NoMemory)?;
        let fence_tree = Arc::new(PageTree::new(Arc::new(Pages::new(1).ok_or(Error::NoMemory)?)).ok_or(Error::NoMemory)?);
        c.mob(fence_id, &fence_tree, PAGE as u32);
        // Programs, each at a 16-byte aligned offset.
        let mut words: Vec<u32> = Vec::new();
        let mut at = Vec::new();
        let mut place = |p: sm4::Program| {
            while words.len() % 4 != 0 { words.push(0); }
            at.push(((words.len() * 4) as u32, (p.code_words * 4) as u32));
            words.extend_from_slice(&p.words);
        };
        place(sm4::vertex());
        for text in encode::fragment_texts() {
            place(sm4::translate(text).map_err(|_| Error::InvalidCommand)?);
        }
        let bytes = words.len() * 4;
        let program_pages = Arc::new(Pages::new(bytes.div_ceil(PAGE)).ok_or(Error::NoMemory)?);
        for (i, w) in words.iter().enumerate() {
            program_pages.write(i * 4, &w.to_le_bytes());
        }
        program_pages.maintain(0, bytes, false);
        let program_id = with_ids(|i| i.mobs.take()).flatten().ok_or(Error::NoMemory)?;
        let program_tree = PageTree::new(program_pages).ok_or(Error::NoMemory)?;
        c.mob(program_id, &program_tree, bytes as u32);
        let mut touch = [(wire::format::BGRA, 0), (wire::format::RGBA, 0), (wire::format::R8, 0)];
        for (format, sid) in touch.iter_mut() {
            *sid = with_ids(|i| i.sids.take()).flatten().ok_or(Error::NoMemory)?;
            let render = if *format == wire::format::R8 { 0 } else { wire::bind::RENDER };
            c.surface(*sid, wire::bind::SAMPLE | render, *format, 1, 1);
        }
        // Owned before the device can use them; the program and fence MOBs live as long
        // as the device.
        self.tables.push(program_tree);
        self.fence = Some((fence_id, fence_tree));
        self.exec_init(c.words())?;
        PROGRAMS.call_once(|| Programs { mob: program_id, at });
        TOUCH.call_once(|| touch);
        Ok(())
    }

    /// Make `req` the device's: build and issue its command buffer, or give it back when
    /// the queue is full.
    pub fn send(&mut self, req: Request) -> Result<(), Request> {
        if self.stopped || self.resetting.is_some() || !self.has_room(1) {
            return Err(req);
        }
        if req.words.len() == 0 && req.fence.is_none() {
            self.pending.push_back(Sent { req, header: None, _payload: None, id: 0, slot: None });
            return Ok(());
        }
        // A fence is a DX command: a request without a DX context carries no fence.
        if req.fence.is_some() && req.dx.is_none() {
            return Err(req);
        }
        let mut tail = Commands::new();
        let mut slot = None;
        if let (Some(fence), Some((mob, tree))) = (req.fence, self.fence.as_ref()) {
            let s = self.next_slot % FENCE_SLOTS;
            // Clear the slot before the device can write it.
            tree.data.write(s as usize * 8, &0u64.to_le_bytes());
            tree.data.maintain(s as usize * 8, 8, false);
            tail.fence_at(*mob, s as u32 * 8, fence);
            slot = Some(s);
        }
        let (header, payload, id) = match self.buffer_of(req.dx, &req.words, tail.words()) {
            Ok(b) => b,
            Err(_) => return Err(req),
        };
        if slot.is_some() {
            self.next_slot += 1;
        }
        self.issue(&header, 0);
        self.pending.push_back(Sent { req, header: Some(header), _payload: Some(payload), id, slot });
        Ok(())
    }

    /// Whether the device holds fewer buffers than a queue holds.
    pub fn has_room(&self, _segments: usize) -> bool {
        self.pending.len() < wire::QUEUE_CAPACITY
    }

    pub fn queue_size(&self) -> usize {
        wire::QUEUE_CAPACITY
    }

    pub fn outstanding(&self) -> usize {
        self.pending.len()
    }

    /// The fence slot's value.
    fn slot_value(&self, slot: u64) -> u64 {
        let Some((_, tree)) = self.fence.as_ref() else { return 0 };
        tree.data.maintain(slot as usize * 8, 8, true);
        let mut b = [0u8; 8];
        tree.data.read(slot as usize * 8, &mut b);
        u64::from_le_bytes(b)
    }

    /// A buffer's terminal status and error offset, if it has one; Err for a status no
    /// buffer this backend sends can get.
    fn status(s: &Sent) -> Result<Option<(u32, u32)>, ()> {
        let Some(header) = s.header.as_ref() else { return Ok(Some((wire::status::COMPLETED, 0))) };
        header.observe();
        let status = header.u32_at(0);
        if header.u64_at(8) != s.id {
            return Err(());
        }
        match status {
            wire::status::PENDING => Ok(None),
            wire::status::COMPLETED | wire::status::COMMAND_ERROR => Ok(Some((status, header.u32_at(4)))),
            _ => Err(()),
        }
    }

    /// The next request the device has answered, in queue order. The backend's own
    /// buffers are taken here and not returned to the core.
    pub fn reap(&mut self) -> Reap {
        loop {
            let Some(front) = self.pending.front() else { return Reap::Empty };
            let (status, error_at) = match Self::status(front) {
                Ok(Some(s)) => s,
                Ok(None) => return Reap::Empty,
                Err(()) => return Reap::Fault,
            };
            let echoed = front.slot.map_or(0, |s| self.slot_value(s));
            // The fence slot is written once the buffer's work is finished, which may come
            // after the buffer's status. A nonzero wrong value is returned to the core
            // so it can report FENCE_MISMATCH rather than waiting for the watchdog.
            if status == wire::status::COMPLETED && front.req.fence.is_some() && echoed == 0 {
                return Reap::Empty;
            }
            let Some(sent) = self.pending.pop_front() else { return Reap::Empty };
            if status == wire::status::COMMAND_ERROR {
                // A command error stops the queue (svga_reg.h, SVGA_CB_STATUS_COMMAND_ERROR);
                // the buffers after it run once it is started again.
                if self.start_queue().is_err() {
                    // Hold the request: the device may still use what it names.
                    self.pending.push_front(sent);
                    return Reap::Fault;
                }
            }
            let upto = if status == wire::status::COMPLETED { u32::MAX } else { error_at };
            apply(&sent.req.effects, upto);
            let Sent { req, slot, .. } = sent;
            let internal = req.internal;
            if internal {
                self.cleaning = false;
            }
            let done = Done { tag: req.tag, fence: req.fence, answer: status, echoed, fenced: slot.is_some() };
            drop(req);
            self.clean_orphans();
            if !internal {
                return Reap::Done(done);
            }
        }
    }

    /// Destroy, in a buffer of the backend's own, unowned objects a failed buffer left on
    /// the device.
    fn clean_orphans(&mut self) {
        if self.cleaning || !with_ids(|ids| ids.orphans).unwrap_or(false) || !self.has_room(1) {
            return;
        }
        let mut b = Builder::new();
        with_ids(|ids| ids.destroy(&mut b, |_, owned| !owned));
        if b.c.is_empty() {
            return;
        }
        let mut req = Request::new(Tag::Background, None, None, b);
        req.internal = true;
        self.cleaning = self.send(req).is_ok();
    }

    /// Start a reset: preempt the queued buffers the device has not started
    /// (`SVGA_DC_CMD_PREEMPT`, queue 0, whatever the buffer's id). Buffers already started
    /// run to their end.
    pub fn begin_reset(&mut self) {
        let _ = self.device_command(&[wire::dc::PREEMPT, 0, 0]);
    }

    /// Whether the reset is confirmed: each buffer the device was given has a terminal
    /// status, and a buffer destroying the objects made before the reset decision, sent
    /// after them, has completed. Then the device names no memory of theirs.
    pub fn reset_done(&mut self) -> bool {
        if self.resetting.is_none() {
            let mut stopped = false;
            for s in &self.pending {
                let Some(h) = s.header.as_ref() else { continue };
                h.observe();
                match h.u32_at(0) {
                    wire::status::PENDING => return false,
                    wire::status::COMPLETED => apply(&s.req.effects, u32::MAX),
                    wire::status::COMMAND_ERROR => {
                        apply(&s.req.effects, h.u32_at(4));
                        stopped = true;
                    }
                    // Preempted or refused: its commands did not run.
                    _ => {}
                }
            }
            if stopped && self.start_queue().is_err() {
                return false;
            }
            let mut b = Builder::new();
            with_ids(|ids| {
                let gen = ids.gen;
                ids.destroy(&mut b, |g, _| g < gen);
                if ids.screen.is_some() && ids.screen_sid.is_none_or(|sid| !ids.res.values().any(|s| s.sid == sid && s.gen >= gen)) {
                    b.mark(What::Screen(None));
                    b.c.destroy_screen();
                }
            });
            if b.c.is_empty() {
                return true;
            }
            let (words, effects) = b.take();
            match self.buffer(None, &words) {
                Ok((header, payload, id)) => {
                    self.issue(&header, 0);
                    let mut req = Request::null(Tag::Background);
                    req.effects = effects;
                    req.internal = true;
                    self.resetting = Some(Sent { req, header: Some(header), _payload: Some(payload), id, slot: None });
                }
                Err(_) => return false,
            }
        }
        let Some(r) = self.resetting.as_ref() else { return true };
        match Self::status(r) {
            Ok(Some((wire::status::COMPLETED, _))) => {
                apply(&r.req.effects, u32::MAX);
                true
            }
            _ => false,
        }
    }

    /// After a confirmed reset: give back the requests the device held and accept
    /// requests again. Their objects are gone; dropping them returns the ids.
    pub fn restart(&mut self) -> (Vec<Request>, Result<(), &'static str>) {
        let held: Vec<Request> = self.pending.drain(..).map(|s| s.req).filter(|r| !r.internal).collect();
        self.resetting = None;
        self.cleaning = false;
        with_ids(|ids| {
            ids.screen_plan = ids.screen;
            let gen = ids.gen;
            for s in ids.res.values_mut().filter(|s| s.gen < gen) {
                s.owned = false;
            }
            for d in ids.ctxs.values_mut().filter(|d| d.gen < gen) {
                d.owned = false;
            }
            for t in ids.transient.values_mut().filter(|t| t.gen < gen) {
                t.owned = false;
            }
            ids.collect();
        });
        (held, Ok(()))
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        if !self.stopped {
            // A dropped owner is not a hardware reset. Retain DMA-visible allocations.
            for table in self.tables.drain(..) { core::mem::forget(table); }
            for sent in self.pending.drain(..) { core::mem::forget(sent); }
            if let Some(f) = self.fence.take() { core::mem::forget(f); }
            if let Some(r) = self.resetting.take() { core::mem::forget(r); }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Request builders the core calls, one per device operation it names.

/// Whether `answer` is a success.
pub fn ok(answer: u32) -> bool {
    answer == wire::status::COMPLETED
}

/// The SVGA format of a BGL format.
pub fn format_of(f: u32) -> u32 {
    match f {
        gpu_abi::format::R8G8B8A8 => wire::format::RGBA,
        gpu_abi::format::R8 => wire::format::R8,
        _ => wire::format::BGRA,
    }
}

/// A DX context for core context `ctx`, with its MOB-backed state and seven context
/// tables. Fails when ids or memory run out.
pub fn ctx_create(tag: Tag, ctx: u32) -> Option<Request> {
    let (cid, mobs) = with_ids(|ids| {
        let cid = ids.cids.take()?;
        let mut mobs = [0u32; 8];
        for i in 0..8 {
            match ids.mobs.take() {
                Some(id) => mobs[i] = id,
                None => {
                    for &taken in &mobs[..i] {
                        ids.mobs.give(taken);
                    }
                    ids.cids.give(cid);
                    return None;
                }
            }
        }
        Some((cid, mobs))
    })
    .flatten()?;
    let context = match objects::Context::new(cid, mobs, VIEWS, encode::SHADER_IDS) {
        Ok(c) => c,
        Err(_) => {
            with_ids(|ids| {
                ids.cids.give(cid);
                for m in mobs {
                    ids.mobs.give(m);
                }
            });
            return None;
        }
    };
    let mut b = Builder::new();
    b.mark(What::ContextMob(ctx, 0, true));
    context.state.define(&mut b.c);
    for (i, (_, m)) in context.cotables.iter().enumerate() {
        b.mark(What::ContextMob(ctx, i as u8 + 1, true));
        m.define(&mut b.c);
    }
    b.mark(What::Context(ctx, true));
    b.c.context(cid, context.state.id);
    for (kind, m) in &context.cotables {
        b.c.cotable(cid, m.id, *kind);
    }
    let mut req = Request::new(tag, None, None, b);
    for t in context.hold() {
        req.keep.push(Keep::Tree(t));
    }
    with_ids(|ids| {
        let gen = ids.gen;
        ids.ctxs.insert(ctx, Dx { cid, gen, owned: true, defined: false, mobs_defined: 0, objects: context })
    });
    Some(req)
}

/// Destroy core context `ctx`'s DX context and its MOBs. The context's memory stays with
/// the backend until the device no longer has its MOBs.
pub fn ctx_destroy(tag: Tag, ctx: u32) -> Request {
    let mut b = Builder::new();
    with_ids(|ids| {
        let Some(d) = ids.ctxs.get(&ctx) else { return };
        if d.defined {
            b.c.command(wire::cmd::DX_BIND_CONTEXT, &[d.cid, wire::INVALID, 0]);
            b.mark(What::Context(ctx, false));
            b.c.command(wire::cmd::DX_DESTROY_CONTEXT, &[d.cid]);
        }
        let mob_ids = core::iter::once(d.objects.state.id).chain(d.objects.cotables.iter().map(|(_, m)| m.id));
        for (i, id) in mob_ids.enumerate() {
            if d.mobs_defined & (1 << i) != 0 {
                b.mark(What::ContextMob(ctx, i as u8, false));
                b.c.destroy_mob(id);
            }
        }
    });
    let mut req = Request::new(tag, None, None, b);
    req.release.push(Release::Context(ctx));
    req
}

/// Context attachment has no SVGA counterpart: surfaces are device-wide. Answered in order.
pub fn ctx_attach(tag: Tag) -> Request {
    Request::null(tag)
}

/// The surface flags of a resource: render target and shader resource for targets (and the
/// screen-target flag for scanout targets), shader resource for textures.
fn surface_flags(target: bool, scanout: bool) -> u32 {
    use wire::bind::*;
    if target {
        RENDER | SAMPLE | if scanout { SCREEN } else { 0 }
    } else {
        SAMPLE
    }
}

/// DEFINE_GB_SURFACE (version 1) for a target or texture; a buffer gets no device object.
pub fn resource_create(tag: Tag, id: u32, kind: u32, format: u32, width: u32, height: u32, scanout: bool) -> Option<Request> {
    if kind == gpu_abi::kind::BUFFER {
        return Some(Request::null(tag));
    }
    let f = format_of(format);
    let sid = with_ids(|ids| {
        // A scanout target larger than the device's screen-target maximum is defined without
        // the screen-target flag, which the device refuses on it; it is not bound to the screen
        // (section 1.7: a target larger than the display is observed by readback).
        let fits = width <= ids.screen_max.0 && height <= ids.screen_max.1;
        let flags = surface_flags(kind == gpu_abi::kind::TARGET, scanout && fits);
        let sid = ids.sids.take()?;
        let gen = ids.gen;
        ids.res.insert(id, Surface { sid, format: f, flags, gen, owned: true, defined: false, mob: None, bound: false });
        Some(sid)
    })
    .flatten()?;
    let flags = with_ids(|ids| ids.res.get(&id).map(|s| s.flags)).flatten()?;
    let mut b = Builder::new();
    b.mark(What::Surface(id, true));
    b.c.surface(sid, flags, f, width, height);
    Some(Request::new(tag, None, None, b))
}

/// A MOB over `backing`, bound to resource `id`'s surface.
pub fn attach_backing(tag: Tag, id: u32, backing: Arc<Pages>) -> Option<Request> {
    if backing.count() == 0 || sid_of(id).is_none() {
        return Some(Request::null(tag));
    }
    let bytes = backing.bytes() as u32;
    let tree = Arc::new(PageTree::new(backing.clone())?);
    let (sid, mob) = with_ids(|ids| {
        let mob = ids.mobs.take()?;
        let s = ids.res.get_mut(&id)?;
        s.mob = Some(Mob { id: mob, tree: tree.clone(), bytes, defined: false });
        Some((s.sid, mob))
    })
    .flatten()?;
    let mut b = Builder::new();
    b.mark(What::Mob(id, true));
    b.c.mob(mob, &tree, bytes);
    b.mark(What::Bound(id, true));
    b.c.bind_surface(sid, mob);
    let mut req = Request::new(tag, None, None, b);
    req.keep.push(Keep::Tree(tree));
    Some(req)
}

/// Destroy resource `id`'s surface and MOB. The request keeps `backing` until the device
/// has answered; the backend keeps the MOB's page tree until the device no longer has it.
pub fn resource_unref(tag: Tag, id: u32, backing: Option<Arc<Pages>>) -> Request {
    let mut b = Builder::new();
    with_ids(|ids| {
        let screen = ids.screen_sid;
        let Some(s) = ids.res.get(&id) else { return };
        if s.defined && screen == Some(s.sid) {
            b.mark(What::ScreenSid(None));
            b.c.unbind_screen();
            ids.screen_sid = None;
        }
        if s.defined && s.bound {
            b.mark(What::Bound(id, false));
            b.c.bind_surface(s.sid, wire::INVALID);
        }
        if s.defined {
            b.mark(What::Surface(id, false));
            b.c.destroy_surface(s.sid);
        }
        if let Some(m) = s.mob.as_ref().filter(|m| m.defined) {
            b.mark(What::Mob(id, false));
            b.c.destroy_mob(m.id);
        }
    });
    let mut req = Request::new(tag, None, None, b);
    if let Some(p) = backing {
        req.keep.push(Keep::Pages(p));
    }
    req.release.push(Release::Resource(id));
    req
}

/// An upload of resource `id`'s backing to its surface, in context `ctx`, fenced:
/// UPDATE_GB_SURFACE, then a one-texel SURFACE_COPY of the surface, which makes the device
/// read the MOB before the buffer completes, so the backing may change once the fence is
/// published (section 1.6.3).
pub fn upload(tag: Tag, ctx: u32, fence: u64, id: u32, backing: Arc<Pages>) -> Option<Request> {
    let cid = cid_of(ctx)?;
    let (sid, format) = with_ids(|ids| ids.res.get(&id).map(|s| (s.sid, s.format))).flatten()?;
    let touch = touch_sid(format)?;
    let mut b = Builder::new();
    b.c.update_surface(sid);
    b.c.copy(sid, touch, [0, 0, 1, 1], [0, 0]);
    let mut req = Request::new(tag, Some(fence), Some(cid), b);
    req.keep.push(Keep::Pages(backing));
    req.upload = true;
    Some(req)
}

/// Test point 27: a fenced buffer whose only command is NOP_ERROR, which the device answers
/// with COMMAND_ERROR.
pub fn undefined_command(tag: Tag, ctx: u32, fence: u64) -> Option<Request> {
    let cid = cid_of(ctx)?;
    let mut b = Builder::new();
    b.c.nop_error();
    Some(Request::new(tag, Some(fence), Some(cid), b))
}

/// One chunk of a translated submission in context `ctx`; the last carries the fence.
pub fn submit(tag: Tag, ctx: u32, fence: Option<u64>, mut chunk: encode::Chunk) -> Option<Request> {
    let cid = cid_of(ctx)?;
    let mut req = Request::new(tag, fence, Some(cid), Builder::new());
    req.words = Payload::Frames(core::mem::replace(&mut chunk.words, WordBuf::new()));
    req.effects = core::mem::take(&mut chunk.effects);
    for sid in core::mem::take(&mut chunk.transient) {
        req.release.push(Release::Transient(sid));
    }
    Some(req)
}

/// Bind resource `id` to the screen target, defining the screen target at the resource's
/// size first when it is not that size.
pub fn set_scanout(tag: Tag, id: u32, width: u32, height: u32) -> Option<Request> {
    let mut b = Builder::new();
    with_ids(|ids| {
        let s = ids.res.get(&id)?;
        if s.flags & wire::bind::SCREEN == 0 {
            return None;
        }
        let sid = s.sid;
        if ids.screen_plan != Some((width, height)) {
            if ids.screen_plan.is_some() {
                b.mark(What::Screen(None));
                b.c.destroy_screen();
            }
            b.mark(What::Screen(Some((width, height))));
            b.c.screen(width, height);
            ids.screen_plan = Some((width, height));
        }
        b.mark(What::ScreenSid(Some(sid)));
        b.c.bind_screen(sid);
        Some(())
    })
    .flatten()?;
    Some(Request::new(tag, None, None, b))
}

/// Update the screen target from its bound surface, fenced in context `ctx`.
pub fn flush_resource(tag: Tag, ctx: u32, fence: u64, width: u32, height: u32) -> Option<Request> {
    let cid = cid_of(ctx)?;
    let mut b = Builder::new();
    b.c.update_screen_rect(0, 0, width, height);
    Some(Request::new(tag, Some(fence), Some(cid), b))
}

/// One observation pass, in context `ctx` and fenced: define a dedicated readback surface of
/// `w` x `h` on its own MOB over `pages`, GPU-copy the rectangle `(x, y, w, h)` of resource
/// `source` into it, READBACK_GB_SURFACE the surface named by `read` (the readback surface,
/// or for test point 21 another target) into its MOB, then destroy the readback surface.
/// Returns the request with the readback surface's sid and MOB id.
#[allow(clippy::too_many_arguments)]
pub fn readback(tag: Tag, ctx: u32, fence: u64, source: u32, read: Option<u32>, format: u32, x: u32, y: u32, w: u32, h: u32, pages: Arc<Pages>) -> Option<(Request, u32, u32)> {
    let cid = cid_of(ctx)?;
    let src = sid_of(source)?;
    let other = match read {
        Some(r) => Some(sid_of(r)?),
        None => None,
    };
    let (sid, mob) = transient(Some(pages.clone()))?;
    let mob = mob?;
    let mut b = Builder::new();
    define_transient(&mut b, sid, 0, format_of(format), w, h);
    b.c.copy(src, sid, [x, y, w, h], [0, 0]);
    b.c.readback_surface(other.unwrap_or(sid));
    destroy_transient(&mut b, sid);
    let mut req = Request::new(tag, Some(fence), Some(cid), b);
    req.keep.push(Keep::Pages(pages));
    req.release.push(Release::Transient(sid));
    Some((req, sid, mob))
}

/// Read a kernel screen target into its attached MOB, completed by a device fence.
pub fn download(tag: Tag, ctx: u32, fence: u64, resource: u32, pages: Arc<Pages>) -> Option<Request> {
    let cid = cid_of(ctx)?;
    let sid = sid_of(resource)?;
    let mut b = Builder::new();
    b.c.readback_surface(sid);
    let mut req = Request::new(tag, Some(fence), Some(cid), b);
    req.keep.push(Keep::Pages(pages));
    Some(req)
}

pub fn flush_rect(tag: Tag, ctx: u32, fence: u64, x: u32, y: u32, w: u32, h: u32) -> Option<Request> {
    let cid = cid_of(ctx)?;
    let mut b = Builder::new();
    b.c.update_screen_rect(x, y, w, h);
    Some(Request::new(tag, Some(fence), Some(cid), b))
}
