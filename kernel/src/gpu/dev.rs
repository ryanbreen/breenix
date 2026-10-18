//! The device behind the GPU core: the VirGL backend (`virgl`, virtio-gpu) or the SVGA3D DX
//! backend (`svga`, VMware's SVGA3). The core builds requests and takes answers through
//! this module; each builder names the device operation and makes it in the active
//! backend's own commands. Answers keep their native values (`docs/gui/gpu.md` section
//! 1.8, ruling 47): VirtIO response types on VirGL, command-buffer statuses on SVGA3D.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use gpu_abi::{backend, kind};

use super::dma::{Pages, WordBuf};
use super::{svga, virgl};
pub use virgl::Tag;

static BACKEND: AtomicU32 = AtomicU32::new(backend::VIRGL);

/// The active backend's number (record v2 offset 16, CAPS offset 16).
pub fn backend() -> u32 {
    BACKEND.load(Ordering::Acquire)
}

pub fn is_svga() -> bool {
    backend() == backend::SVGA3D_DX
}

pub fn select_svga() {
    BACKEND.store(backend::SVGA3D_DX, Ordering::Release);
}

/// Native context identity, captured with a write's completion before context teardown.
pub fn context_id(ctx: u32) -> Option<u32> {
    if is_svga() { svga::cid_of(ctx) } else { Some(ctx) }
}

/// The device's native success answer.
pub fn success() -> u32 {
    if is_svga() { svga::wire::status::COMPLETED } else { virgl::wire::resp::OK_NODATA }
}

/// Whether `answer` is a success.
pub fn ok(answer: u32) -> bool {
    if is_svga() { svga::ok(answer) } else { virgl::ok(answer) }
}

/// The command a target's render answer names: SUBMIT_3D, or DX_DRAW.
pub fn draw_kind() -> u32 {
    if is_svga() { svga::wire::cmd::DX_DRAW } else { virgl::wire::cmd::SUBMIT_3D }
}

/// The command an upload's answer names: TRANSFER_TO_HOST_3D, or UPDATE_GB_SURFACE.
pub fn upload_kind() -> u32 {
    if is_svga() { svga::wire::cmd::UPDATE_SURFACE } else { virgl::wire::cmd::TRANSFER_TO_HOST_3D }
}

pub enum Device {
    Virgl(virgl::Device),
    Svga(svga::Device),
}

pub enum Request {
    Virgl(virgl::Request),
    Svga(svga::Request),
}

/// A request the device has returned: the answer, the fence it echoes, and whether the
/// answer carries a fence at all.
pub struct Done {
    pub tag: Tag,
    pub fence: Option<u64>,
    pub answer: u32,
    pub echoed: u64,
    pub fenced: bool,
}

impl Done {
    /// The answer of a request a confirmed reset ended.
    pub fn none(tag: Tag) -> Done {
        Done { tag, fence: None, answer: 0, echoed: 0, fenced: false }
    }
}

pub enum Reap {
    Empty,
    Done(Done),
    Fault,
}

impl Request {
    pub fn tag(&self) -> Tag {
        match self {
            Request::Virgl(r) => r.tag,
            Request::Svga(r) => r.tag,
        }
    }
    pub fn set_tag(&mut self, tag: Tag) {
        match self {
            Request::Virgl(r) => r.tag = tag,
            Request::Svga(r) => r.tag = tag,
        }
    }
    pub fn fence(&self) -> Option<u64> {
        match self {
            Request::Virgl(r) => r.fence,
            Request::Svga(r) => r.fence,
        }
    }
    /// Queue slots the request takes: descriptors on VirGL, one command buffer on SVGA3.
    pub fn segment_count(&self) -> usize {
        match self {
            Request::Virgl(r) => r.segment_count(),
            Request::Svga(_) => 1,
        }
    }
    /// An upload of a texture's contents, whose completion the core accounts.
    pub fn is_upload(&self) -> bool {
        match self {
            Request::Virgl(r) => r.command_type() == virgl::wire::cmd::TRANSFER_TO_HOST_3D,
            Request::Svga(r) => r.is_upload(),
        }
    }
}

