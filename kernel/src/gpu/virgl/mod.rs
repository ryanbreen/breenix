//! The VirGL backend's device: virtio-gpu over the modern PCI transport.
//!
//! Written from VirtIO 1.3 CSD01: device initialisation (section 3.1), the PCI transport
//! (section 4.1), split virtqueues (section 2.7) and the GPU device (section 5.7). The
//! control queue carries the commands; a fenced command's response arrives only after
//! the device has finished it and echoes its fence (section 5.7.6.7). Completion is
//! signalled by the transport's interrupt. On QEMU the device uses its PCI INTx pin and
//! the ISR status byte, whose read acknowledges it (section 4.1.4.5). On Parallels it uses
//! MSI-X (section 4.1.4.8, PCI Local Bus 3.0 section 6.8.2) through the GICv2m doorbell:
//! vector 0 for the control queue and vector 1 for configuration changes, each its own
//! edge-triggered SPI, and the driver does not read the ISR byte for a queue vector.
//!
//! A request owns every buffer the device may touch for it, including any resource
//! backing it reads or writes, until the device returns it or a confirmed reset ends the
//! device's use of it (sections 2.4 and 2.7.13); a device whose reset is not confirmed
//! keeps them. The device object lives under the GPU core's lock; nothing here blocks.

pub mod encode;
pub mod queue;
pub mod wire;
pub(crate) mod emergency;

use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::drivers::pci;
use crate::drivers::virtio::pci_transport::VirtioPciDevice;
use crate::gpu::dma::{DmaBox, Pages, WordBuf};
use queue::{Queue, Segment};
use wire::{cmd, resp};

/// What a request is for, so its completion goes to the right place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tag {
    /// A thread waits for this serial's answer.
    Sync(u64),
    /// One SUBMIT_3D of submission `serial` in device context `ctx`; `last` carries the
    /// submission's fence.
    Submit { ctx: u32, serial: u64, last: bool },
    /// Nobody waits; the answer is only counted.
    Background,
}

enum Payload {
    None,
    Words(WordBuf),
    Pages(Pages, usize),
}

/// A command in the device's hands, with every buffer it uses.
pub struct Request {
    pub tag: Tag,
    /// The fence the command carries, when it is fenced.
    pub fence: Option<u64>,
    hdr: DmaBox,
    payload: Payload,
    resp: DmaBox,
    /// Resource backing the device reads or writes for this command.
    keep: Option<Arc<Pages>>,
}

/// A request the device has returned.
pub struct Done {
    pub tag: Tag,
    pub fence: Option<u64>,
    /// The response type and the fence it echoes.
    pub answer: u32,
    pub echoed: u64,
    pub resp: DmaBox,
}

impl Request {
    /// The wire command type, used to retain upload completion accounting.
    pub(super) fn command_type(&self) -> u32 {
        self.hdr.u32_at(wire::HDR_TYPE)
    }

    /// A command of `len` header bytes (the control header and the command's fields)
    /// with a response of `resp_len` bytes.
    fn new(tag: Tag, ty: u32, ctx: u32, fence: Option<u64>, len: usize, resp_len: usize) -> Request {
        let mut hdr = DmaBox::new(len);
        hdr.put_u32(wire::HDR_TYPE, ty);
        if let Some(f) = fence {
            hdr.put_u32(wire::HDR_FLAGS, wire::FLAG_FENCE);
            hdr.put_u64(wire::HDR_FENCE, f);
        }
        hdr.put_u32(wire::HDR_CTX, ctx);
        Request { tag, fence, hdr, payload: Payload::None, resp: DmaBox::new(resp_len), keep: None }
    }

    fn field(mut self, at: usize, v: u32) -> Request {
        self.hdr.put_u32(wire::HDR + at, v);
        self
    }

    fn field64(mut self, at: usize, v: u64) -> Request {
        self.hdr.put_u64(wire::HDR + at, v);
        self
    }

