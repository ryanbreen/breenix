//! A fake virtio-mmio network device, from virtio 1.2: the register layouts of
//! sections 4.2.2 and 4.2.4 (legacy), the split rings of section 2.7 with the
//! legacy layout of 2.7.2, and the network device of section 5.1. It answers
//! the driver's register accesses, finds its rings from the registers the
//! driver wrote, and reads and writes those rings in host memory.
//!
//! Transmissions are taken when the driver notifies a transmit queue, and
//! completed at once unless `hold_tx` is set. A control-queue command is
//! answered with `ctrl_ack` when the driver notifies the control queue.
//! Receptions are injected by the test with `deliver`.
use std::sync::Mutex;

pub const BASE: u64 = 0x0a00_0000;
pub const SLOT_SIZE: u64 = 0x200;
pub const SLOTS: u64 = 32;
/// The fake's interrupt: QEMU virt's first virtio-mmio slot.
pub const IRQ: u8 = 48;

/// Feature bits (virtio 1.2 sections 5.1.3 and 6).
pub mod f {
    pub const CSUM: u64 = 1 << 0;
    pub const GUEST_CSUM: u64 = 1 << 1;
    pub const MTU: u64 = 1 << 3;
    pub const MAC: u64 = 1 << 5;
    pub const GUEST_TSO4: u64 = 1 << 7;
    pub const HOST_TSO4: u64 = 1 << 11;
    pub const MRG_RXBUF: u64 = 1 << 15;
    pub const STATUS: u64 = 1 << 16;
    pub const CTRL_VQ: u64 = 1 << 17;
    pub const CTRL_RX: u64 = 1 << 18;
    pub const MQ: u64 = 1 << 22;
    pub const NOTIFY_ON_EMPTY: u64 = 1 << 24;
    pub const ANY_LAYOUT: u64 = 1 << 27;
    pub const INDIRECT_DESC: u64 = 1 << 28;
    pub const EVENT_IDX: u64 = 1 << 29;
    pub const VERSION_1: u64 = 1 << 32;
}

pub mod status {
    pub const ACKNOWLEDGE: u32 = 1;
    pub const DRIVER: u32 = 2;
    pub const DRIVER_OK: u32 = 4;
    pub const FEATURES_OK: u32 = 8;
    pub const FAILED: u32 = 128;
}

/// Register offsets (sections 4.2.2 and 4.2.4).
pub mod reg {
    pub const MAGIC: u64 = 0x000;
    pub const VERSION: u64 = 0x004;
    pub const DEVICE_ID: u64 = 0x008;
    pub const VENDOR_ID: u64 = 0x00c;
    pub const DEVICE_FEATURES: u64 = 0x010;
    pub const DEVICE_FEATURES_SEL: u64 = 0x014;
    pub const DRIVER_FEATURES: u64 = 0x020;
    pub const DRIVER_FEATURES_SEL: u64 = 0x024;
    pub const GUEST_PAGE_SIZE: u64 = 0x028;
    pub const QUEUE_SEL: u64 = 0x030;
    pub const QUEUE_NUM_MAX: u64 = 0x034;
    pub const QUEUE_NUM: u64 = 0x038;
    pub const QUEUE_ALIGN: u64 = 0x03c;
    pub const QUEUE_PFN: u64 = 0x040;
    pub const QUEUE_READY: u64 = 0x044;
    pub const QUEUE_NOTIFY: u64 = 0x050;
    pub const INTERRUPT_STATUS: u64 = 0x060;
    pub const INTERRUPT_ACK: u64 = 0x064;
    pub const STATUS: u64 = 0x070;
    pub const QUEUE_DESC_LOW: u64 = 0x080;
    pub const QUEUE_DESC_HIGH: u64 = 0x084;
    pub const QUEUE_AVAIL_LOW: u64 = 0x090;
    pub const QUEUE_AVAIL_HIGH: u64 = 0x094;
    pub const QUEUE_USED_LOW: u64 = 0x0a0;
    pub const QUEUE_USED_HIGH: u64 = 0x0a4;
    pub const CONFIG_GENERATION: u64 = 0x0fc;
    pub const CONFIG: u64 = 0x100;
}