impl Device {
    pub fn send(&mut self, req: Request) -> Result<(), Request> {
        match (self, req) {
            (Device::Virgl(d), Request::Virgl(r)) => d.send(r).map_err(Request::Virgl),
            (Device::Svga(d), Request::Svga(r)) => d.send(r).map_err(Request::Svga),
            (_, r) => Err(r),
        }
    }
    pub fn reap(&mut self) -> Reap {
        match self {
            Device::Virgl(d) => match d.reap() {
                virgl::Reap::Empty => Reap::Empty,
                virgl::Reap::Fault => Reap::Fault,
                virgl::Reap::Done(x) => {
                    let fenced = x.resp.u32_at(virgl::wire::HDR_FLAGS) & virgl::wire::FLAG_FENCE != 0;
                    Reap::Done(Done { tag: x.tag, fence: x.fence, answer: x.answer, echoed: x.echoed, fenced })
                }
            },
            Device::Svga(d) => match d.reap() {
                svga::Reap::Empty => Reap::Empty,
                svga::Reap::Fault => Reap::Fault,
                svga::Reap::Done(x) => Reap::Done(Done { tag: x.tag, fence: x.fence, answer: x.answer, echoed: x.echoed, fenced: x.fenced }),
            },
        }
    }
    pub fn has_room(&self, segments: usize) -> bool {
        match self {
            Device::Virgl(d) => d.has_room(segments),
            Device::Svga(d) => d.has_room(segments),
        }
    }
    pub fn outstanding(&self) -> usize {
        match self {
            Device::Virgl(d) => d.outstanding(),
            Device::Svga(d) => d.outstanding(),
        }
    }
    pub fn queue_size(&self) -> usize {
        match self {
            Device::Virgl(d) => d.queue_size(),
            Device::Svga(d) => d.queue_size(),
        }
    }
    /// The VirGL device signals a fault through its status; the SVGA device's faults are
    /// command-buffer statuses, seen when reaped.
    pub fn needs_reset(&self) -> bool {
        match self {
            Device::Virgl(d) => d.needs_reset(),
            Device::Svga(_) => false,
        }
    }
    pub fn begin_reset(&mut self) {
        match self {
            Device::Virgl(d) => d.begin_reset(),
            Device::Svga(d) => d.begin_reset(),
        }
    }
    pub fn reset_done(&mut self) -> bool {
        match self {
            Device::Virgl(d) => d.reset_done(),
            Device::Svga(d) => d.reset_done(),
        }
    }
    pub fn restart(&mut self) -> (Vec<Request>, Result<(), &'static str>) {
        match self {
            Device::Virgl(d) => {
                let (held, up) = d.restart();
                (held.into_iter().map(Request::Virgl).collect(), up)
            }
            Device::Svga(d) => {
                let (held, up) = d.restart();
                (held.into_iter().map(Request::Svga).collect(), up)
            }
        }
    }
    /// The VirGL control queue's notification address; 0 on SVGA3, whose submission is the
    /// register write itself.
    pub fn doorbell(&self) -> u64 {
        match self {
            Device::Virgl(d) => d.doorbell(),
            Device::Svga(_) => 0,
        }
    }
}

// Builders. Each makes one device operation the core names, and fails when the backend
// could not reserve the ids or memory the request needs.

pub fn ctx_create(tag: Tag, ctx: u32) -> Option<Request> {
    if is_svga() { svga::ctx_create(tag, ctx).map(Request::Svga) } else { Some(Request::Virgl(virgl::ctx_create(tag, ctx))) }
}

pub fn ctx_destroy(tag: Tag, ctx: u32) -> Request {
    if is_svga() { Request::Svga(svga::ctx_destroy(tag, ctx)) } else { Request::Virgl(virgl::ctx_destroy(tag, ctx)) }
}

pub fn ctx_attach(tag: Tag, ctx: u32, resource: u32) -> Request {
    if is_svga() { Request::Svga(svga::ctx_attach(tag)) } else { Request::Virgl(virgl::ctx_attach(tag, ctx, resource)) }
}