    /// The descriptor chain: header, payload pages, response.
    fn segments(&self) -> Vec<Segment> {
        let mut segs = alloc::vec![Segment { addr: self.hdr.addr(), len: self.hdr.len() as u32, write: false }];
        match &self.payload {
            Payload::None => {}
            Payload::Words(w) => {
                let mut left = w.len() * 4;
                for i in 0..w.page_count() {
                    let n = left.min(crate::gpu::dma::PAGE);
                    if n == 0 {
                        break;
                    }
                    segs.push(Segment { addr: w.page(i).page_addr(0), len: n as u32, write: false });
                    left -= n;
                }
            }
            Payload::Pages(p, len) => {
                let mut left = *len;
                for i in 0..p.count() {
                    let n = left.min(crate::gpu::dma::PAGE);
                    if n == 0 {
                        break;
                    }
                    segs.push(Segment { addr: p.page_addr(i), len: n as u32, write: false });
                    left -= n;
                }
            }
        }
        segs.push(Segment { addr: self.resp.addr(), len: self.resp.len() as u32, write: true });
        segs
    }

    /// How many descriptors the request's chain uses.
    pub fn segment_count(&self) -> usize {
        self.segments().len()
    }

    fn publish(&self) {
        self.hdr.publish();
        match &self.payload {
            Payload::None => {}
            Payload::Words(w) => {
                for i in 0..w.page_count() {
                    w.page(i).maintain(0, crate::gpu::dma::PAGE, false);
                }
            }
            Payload::Pages(p, len) => p.maintain(0, *len, false),
        }
        self.resp.publish();
    }
}

/// A scanout as GET_DISPLAY_INFO reports it.
#[derive(Clone, Copy, Default)]
pub struct Scanout {
    pub enabled: bool,
    pub width: u32,
    pub height: u32,
}

/// What [`Device::reap`] found in the used ring.
pub enum Reap {
    Empty,
    Done(Done),
    /// An entry naming no chain the driver made available, or claiming more bytes than
    /// its writable buffer: a device fault (`docs/gui/gpu.md` section 1.4.1).
    Fault,
}

/// The device as initialisation found it.
pub struct Info {
    pub features: u64,
    /// Distinct capability set ids, ascending.
    pub capset_ids: Vec<u32>,
    /// The capability set read: id, version and bytes.
    pub capset: Option<(u32, u32, Vec<u8>)>,
    pub scanouts: Vec<Scanout>,
    pub num_scanouts: u32,
}

/// How the device interrupts.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Irq {
    /// One level-triggered INTx line for both causes, told apart by the ISR byte.
    Intx(u32),
    /// MSI-X: the control queue's SPI and the configuration-change SPI.
    Msix { queue: u32, config: u32 },
}

pub struct Device {
    pci: VirtioPciDevice,
    queue: Queue,
    inflight: Vec<Option<Request>>,
    pub info: Info,
    pub irq: Irq,
    /// The ISR status byte's address.
    pub isr: u64,
}

/// MSI-X vector numbers the driver assigns (VirtIO 1.3 section 4.1.4.3).
const QUEUE_VECTOR: u16 = 0;
const CONFIG_VECTOR: u16 = 1;

/// Features the backend asks for: VirGL, and the modern interface (section 6).
const WANTED: u64 = wire::F_VIRGL | wire::F_VERSION_1;

/// The modern virtio-gpu PCI device id (0x1040 + device type 16).
const GPU_PCI_ID: u16 = 0x1050;

/// The GIC interrupt id of PCI device `slot`'s INTx pin `pin` (1 = INTA) on QEMU's
/// `virt` machine: its device tree's `interrupt-map` routes pin `p` of slot `s` to SPI
/// `3 + (s + p - 1) mod 4`, level-high, and SPI `n` is GIC interrupt `32 + n`.
fn qemu_virt_intx(slot: u8, pin: u8) -> u32 {
    32 + 3 + ((slot as u32 + pin as u32 - 1) % 4)
}