const DESC_F_NEXT: u16 = 1;
const DESC_F_WRITE: u16 = 2;
const AVAIL_F_NO_INTERRUPT: u16 = 1;

/// What the device is, before the driver touches it.
#[derive(Clone)]
pub struct Config {
    /// 1 for the legacy register layout, 2 for the modern one.
    pub version: u32,
    pub features: u64,
    /// QueueNumMax of each receive and transmit queue.
    pub queue_max: u32,
    /// QueueNumMax of the control queue.
    pub ctrl_queue_max: u32,
    pub mac: [u8; 6],
    /// max_virtqueue_pairs, meaningful with MQ.
    pub max_pairs: u16,
    /// mtu, meaningful with MTU.
    pub mtu: u16,
    /// The ack byte written for each control command.
    pub ctrl_ack: u8,
    /// CPUs firmware reports to the kernel.
    pub cpus: u64,
}

impl Config {
    pub fn legacy(features: u64) -> Self {
        Self {
            version: 1,
            features,
            queue_max: 256,
            ctrl_queue_max: 64,
            mac: [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
            max_pairs: 1,
            mtu: 1500,
            ctrl_ack: 0,
            cpus: 4,
        }
    }
}

/// One register write, with the device status when it was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Write {
    pub offset: u64,
    pub value: u32,
    /// QueueSel at the time of the write.
    pub queue: u32,
}

/// One buffer the driver offered on a transmit queue.
#[derive(Clone, Debug)]
pub struct TxPacket {
    pub queue: u32,
    pub head: u16,
    /// (address, length, flags) of each descriptor in the chain.
    pub descriptors: Vec<(u64, u32, u16)>,
    /// The device-readable bytes, concatenated: header then frame.
    pub bytes: Vec<u8>,
}

#[derive(Clone, Default, Debug)]
pub struct Queue {
    pub num_max: u32,
    pub num: u32,
    pub align: u32,
    pub pfn: u32,
    pub desc: u64,
    pub avail: u64,
    pub used: u64,
    pub ready: bool,
    /// The device's next available-ring position to read.
    pub last_avail: u16,
    /// The device's copy of used.idx.
    pub used_idx: u16,
    pub notifies: u32,
    pub interrupts: u32,
    /// Transmit heads taken but not yet returned (`hold_tx`).
    pub held: Vec<u16>,
}

pub struct Device {
    pub cfg: Config,
    pub status: u32,
    /// The status values the driver wrote, in order.
    pub status_writes: Vec<u32>,
    dev_sel: u32,
    drv_sel: u32,
    pub driver_features: u64,
    pub page_size: u32,
    pub queue_sel: u32,
    pub queues: Vec<Queue>,
    pub int_status: u32,
    pub log: Vec<Write>,
    pub tx: Vec<TxPacket>,
    pub ctrl: Vec<Vec<u8>>,
    pub hold_tx: bool,
    pub irq_enabled: Option<u8>,
    pub softirqs: u32,
    /// Ring errors the device found in the driver's structures.
    pub errors: Vec<String>,
    /// Frames the allocator may still hand out, when limited.
    pub frame_budget: Option<usize>,
    /// When set, the device asks not to be notified (used.flags, avail_event).
    pub suppress_notify: bool,
    /// Reads of the device-specific configuration: (offset into it, bytes wide).
    pub config_reads: Vec<(u64, u32)>,
}

static DEVICE: Mutex<Option<Device>> = Mutex::new(None);