/// A resource of BGL kind `k` and format `f`: VirGL's target, format and bind for it, or
/// an SVGA surface (a buffer gets no surface).
pub fn resource_create(tag: Tag, id: u32, k: u32, f: u32, width: u32, height: u32, scanout: bool) -> Option<Request> {
    if is_svga() {
        return svga::resource_create(tag, id, k, f, width, height, scanout).map(Request::Svga);
    }
    use virgl::wire::{bind, format, target};
    let vformat = match (k, f) {
        (kind::BUFFER, _) => format::R8_UNORM,
        (_, gpu_abi::format::R8G8B8A8) => format::R8G8B8A8_UNORM,
        (_, gpu_abi::format::R8) => format::R8_UNORM,
        _ => format::B8G8R8A8_UNORM,
    };
    let (t, b) = match k {
        kind::BUFFER => (target::BUFFER, bind::VERTEX_BUFFER),
        kind::TARGET => (target::TEXTURE_2D, bind::RENDER_TARGET | bind::SAMPLER_VIEW | if scanout { bind::SCANOUT } else { 0 }),
        _ => (target::TEXTURE_2D, bind::SAMPLER_VIEW),
    };
    Some(Request::Virgl(virgl::resource_create(tag, id, t, vformat, b, width, height)))
}

pub fn attach_backing(tag: Tag, id: u32, backing: Arc<Pages>) -> Option<Request> {
    if is_svga() { svga::attach_backing(tag, id, backing).map(Request::Svga) } else { virgl::attach_backing(tag, id, backing).map(Request::Virgl) }
}

pub fn resource_unref(tag: Tag, id: u32, backing: Option<Arc<Pages>>) -> Request {
    if is_svga() { Request::Svga(svga::resource_unref(tag, id, backing)) } else { Request::Virgl(virgl::resource_unref(tag, id, backing)) }
}

/// An upload of the box `(x, y, w, h)` of resource `id`'s backing (at `offset`, `stride`
/// bytes per row) to the device, fenced in context `ctx`. SVGA3 uploads the whole surface
/// from its MOB, which the backing is.
#[allow(clippy::too_many_arguments)]
pub fn transfer_to_host(tag: Tag, ctx: u32, fence: u64, id: u32, backing: Arc<Pages>, x: u32, y: u32, w: u32, h: u32, offset: u64, stride: u32) -> Option<Request> {
    if is_svga() {
        svga::upload(tag, ctx, fence, id, backing).map(Request::Svga)
    } else {
        Some(Request::Virgl(virgl::transfer_to_host(tag, ctx, fence, id, backing, x, y, w, h, offset, stride)))
    }
}

pub fn undefined_command(tag: Tag, ctx: u32, fence: u64) -> Option<Request> {
    if is_svga() { svga::undefined_command(tag, ctx, fence).map(Request::Svga) } else { Some(Request::Virgl(virgl::undefined_command(tag, ctx, fence))) }
}

pub fn submit_virgl(tag: Tag, ctx: u32, fence: Option<u64>, words: WordBuf) -> Request {
    Request::Virgl(virgl::submit(tag, ctx, fence, words))
}

pub fn submit_svga(tag: Tag, ctx: u32, fence: Option<u64>, chunk: svga::encode::Chunk) -> Option<Request> {
    svga::submit(tag, ctx, fence, chunk).map(Request::Svga)
}

pub fn set_scanout(tag: Tag, ctx: u32, resource: u32, width: u32, height: u32) -> Option<Request> {
    if is_svga() { svga::set_scanout(tag, resource, width, height).map(Request::Svga) } else { Some(Request::Virgl(virgl::set_scanout(tag, ctx, resource, width, height))) }
}

pub fn flush_resource(tag: Tag, ctx: u32, fence: u64, resource: u32, width: u32, height: u32) -> Option<Request> {
    if is_svga() { svga::flush_resource(tag, ctx, fence, width, height).map(Request::Svga) } else { Some(Request::Virgl(virgl::flush_resource(tag, ctx, fence, resource, width, height))) }
}