/// Route the device's interrupts: INTx on QEMU's `virt` machine, MSI-X through the GICv2m
/// doorbell on Parallels. The MSI-X table entries live in the function's BAR and survive a
/// device reset; the virtio vector registers do not, so [`Device::enable_queue`] writes them.
fn route(pci_dev: &pci::Device) -> Result<Irq, &'static str> {
    if crate::platform_config::is_qemu() {
        let pin = pci::pci_read_config_byte(pci_dev.bus, pci_dev.device, pci_dev.function, 0x3D);
        if pin == 0 || pin > 4 {
            return Err("the virtio-gpu PCI device has no INTx pin");
        }
        // Command register bit 10 masks INTx; the transport needs it clear.
        let command = pci::pci_read_config_word(pci_dev.bus, pci_dev.device, pci_dev.function, 0x04);
        pci::pci_write_config_word(pci_dev.bus, pci_dev.device, pci_dev.function, 0x04, command & !(1 << 10));
        return Ok(Irq::Intx(qemu_virt_intx(pci_dev.device, pin)));
    }
    if !crate::platform_config::is_parallels() {
        return Err("the GPU core drives virtio-gpu on QEMU and Parallels only");
    }
    let cap = pci_dev.find_msix_capability().ok_or("the virtio-gpu PCI device has no MSI-X capability")?;
    if pci_dev.msix_table_size(cap) <= CONFIG_VECTOR {
        return Err("the virtio-gpu MSI-X table has fewer than two vectors");
    }
    let doorbell = crate::platform_config::gicv2m_doorbell().ok_or("no GICv2m frame for virtio-gpu MSI-X")?;
    let queue = crate::platform_config::allocate_msi_spi();
    let config = crate::platform_config::allocate_msi_spi();
    if queue == 0 || config == 0 {
        return Err("no GICv2m SPI left for virtio-gpu MSI-X");
    }
    pci_dev.configure_msix_entry(cap, QUEUE_VECTOR, doorbell, queue);
    pci_dev.configure_msix_entry(cap, CONFIG_VECTOR, doorbell, config);
    pci_dev.enable_msix(cap);
    pci_dev.disable_intx();
    Ok(Irq::Msix { queue, config })
}

