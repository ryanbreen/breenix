//! The VirGL backend's device: virtio-gpu over the modern PCI transport.
//!
//! Written from VirtIO 1.3 CSD01: device initialisation (section 3.1), the PCI transport
//! (section 4.1), split virtqueues (section 2.7) and the GPU device (section 5.7). The
//! control queue carries every command; a fenced command's response arrives only after
//! the device has finished it and echoes its fence (section 5.7.6.7). Completion is
//! signalled by the transport's interrupt; with MSI-X off the device uses its PCI INTx
//! pin and the ISR status byte, whose read acknowledges it (section 4.1.4.5).
//!
//! The device object lives under the GPU core's lock; nothing here blocks.

pub mod encode;
pub mod queue;
pub mod wire;

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
    /// The core keeps memory the command names until the device returns it.
    Retire(u64),
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
        Request { tag, fence, hdr, payload: Payload::None, resp: DmaBox::new(resp_len) }
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

pub struct Device {
    pci: VirtioPciDevice,
    queue: Queue,
    inflight: Vec<Option<Request>>,
    pub info: Info,
    /// The GIC interrupt id of the device's INTx line.
    pub intid: u32,
    /// The ISR status byte's address.
    pub isr: u64,
}

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

impl Device {
    /// Find, reset and initialise the device; read its display information and
    /// capability sets with polled commands. Interrupts are not enabled yet.
    pub fn probe() -> Result<Device, &'static str> {
        if !crate::platform_config::is_qemu() {
            return Err("the GPU core drives QEMU's virtio-gpu-gl-pci on this target");
        }
        let pci_dev = pci::find_device(pci::VIRTIO_VENDOR_ID, GPU_PCI_ID).ok_or("no virtio-gpu PCI device")?;
        let pin = pci::pci_read_config_byte(pci_dev.bus, pci_dev.device, pci_dev.function, 0x3D);
        if pin == 0 || pin > 4 {
            return Err("the virtio-gpu PCI device has no INTx pin");
        }
        // Command register bit 10 masks INTx; the transport needs it clear.
        let command = pci::pci_read_config_word(pci_dev.bus, pci_dev.device, pci_dev.function, 0x04);
        pci::pci_write_config_word(pci_dev.bus, pci_dev.device, pci_dev.function, 0x04, command & !(1 << 10));
        let intid = qemu_virt_intx(pci_dev.device, pin);
        let mut pci = VirtioPciDevice::probe(pci_dev).ok_or("virtio-gpu PCI capabilities missing")?;
        let isr = pci.isr_address().ok_or("virtio-gpu PCI ISR capability missing")?;
        let size = Self::start(&mut pci)?;
        let queue = Queue::new(size).ok_or("no memory for the control queue")?;
        let mut dev = Device {
            pci,
            queue,
            inflight: Vec::new(),
            info: Info { features: 0, capset_ids: Vec::new(), capset: None, scanouts: Vec::new(), num_scanouts: 0 },
            intid,
            isr,
        };
        dev.inflight.resize_with(dev.queue.size() as usize, || None);
        dev.enable_queue();
        dev.info.features = dev.pci.device_features() & WANTED;
        dev.read_info()?;
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

    fn enable_queue(&mut self) {
        self.pci.select_queue(0);
        self.pci.set_queue_num(self.queue.size() as u32);
        self.pci.set_queue_desc(self.queue.desc_addr());
        self.pci.set_queue_avail(self.queue.avail_addr());
        self.pci.set_queue_used(self.queue.used_addr());
        self.pci.set_queue_ready(true);
        self.pci.cache_queue_notify_addr(0);
        self.pci.driver_ok();
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
        for _ in 0..50_000_000u64 {
            if let Some(d) = self.reap() {
                return Ok(d);
            }
            core::hint::spin_loop();
        }
        Err("virtio-gpu did not answer")
    }

    /// Hand `req` to the device, or give it back when the queue has no room.
    pub fn send(&mut self, req: Request) -> Result<(), Request> {
        let segs = req.segments();
        if segs.len() > self.queue.free_count() {
            return Err(req);
        }
        req.publish();
        let Some(head) = self.queue.add(&segs) else { return Err(req) };
        self.inflight[head as usize] = Some(req);
        self.pci.notify_queue_fast(0);
        Ok(())
    }

    /// Whether a request of `segments` descriptors fits now.
    pub fn has_room(&self, segments: usize) -> bool {
        segments <= self.queue.free_count()
    }

    /// The next request the device returned.
    pub fn reap(&mut self) -> Option<Done> {
        loop {
            let (head, _len) = self.queue.pop_used()?;
            let Some(req) = self.inflight.get_mut(head as usize).and_then(Option::take) else {
                continue;
            };
            req.resp.observe();
            let answer = req.resp.u32_at(wire::HDR_TYPE);
            let echoed = req.resp.u64_at(wire::HDR_FENCE);
            let Request { tag, fence, resp, .. } = req;
            return Some(Done { tag, fence, answer, echoed, resp });
        }
    }