/// Put a device with `cfg` in slot 0, leaving slots 1 to 31 empty.
pub fn install(cfg: Config) {
    let queues = vec![Queue::default(); 2 * 8 + 1];
    *DEVICE.lock().unwrap() = Some(Device {
        cfg,
        status: 0,
        status_writes: Vec::new(),
        dev_sel: 0,
        drv_sel: 0,
        driver_features: 0,
        page_size: 0,
        queue_sel: 0,
        queues,
        int_status: 0,
        log: Vec::new(),
        tx: Vec::new(),
        ctrl: Vec::new(),
        hold_tx: false,
        irq_enabled: None,
        softirqs: 0,
        errors: Vec::new(),
        frame_budget: None,
        suppress_notify: false,
        config_reads: Vec::new(),
    });
}

pub fn with<R>(f: impl FnOnce(&mut Device) -> R) -> R {
    let mut guard = DEVICE.lock().unwrap();
    f(guard.as_mut().expect("no device installed"))
}

pub fn reported_cpus() -> u64 {
    with(|d| d.cfg.cpus)
}

pub fn allocation_allowed(frames: usize) -> bool {
    with(|d| match d.frame_budget.as_mut() {
        None => true,
        Some(left) if *left >= frames => {
            *left -= frames;
            true
        }
        Some(_) => false,
    })
}

pub fn note_irq_enabled(irq: u8) {
    with(|d| d.irq_enabled = Some(irq));
}

pub fn note_softirq(_kind: crate::task::softirqd::SoftirqType) {
    with(|d| d.softirqs += 1);
}

fn split(address: u64) -> Option<u64> {
    assert!(
        (BASE..BASE + SLOTS * SLOT_SIZE).contains(&address),
        "register access outside the virtio-mmio window: {address:#x}"
    );
    let slot = (address - BASE) / SLOT_SIZE;
    (slot == 0).then_some((address - BASE) % SLOT_SIZE)
}

pub fn read8(address: u64) -> u8 {
    let Some(offset) = split(address) else { return 0 };
    with(|d| {
        d.note_config_read(offset, 1);
        d.config_byte(offset)
    })
}

pub fn read16(address: u64) -> u16 {
    let Some(offset) = split(address) else { return 0 };
    with(|d| {
        d.note_config_read(offset, 2);
        u16::from_le_bytes([d.config_byte(offset), d.config_byte(offset + 1)])
    })
}

pub fn read32(address: u64) -> u32 {
    let Some(offset) = split(address) else { return 0 };
    with(|d| {
        d.note_config_read(offset, 4);
        d.read(offset)
    })
}

pub fn write32(address: u64, value: u32) {
    let Some(offset) = split(address) else { return };
    with(|d| d.write(offset, value));
}

/// The 16-bit "need event" test of virtio 1.2 appendix A (virtq_need_event).
pub fn need_event(event: u16, new: u16, old: u16) -> bool {
    new.wrapping_sub(event).wrapping_sub(1) < new.wrapping_sub(old)
}

unsafe fn rd16(a: u64) -> u16 {
    core::ptr::read_volatile(a as *const u16)
}
unsafe fn wr16(a: u64, v: u16) {
    core::ptr::write_volatile(a as *mut u16, v)
}
unsafe fn rd32(a: u64) -> u32 {
    core::ptr::read_volatile(a as *const u32)
}
unsafe fn wr32(a: u64, v: u32) {
    core::ptr::write_volatile(a as *mut u32, v)
}
unsafe fn rd64(a: u64) -> u64 {
    core::ptr::read_volatile(a as *const u64)
}

impl Device {
    pub fn negotiated(&self, bit: u64) -> bool {
        self.driver_features & bit != 0
    }

    /// N of section 5.1.2: max_virtqueue_pairs with MQ negotiated, else 1.
    pub fn pairs(&self) -> u32 {
        if self.negotiated(f::MQ) {
            u32::from(self.cfg.max_pairs)
        } else {
            1
        }
    }

    pub fn ctrl_index(&self) -> Option<u32> {
        self.negotiated(f::CTRL_VQ).then(|| 2 * self.pairs())
    }