impl Device {
    /// Find, reset and initialise the device; read its display information and
    /// capability sets with polled commands. Interrupts are not enabled yet.
    pub fn probe() -> Result<Device, &'static str> {
        let pci_dev = pci::find_device(pci::VIRTIO_VENDOR_ID, GPU_PCI_ID).ok_or("no virtio-gpu PCI device")?;
        let irq = route(&pci_dev)?;
        let mut pci = VirtioPciDevice::probe(pci_dev).ok_or("virtio-gpu PCI capabilities missing")?;
        let isr = pci.isr_address().ok_or("virtio-gpu PCI ISR capability missing")?;
        let size = Self::start(&mut pci)?;
        let queue = Queue::new(size).ok_or("no memory for the control queue")?;
        let mut dev = Device {
            pci,
            queue,
            inflight: Vec::new(),
            info: Info { features: 0, capset_ids: Vec::new(), capset: None, scanouts: Vec::new(), num_scanouts: 0 },
            irq,
            isr,
        };
        dev.inflight.resize_with(dev.queue.size() as usize, || None);
        let up = dev.enable_queue();
        dev.info.features = dev.pci.device_features() & WANTED;
        if let Err(error) = up.and_then(|()| dev.read_info()) {
            // DRIVER_OK made the ring and request buffers DMA-visible. Only a
            // confirmed status-zero reset ends that ownership (VirtIO 2.4.2).
            dev.begin_reset();
            let start = crate::signal::monotonic_nanos();
            while !dev.reset_done() && crate::signal::monotonic_nanos().saturating_sub(start) < 100_000_000 {
                core::hint::spin_loop();
            }
            if !dev.reset_done() {
                core::mem::forget(dev);
            }
            return Err(error);
        }
        Ok(dev)
    }

    /// Reset and negotiate (section 3.1.1); returns the control queue's size.
    fn start(pci: &mut VirtioPciDevice) -> Result<u16, &'static str> {
        pci.init(WANTED)?;
        pci.select_queue(0);
        let max = pci.get_queue_num_max();
        if max == 0 {
            return Err("virtio-gpu control queue unavailable");
        }
        let mut size = 1u16;
        while (size as u32) * 2 <= max && size < queue::MAX_SIZE {
            size *= 2;
        }
        Ok(size)
    }

    /// Set up the control queue and, with MSI-X, its vectors (section 4.1.4.3: a vector
    /// the device cannot use reads back as NO_VECTOR), then set DRIVER_OK.
    fn enable_queue(&mut self) -> Result<(), &'static str> {
        let msix = matches!(self.irq, Irq::Msix { .. });
        if msix && self.pci.set_config_msix_vector(CONFIG_VECTOR) != CONFIG_VECTOR {
            return Err("virtio-gpu refused its configuration MSI-X vector");
        }
        self.pci.select_queue(0);
        self.pci.set_queue_num(self.queue.size() as u32);
        self.pci.set_queue_desc(self.queue.desc_addr());
        self.pci.set_queue_avail(self.queue.avail_addr());
        self.pci.set_queue_used(self.queue.used_addr());
        if msix && self.pci.set_queue_msix_vector(QUEUE_VECTOR) != QUEUE_VECTOR {
            return Err("virtio-gpu refused its control queue MSI-X vector");
        }
        self.pci.set_queue_ready(true);
        self.pci.cache_queue_notify_addr(0);
        self.pci.driver_ok();
        Ok(())
    }

    /// Display information and capability sets, polled.
    fn read_info(&mut self) -> Result<(), &'static str> {
        self.info.num_scanouts = self.pci.read_config_u32(wire::CFG_NUM_SCANOUTS);
        let num_capsets = self.pci.read_config_u32(wire::CFG_NUM_CAPSETS);
        let display = Request::new(Tag::Background, cmd::GET_DISPLAY_INFO, 0, None, wire::HDR, wire::HDR + wire::MAX_SCANOUTS * wire::DISPLAY_ONE);
        let d = self.exec_polled(display)?;
        if d.answer != resp::OK_DISPLAY_INFO {
            return Err("GET_DISPLAY_INFO refused");
        }
        self.info.scanouts = (0..wire::MAX_SCANOUTS)
            .map(|i| {
                let at = wire::HDR + i * wire::DISPLAY_ONE;
                Scanout { width: d.resp.u32_at(at + 8), height: d.resp.u32_at(at + 12), enabled: d.resp.u32_at(at + 16) != 0 }
            })
            .collect();
        let mut sets: Vec<(u32, u32, u32)> = Vec::new();
        for index in 0..num_capsets.min(16) {
            let q = Request::new(Tag::Background, cmd::GET_CAPSET_INFO, 0, None, wire::HDR + 8, wire::HDR + 16).field(0, index);
            let d = self.exec_polled(q)?;
            if d.answer != resp::OK_CAPSET_INFO {
                return Err("GET_CAPSET_INFO refused");
            }
            let (id, ver, size) = (d.resp.u32_at(wire::HDR), d.resp.u32_at(wire::HDR + 4), d.resp.u32_at(wire::HDR + 8));
            // A device may list one set at several indices: keep each id once.
            if !sets.iter().any(|s| s.0 == id) {
                sets.push((id, ver, size));
            }
        }
        sets.sort_by_key(|s| s.0);
        self.info.capset_ids = sets.iter().map(|s| s.0).collect();
        let chosen = sets
            .iter()
            .find(|s| s.0 == wire::CAPSET_VIRGL2)
            .or_else(|| sets.iter().find(|s| s.0 == wire::CAPSET_VIRGL))
            .copied();
        if let Some((id, ver, size)) = chosen {
            let size = (size as usize).min(crate::gpu::dma::PAGE - wire::HDR);
            let q = Request::new(Tag::Background, cmd::GET_CAPSET, 0, None, wire::HDR + 8, wire::HDR + size).field(0, id).field(4, ver);
            let d = self.exec_polled(q)?;
            if d.answer != resp::OK_CAPSET {
                return Err("GET_CAPSET refused");
            }
            let bytes = d.resp.as_ref()[wire::HDR..].to_vec();
            self.info.capset = Some((id, ver, bytes));
        }
        Ok(())
    }

    /// Send one request and spin until the device returns it (initialisation only).
    fn exec_polled(&mut self, req: Request) -> Result<Done, &'static str> {
        self.send(req).map_err(|_| "control queue full")?;
        self.pci.notify_queue_fast(0);
        for _ in 0..50_000_000u64 {
            match self.reap() {
                Reap::Done(d) => return Ok(d),
                Reap::Fault => return Err("virtio-gpu returned a buffer it was not given"),
                Reap::Empty => core::hint::spin_loop(),
            }
        }
        Err("virtio-gpu did not answer")
    }

    /// Make `req` available to the device, or give it back when the queue has no room.
    /// The caller notifies the device (section 4.1.4.4), through [`Device::doorbell`].
    pub fn send(&mut self, req: Request) -> Result<(), Request> {
        let segs = req.segments();
        if segs.len() > self.queue.free_count() {
            return Err(req);
        }
        req.publish();
        let Some(head) = self.queue.add(&segs) else { return Err(req) };
        self.inflight[head as usize] = Some(req);
        Ok(())
    }

    /// The control queue's notification address: a 16-bit write of the queue index there
    /// tells the device buffers are available (VirtIO 1.3 section 4.1.4.4).
    pub fn doorbell(&self) -> u64 {
        self.pci.cached_notify_address(0).unwrap_or(0)
    }

    /// The control queue's size in descriptors.
    pub fn queue_size(&self) -> usize {
        self.queue.normal_capacity() as usize
    }

    pub(crate) fn terminal_queue(&self) -> emergency::Queue {
        self.queue.terminal(self.doorbell())
    }

    /// Whether a request of `segments` descriptors fits now.
    pub fn has_room(&self, segments: usize) -> bool {
        segments <= self.queue.free_count()
    }

    /// The next request the device returned. A faulty entry leaves the request it names,
    /// if any, in the device's hands, so only a confirmed reset releases its buffers.
    pub fn reap(&mut self) -> Reap {
        let (head, len) = match self.queue.pop_used() {
            Ok(Some(entry)) => entry,
            Ok(None) => return Reap::Empty,
            Err(()) => return Reap::Fault,
        };
        let Some(req) = self.inflight.get_mut(head as usize).and_then(Option::take) else {
            return Reap::Fault;
        };
        if len as usize > req.resp.len() {
            self.inflight[head as usize] = Some(req);
            return Reap::Fault;
        }
        req.resp.observe();
        let answer = req.resp.u32_at(wire::HDR_TYPE);
        let echoed = req.resp.u64_at(wire::HDR_FENCE);
        let Request { tag, fence, resp, .. } = req;
        Reap::Done(Done { tag, fence, answer, echoed, resp })
    }

    /// Whether the device has set DEVICE_NEEDS_RESET (section 2.1): a device fault.
    pub fn needs_reset(&self) -> bool {
        self.pci.read_status() & wire::STATUS_NEEDS_RESET != 0
    }

    /// Requests the device holds.
    pub fn outstanding(&self) -> usize {
        self.inflight.iter().filter(|r| r.is_some()).count()
    }

    /// Start a reset (section 2.4): write 0 to the device status.
    pub fn begin_reset(&mut self) {
        let _access = crate::gpu::panic::Access::enter();
        self.pci.reset();
    }

    /// Whether the device has completed the reset: it reads its status back as 0, which
    /// ends its use of every buffer it was given (section 2.4.2).
    pub fn reset_done(&self) -> bool {
        let _access = crate::gpu::panic::Access::enter();
        self.pci.read_status() == 0
    }

    /// After a confirmed reset: give back every request the device held, whose buffers
    /// it can no longer use, and bring the device up again with an empty queue.
    pub fn restart(&mut self) -> (Vec<Request>, Result<(), &'static str>) {
        let held: Vec<Request> = self.inflight.iter_mut().filter_map(Option::take).collect();
        let _access = crate::gpu::panic::Access::enter();
        self.queue.reset();
        let up = match Self::start(&mut self.pci) {
            Ok(size) if size >= self.queue.size() => self.enable_queue(),
            Ok(_) => Err("virtio-gpu control queue shrank across the reset"),
            Err(e) => Err(e),
        };
        (held, up)
    }
}