    /// Requests the device holds.
    pub fn outstanding(&self) -> usize {
        self.inflight.iter().filter(|r| r.is_some()).count()
    }

    /// Reset the device (section 2.4), which ends its use of every request, and bring it
    /// back. Returns the tags of the requests it held, which the caller fails.
    pub fn reset(&mut self) -> Result<Vec<Tag>, &'static str> {
        self.pci.reset();
        let mut cleared = false;
        for _ in 0..10_000_000u64 {
            if self.pci.read_status() == 0 {
                cleared = true;
                break;
            }
            core::hint::spin_loop();
        }
        let tags: Vec<Tag> = self.inflight.iter_mut().filter_map(Option::take).map(|r| r.tag).collect();
        if !cleared {
            return Err("virtio-gpu reset did not complete");
        }
        self.queue.reset();
        Self::start(&mut self.pci)?;
        self.enable_queue();
        Ok(tags)
    }

}

/// The interrupt handler's view: read the ISR byte at `isr` (acknowledging the level
/// interrupt) and return it.
#[inline]
pub fn read_isr(isr: u64) -> u8 {
    // SAFETY: the ISR capability's mapped byte, valid for the life of the boot.
    unsafe { core::ptr::read_volatile(isr as *const u8) }
}

// Command builders (section 5.7.6.7's layouts). `ctx` is the device context the command
// runs in, 0 for none.

pub fn ctx_create(tag: Tag, ctx: u32) -> Request {
    // virtio_gpu_ctx_create: nlen, context_init (0: the device's default VirGL), name[64].
    let name = b"breenix";
    let mut r = Request::new(tag, cmd::CTX_CREATE, ctx, None, wire::HDR + 72, wire::HDR).field(0, name.len() as u32);
    r.hdr.as_mut()[wire::HDR + 8..wire::HDR + 8 + name.len()].copy_from_slice(name);
    r
}

pub fn ctx_destroy(tag: Tag, ctx: u32) -> Request {
    Request::new(tag, cmd::CTX_DESTROY, ctx, None, wire::HDR, wire::HDR)
}

pub fn ctx_attach(tag: Tag, ctx: u32, resource: u32) -> Request {
    Request::new(tag, cmd::CTX_ATTACH_RESOURCE, ctx, None, wire::HDR + 8, wire::HDR).field(0, resource)
}

/// RESOURCE_CREATE_3D: id, target, format, bind, width, height, depth, array size, last
/// level, samples, flags.
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

pub fn resource_unref(tag: Tag, id: u32) -> Request {
    Request::new(tag, cmd::RESOURCE_UNREF, 0, None, wire::HDR + 8, wire::HDR).field(0, id)
}

/// RESOURCE_ATTACH_BACKING with one entry per run of adjacent frames of `backing`.
pub fn attach_backing(tag: Tag, id: u32, backing: &Pages) -> Option<Request> {
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
    Some(r)
}

/// TRANSFER_FROM_HOST_3D of a box into the attached backing at `offset`, with `stride`
/// bytes per row; fenced.
#[allow(clippy::too_many_arguments)]
pub fn transfer_from_host(tag: Tag, ctx: u32, fence: u64, id: u32, x: u32, y: u32, w: u32, h: u32, offset: u64, stride: u32) -> Request {
    Request::new(tag, cmd::TRANSFER_FROM_HOST_3D, ctx, Some(fence), wire::HDR + 48, wire::HDR)
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
        .field(44, 0)
}

/// A fenced command of a type section 5.7.6.7 does not define (the last of the 3D range's
/// unassigned numbers): test point 27's way to make the device answer with an error.
pub fn undefined_command(tag: Tag, ctx: u32, fence: u64) -> Request {
    Request::new(tag, 0x02FF, ctx, Some(fence), wire::HDR, wire::HDR)
}

/// SUBMIT_3D of a VirGL command buffer.
pub fn submit(tag: Tag, ctx: u32, fence: Option<u64>, words: WordBuf) -> Request {
    let size = (words.len() * 4) as u32;
    let mut r = Request::new(tag, cmd::SUBMIT_3D, ctx, fence, wire::HDR + 8, wire::HDR).field(0, size);
    r.payload = Payload::Words(words);
    r
}

/// How many descriptors a SUBMIT_3D of `words` uses.
pub fn submit_segments(words: &WordBuf) -> usize {
    2 + (words.len() * 4).div_ceil(crate::gpu::dma::PAGE)
}

/// Whether `answer` is a success.
pub fn ok(answer: u32) -> bool {
    (resp::OK_NODATA..resp::ERR_UNSPEC).contains(&answer)
}