    fn queue_exists(&self, q: u32) -> u32 {
        let offered_pairs = if self.cfg.features & f::MQ != 0 {
            u32::from(self.cfg.max_pairs)
        } else {
            1
        };
        if q < 2 * offered_pairs {
            self.cfg.queue_max
        } else if self.cfg.features & f::CTRL_VQ != 0 && q == 2 * self.pairs() {
            self.cfg.ctrl_queue_max
        } else {
            0
        }
    }

    fn note_config_read(&mut self, offset: u64, width: u32) {
        if let Some(at) = offset.checked_sub(reg::CONFIG) {
            self.config_reads.push((at, width));
        }
    }

    /// The device-specific configuration (section 5.1.4): mac, status (link
    /// up), max_virtqueue_pairs and mtu, little-endian as the host is.
    fn config_byte(&self, offset: u64) -> u8 {
        let mut cfg = [0u8; 12];
        cfg[..6].copy_from_slice(&self.cfg.mac);
        cfg[6..8].copy_from_slice(&1u16.to_le_bytes());
        cfg[8..10].copy_from_slice(&self.cfg.max_pairs.to_le_bytes());
        cfg[10..12].copy_from_slice(&self.cfg.mtu.to_le_bytes());
        offset
            .checked_sub(reg::CONFIG)
            .and_then(|i| cfg.get(i as usize).copied())
            .unwrap_or(0)
    }

    fn read(&mut self, offset: u64) -> u32 {
        let q = self.queue_sel as usize;
        match offset {
            reg::MAGIC => 0x7472_6976,
            reg::VERSION => self.cfg.version,
            reg::DEVICE_ID => 1,
            reg::VENDOR_ID => 0x554d_4551,
            reg::DEVICE_FEATURES => match self.dev_sel {
                0 => self.cfg.features as u32,
                1 => (self.cfg.features >> 32) as u32,
                _ => 0,
            },
            reg::QUEUE_NUM_MAX => self.queue_exists(self.queue_sel),
            reg::QUEUE_PFN => self.queues[q].pfn,
            reg::QUEUE_READY => u32::from(self.queues[q].ready),
            reg::INTERRUPT_STATUS => self.int_status,
            reg::STATUS => self.status,
            reg::CONFIG_GENERATION => 0,
            o if o >= reg::CONFIG => u32::from_le_bytes(core::array::from_fn(|i| {
                self.config_byte(o + i as u64)
            })),
            _ => 0,
        }
    }