/// The interrupt handler's view: read the ISR byte at `isr` (acknowledging the level
/// interrupt) and return it.
#[inline]
pub fn read_isr(isr: u64) -> u8 {
    // SAFETY: the ISR capability's mapped byte, valid for the life of the boot.
    unsafe { core::ptr::read_volatile(isr as *const u8) }
}

// Command builders. Every request starts with the control header (section 5.7.6.7); `ctx`
// is the header's context, 0 for none. Each builder names where its request data comes
// from: a section of the specification, or, for the four layouts section 5.7.6.9 names
// without defining, the virtio-gpu interface header (`docs/gui/gpu.md` section 1.9).

/// CTX_CREATE (section 5.7.6.9): nlen, context_init (0: the device chooses the context
/// type, here VirGL), debug_name[64].
pub fn ctx_create(tag: Tag, ctx: u32) -> Request {
    let name = b"breenix";
    let mut r = Request::new(tag, cmd::CTX_CREATE, ctx, None, wire::HDR + 72, wire::HDR).field(0, name.len() as u32);
    r.hdr.as_mut()[wire::HDR + 8..wire::HDR + 8 + name.len()].copy_from_slice(name);
    r
}

/// CTX_DESTROY (section 5.7.6.9): the header alone, whose ctx_id names the context.
pub fn ctx_destroy(tag: Tag, ctx: u32) -> Request {
    Request::new(tag, cmd::CTX_DESTROY, ctx, None, wire::HDR, wire::HDR)
}

/// CTX_ATTACH_RESOURCE: resource_id, padding (the interface header's
/// `virtio_gpu_ctx_resource`).
pub fn ctx_attach(tag: Tag, ctx: u32, resource: u32) -> Request {
    Request::new(tag, cmd::CTX_ATTACH_RESOURCE, ctx, None, wire::HDR + 8, wire::HDR).field(0, resource)
}

/// RESOURCE_CREATE_3D: resource_id, target, format, bind, width, height, depth,
/// array_size, last_level, nr_samples, flags, padding (the interface header's
/// `virtio_gpu_resource_create_3d`): one single-sample level of a 2D texture or buffer.
pub fn resource_create(tag: Tag, id: u32, target: u32, format: u32, bind: u32, width: u32, height: u32) -> Request {
    Request::new(tag, cmd::RESOURCE_CREATE_3D, 0, None, wire::HDR + 48, wire::HDR)
        .field(0, id)
        .field(4, target)
        .field(8, format)
        .field(12, bind)
        .field(16, width)
        .field(20, height)
        .field(24, 1)
        .field(28, 1)
        .field(32, 0)
        .field(36, 0)
        .field(40, 0)
}

/// RESOURCE_UNREF (section 5.7.6.8): resource_id, padding. The request keeps `backing`,
/// which the device may use until it answers.
pub fn resource_unref(tag: Tag, id: u32, backing: Option<Arc<Pages>>) -> Request {
    let mut r = Request::new(tag, cmd::RESOURCE_UNREF, 0, None, wire::HDR + 8, wire::HDR).field(0, id);
    r.keep = backing;
    r
}

/// RESOURCE_ATTACH_BACKING (section 5.7.6.8): resource_id, nr_entries, then one
/// `virtio_gpu_mem_entry` (addr, length, padding) per run of adjacent frames of `backing`.
pub fn attach_backing(tag: Tag, id: u32, backing: Arc<Pages>) -> Option<Request> {
    let runs = backing.runs();
    let bytes = runs.len() * 16;
    let entries = Pages::new(bytes.div_ceil(crate::gpu::dma::PAGE))?;
    for (i, (addr, len)) in runs.iter().enumerate() {
        let mut e = [0u8; 16];
        e[..8].copy_from_slice(&addr.to_le_bytes());
        e[8..12].copy_from_slice(&len.to_le_bytes());
        entries.write(i * 16, &e);
    }
    let mut r = Request::new(tag, cmd::RESOURCE_ATTACH_BACKING, 0, None, wire::HDR + 8, wire::HDR)
        .field(0, id)
        .field(4, runs.len() as u32);
    r.payload = Payload::Pages(entries, bytes);
    r.keep = Some(backing);
    Some(r)
}