    fn write(&mut self, offset: u64, value: u32) {
        self.log.push(Write { offset, value, queue: self.queue_sel });
        let q = self.queue_sel as usize;
        match offset {
            reg::DEVICE_FEATURES_SEL => self.dev_sel = value,
            reg::DRIVER_FEATURES_SEL => self.drv_sel = value,
            reg::DRIVER_FEATURES => match self.drv_sel {
                0 => self.driver_features = (self.driver_features & !0xffff_ffff) | u64::from(value),
                1 => {
                    self.driver_features =
                        (self.driver_features & 0xffff_ffff) | (u64::from(value) << 32)
                }
                _ => {}
            },
            reg::GUEST_PAGE_SIZE => self.page_size = value,
            reg::QUEUE_SEL => self.queue_sel = value,
            reg::QUEUE_NUM => self.queues[q].num = value,
            reg::QUEUE_ALIGN => self.queues[q].align = value,
            reg::QUEUE_PFN => {
                let queue = &mut self.queues[q];
                queue.pfn = value;
                if value != 0 {
                    let num = u64::from(queue.num);
                    let align = u64::from(queue.align.max(1));
                    queue.desc = u64::from(value) * u64::from(self.page_size);
                    queue.avail = queue.desc + 16 * num;
                    queue.used = (queue.avail + 2 * (3 + num)).next_multiple_of(align);
                }
            }
            reg::QUEUE_READY => self.queues[q].ready = value != 0,
            reg::QUEUE_DESC_LOW => set_low(&mut self.queues[q].desc, value),
            reg::QUEUE_DESC_HIGH => set_high(&mut self.queues[q].desc, value),
            reg::QUEUE_AVAIL_LOW => set_low(&mut self.queues[q].avail, value),
            reg::QUEUE_AVAIL_HIGH => set_high(&mut self.queues[q].avail, value),
            reg::QUEUE_USED_LOW => set_low(&mut self.queues[q].used, value),
            reg::QUEUE_USED_HIGH => set_high(&mut self.queues[q].used, value),
            reg::QUEUE_NOTIFY => {
                if let Some(queue) = self.queues.get_mut(value as usize) {
                    queue.notifies += 1;
                }
                self.process(value);
            }
            reg::INTERRUPT_ACK => self.int_status &= !value,
            reg::STATUS => {
                self.status_writes.push(value);
                if value == 0 {
                    self.reset();
                    return;
                }
                let mut value = value;
                if self.cfg.version == 2 && value & status::FEATURES_OK != 0 {
                    let accepted = self.driver_features & !self.cfg.features == 0
                        && self.negotiated(f::VERSION_1);
                    if !accepted {
                        value &= !status::FEATURES_OK;
                    }
                }
                self.status = value;
            }
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.status = 0;
        self.driver_features = 0;
        self.int_status = 0;
        for queue in &mut self.queues {
            *queue = Queue::default();
        }
    }

    /// Is the queue live, with rings the device can find?
    fn live(&self, q: u32) -> bool {
        let queue = &self.queues[q as usize];
        queue.num != 0 && (if self.cfg.version == 1 { queue.pfn != 0 } else { queue.ready })
    }

    fn avail_idx(&self, q: u32) -> u16 {
        unsafe { rd16(self.queues[q as usize].avail + 2) }
    }

    /// Take the next head the driver offered, if any.
    pub fn take_avail(&mut self, q: u32) -> Option<u16> {
        if !self.live(q) || self.queues[q as usize].last_avail == self.avail_idx(q) {
            return None;
        }
        let queue = &mut self.queues[q as usize];
        let pos = u64::from(queue.last_avail % queue.num as u16);
        let head = unsafe { rd16(queue.avail + 4 + 2 * pos) };
        queue.last_avail = queue.last_avail.wrapping_add(1);
        Some(head)
    }

    /// Heads the driver has offered and the device has not taken.
    pub fn avail_pending(&self, q: u32) -> u16 {
        self.avail_idx(q).wrapping_sub(self.queues[q as usize].last_avail)
    }

    /// The descriptor chain at `head`: (address, length, flags).
    pub fn chain(&mut self, q: u32, head: u16) -> Vec<(u64, u32, u16)> {
        let queue = &self.queues[q as usize];
        let mut out = Vec::new();
        let mut index = head;
        loop {
            if u32::from(index) >= queue.num || out.len() as u32 > queue.num {
                self.errors.push(format!("queue {q}: chain from {head} reaches {index}"));
                return out;
            }
            let d = queue.desc + 16 * u64::from(index);
            let (addr, len, flags, next) =
                unsafe { (rd64(d), rd32(d + 8), rd16(d + 12), rd16(d + 14)) };
            out.push((addr, len, flags));
            if flags & DESC_F_NEXT == 0 {
                return out;
            }
            index = next;
        }
    }

    /// Write used entries for `heads` and publish them with one used.idx store,
    /// then interrupt as section 2.7.7.2 says.
    pub fn push_used(&mut self, q: u32, entries: &[(u32, u32)]) {
        if entries.is_empty() {
            return;
        }
        let event_idx = self.negotiated(f::EVENT_IDX);
        let queue = &mut self.queues[q as usize];
        let num = queue.num;
        let old = queue.used_idx;
        for &(id, len) in entries {
            let pos = u64::from(queue.used_idx % num as u16);
            unsafe {
                wr32(queue.used + 4 + 8 * pos, id);
                wr32(queue.used + 8 + 8 * pos, len);
            }
            queue.used_idx = queue.used_idx.wrapping_add(1);
        }
        let new = queue.used_idx;
        unsafe { wr16(queue.used + 2, new) };
        let interrupt = if event_idx {
            let used_event = unsafe { rd16(queue.avail + 4 + 2 * u64::from(num)) };
            need_event(used_event, new, old)
        } else {
            let flags = unsafe { rd16(queue.avail) };
            flags & AVAIL_F_NO_INTERRUPT == 0
        };
        if interrupt {
            queue.interrupts += 1;
            self.int_status |= 1;
        }
    }

    /// The device's view of notification suppression: ask for a notification
    /// on the next buffer, or ask not to be notified (section 2.7.10.2).
    fn publish_avail_event(&mut self, q: u32) {
        if !self.live(q) {
            return;
        }
        let suppress = self.suppress_notify;
        let event_idx = self.negotiated(f::EVENT_IDX);
        let queue = &self.queues[q as usize];
        unsafe {
            if event_idx {
                let event = if suppress {
                    queue.last_avail.wrapping_sub(1)
                } else {
                    queue.last_avail
                };
                wr16(queue.used + 4 + 8 * u64::from(queue.num), event);
            } else {
                wr16(queue.used, u16::from(suppress));
            }
        }
    }

    pub fn tx_queue(&self, q: u32) -> bool {
        q < 2 * self.pairs() && q % 2 == 1
    }

    fn process(&mut self, q: u32) {
        if Some(q) == self.ctrl_index() {
            self.process_ctrl(q);
        } else if self.tx_queue(q) {
            self.process_tx(q);
        }
        self.publish_avail_event(q);
    }

    pub fn process_tx(&mut self, q: u32) {
        let mut done = Vec::new();
        while let Some(head) = self.take_avail(q) {
            let descriptors = self.chain(q, head);
            let mut bytes = Vec::new();
            for &(addr, len, flags) in &descriptors {
                if flags & DESC_F_WRITE != 0 {
                    self.errors.push(format!("queue {q}: writable descriptor in a transmission"));
                    continue;
                }
                bytes.extend_from_slice(unsafe {
                    core::slice::from_raw_parts(addr as *const u8, len as usize)
                });
            }
            self.tx.push(TxPacket { queue: q, head, descriptors, bytes });
            if self.hold_tx {
                self.queues[q as usize].held.push(head);
            } else {
                done.push((u32::from(head), 0));
            }
        }
        self.push_used(q, &done);
    }

    /// Return up to `n` held transmissions, oldest first.
    pub fn complete_tx(&mut self, q: u32, n: usize) {
        let held = &mut self.queues[q as usize].held;
        let take = n.min(held.len());
        let done: Vec<(u32, u32)> = held.drain(..take).map(|h| (u32::from(h), 0)).collect();
        self.push_used(q, &done);
    }

    fn process_ctrl(&mut self, q: u32) {
        let mut done = Vec::new();
        while let Some(head) = self.take_avail(q) {
            let chain = self.chain(q, head);
            let mut command = Vec::new();
            let mut ack_at = None;
            for &(addr, len, flags) in &chain {
                if flags & DESC_F_WRITE != 0 {
                    ack_at = Some(addr);
                } else {
                    command.extend_from_slice(unsafe {
                        core::slice::from_raw_parts(addr as *const u8, len as usize)
                    });
                }
            }
            self.ctrl.push(command);
            match ack_at {
                Some(addr) => {
                    unsafe { core::ptr::write_volatile(addr as *mut u8, self.cfg.ctrl_ack) };
                    done.push((u32::from(head), 1));
                }
                None => self.errors.push("control command without a writable ack".into()),
            }
        }
        self.push_used(q, &done);
    }

    /// Header bytes on this device's receive and transmit buffers (5.1.6.1).
    pub fn header_len(&self) -> usize {
        if self.negotiated(f::VERSION_1) || self.negotiated(f::MRG_RXBUF) {
            12
        } else {
            10
        }
    }

    /// Writable bytes of the chain at `head`, as (address, length) pieces.
    fn writable(&mut self, q: u32, head: u16) -> Vec<(u64, u32)> {
        self.chain(q, head)
            .into_iter()
            .filter(|&(_, _, flags)| flags & DESC_F_WRITE != 0)
            .map(|(addr, len, _)| (addr, len))
            .collect()
    }

    /// Receive `frame` on receive queue `q` as section 5.1.6.4 says: a header
    /// with num_buffers, then the frame, over one buffer or, with MRG_RXBUF,
    /// as many as it needs, each but the last filled to its length. False when
    /// the driver has not offered enough room; the buffers stay offered then.
    pub fn deliver(&mut self, q: u32, frame: &[u8]) -> bool {
        let hdr = self.header_len();
        let mut bytes = vec![0u8; hdr];
        bytes.extend_from_slice(frame);
        let merge = self.negotiated(f::MRG_RXBUF);
        // Count the room first, without taking anything.
        let saved = self.queues[q as usize].last_avail;
        let mut heads = Vec::new();
        let mut room = 0usize;
        while room < bytes.len() {
            let Some(head) = self.take_avail(q) else { break };
            let pieces = self.writable(q, head);
            room += pieces.iter().map(|&(_, l)| l as usize).sum::<usize>();
            heads.push((head, pieces));
            if !merge {
                break;
            }
        }
        if room < bytes.len() {
            self.queues[q as usize].last_avail = saved;
            return false;
        }
        if hdr == 12 {
            bytes[10..12].copy_from_slice(&(heads.len() as u16).to_le_bytes());
        }
        let mut offset = 0;
        let mut entries = Vec::new();
        for (head, pieces) in heads {
            let start = offset;
            for (addr, len) in pieces {
                let n = (len as usize).min(bytes.len() - offset);
                unsafe {
                    core::ptr::copy_nonoverlapping(bytes[offset..].as_ptr(), addr as *mut u8, n)
                };
                offset += n;
            }
            entries.push((u32::from(head), (offset - start) as u32));
        }
        self.push_used(q, &entries);
        true
    }

    /// Take `n` offered receive buffers and return them as used entries the
    /// test chooses, for malformed completions: each is (id, len, header).
    /// The header, if given, is written at the start of the taken buffer.
    pub fn deliver_raw(&mut self, q: u32, entries: &[(Option<u32>, u32, Option<Vec<u8>>)]) {
        let mut used = Vec::new();
        for (id, len, header) in entries {
            let head = self.take_avail(q).expect("an offered buffer");
            if let Some(header) = header {
                let pieces = self.writable(q, head);
                let (addr, cap) = pieces[0];
                let n = header.len().min(cap as usize);
                unsafe { core::ptr::copy_nonoverlapping(header.as_ptr(), addr as *mut u8, n) };
            }
            used.push((id.unwrap_or(u32::from(head)), *len));
        }
        self.push_used(q, &used);
    }

    /// The writes made to register `offset`, in order.
    pub fn writes_to(&self, offset: u64) -> Vec<Write> {
        self.log.iter().copied().filter(|w| w.offset == offset).collect()
    }

    /// The position in the log of the first write to `offset`.
    pub fn first_write(&self, offset: u64) -> Option<usize> {
        self.log.iter().position(|w| w.offset == offset)
    }
}

fn set_low(field: &mut u64, value: u32) {
    *field = (*field & !0xffff_ffff) | u64::from(value);
}

fn set_high(field: &mut u64, value: u32) {
    *field = (*field & 0xffff_ffff) | (u64::from(value) << 32);
}