/// TRANSFER_FROM_HOST_3D of a box into the attached `backing` at `offset`, with `stride`
/// bytes per row; fenced. Layout: box (x, y, z, w, h, d), offset (u64), resource_id,
/// level, stride, layer_stride (the interface header's `virtio_gpu_transfer_host_3d`).
#[allow(clippy::too_many_arguments)]
pub fn transfer_from_host(
    tag: Tag,
    ctx: u32,
    fence: u64,
    id: u32,
    backing: Arc<Pages>,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    offset: u64,
    stride: u32,
) -> Request {
    let mut r = Request::new(tag, cmd::TRANSFER_FROM_HOST_3D, ctx, Some(fence), wire::HDR + 48, wire::HDR)
        .field(0, x)
        .field(4, y)
        .field(8, 0)
        .field(12, w)
        .field(16, h)
        .field(20, 1)
        .field64(24, offset)
        .field(32, id)
        .field(36, 0)
        .field(40, stride)
        .field(44, 0);
    r.keep = Some(backing);
    r
}

/// TRANSFER_TO_HOST_3D of a box from the attached `backing` at `offset`, with `stride` bytes
/// per row; fenced. The same layout as [`transfer_from_host`] (the interface header's
/// `virtio_gpu_transfer_host_3d`, used by both directions).
#[allow(clippy::too_many_arguments)]
pub fn transfer_to_host(
    tag: Tag,
    ctx: u32,
    fence: u64,
    id: u32,
    backing: Arc<Pages>,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    offset: u64,
    stride: u32,
) -> Request {
    let mut r = transfer_from_host(tag, ctx, fence, id, backing, x, y, w, h, offset, stride);
    r.hdr.put_u32(wire::HDR_TYPE, cmd::TRANSFER_TO_HOST_3D);
    r
}

/// A fenced command of a type section 5.7.6.7 does not define (the last of the 3D range's
/// unassigned numbers): test point 27's way to make the device answer with an error.
pub fn undefined_command(tag: Tag, ctx: u32, fence: u64) -> Request {
    Request::new(tag, 0x02FF, ctx, Some(fence), wire::HDR, wire::HDR)
}

/// SUBMIT_3D of a VirGL command buffer: size in bytes, padding, then the buffer (the
/// interface header's `virtio_gpu_cmd_submit`).
pub fn submit(tag: Tag, ctx: u32, fence: Option<u64>, words: WordBuf) -> Request {
    let size = (words.len() * 4) as u32;
    let mut r = Request::new(tag, cmd::SUBMIT_3D, ctx, fence, wire::HDR + 8, wire::HDR).field(0, size);
    r.payload = Payload::Words(words);
    r
}

/// Whether `answer` is a success.
pub fn ok(answer: u32) -> bool {
    (resp::OK_NODATA..resp::ERR_UNSPEC).contains(&answer)
}

/// SET_SCANOUT (VirtIO 1.3 section 5.7.6.8): r (x, y, width, height), scanout_id,
/// resource_id: the whole resource on scanout 0, unfenced; the flush after it carries the
/// fence.
pub fn set_scanout(tag: Tag, ctx: u32, resource: u32, width: u32, height: u32) -> Request {
    Request::new(tag, cmd::SET_SCANOUT, ctx, None, wire::HDR + 24, wire::HDR)
        .field(0, 0).field(4, 0).field(8, width).field(12, height)
        .field(16, 0).field(20, resource)
}

/// RESOURCE_FLUSH (VirtIO 1.3 section 5.7.6.8): r (x, y, width, height), resource_id,
/// padding: the resource's whole rectangle, fenced.
pub fn flush_resource(tag: Tag, ctx: u32, fence: u64, resource: u32, width: u32, height: u32) -> Request {
    Request::new(tag, cmd::RESOURCE_FLUSH, ctx, Some(fence), wire::HDR + 24, wire::HDR)
        .field(0, 0).field(4, 0).field(8, width).field(12, height)
        .field(16, resource).field(20, 0)
}
