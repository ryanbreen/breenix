//! A small virtio-blk device over host memory. It reads the descriptor table,
//! available ring and request headers the production queue wrote, walks each
//! chain the way section 2.7.5 describes, moves data between the chain's
//! buffers and its disk, writes the status byte, and returns chains to the
//! used ring in whatever order the test asks.

use crate::blk_queue::{request_type, status, Desc, DmaMemory, RingMemory};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

const NEXT: u16 = 1;
const WRITE: u16 = 2;

/// Bus memory: the chains' headers, status bytes and data frames.
#[derive(Default)]
pub struct Bus {
    pages: BTreeMap<u64, Box<[u8; 4096]>>,
    /// Every driver store, by address, so a test can see what the driver touched.
    pub driver_writes: Vec<u64>,
}

impl Bus {
    pub fn store(&mut self, addr: u64, data: &[u8]) {
        for (offset, byte) in data.iter().enumerate() {
            let at = addr + offset as u64;
            self.pages.entry(at & !0xfff).or_insert_with(|| Box::new([0; 4096]))[(at & 0xfff) as usize] = *byte;
        }
    }

    pub fn load(&self, addr: u64, out: &mut [u8]) {
        for (offset, byte) in out.iter_mut().enumerate() {
            let at = addr + offset as u64;
            *byte = self.pages.get(&(at & !0xfff)).map_or(0, |page| page[(at & 0xfff) as usize]);
        }
    }
}

pub struct Shared {
    pub bus: Bus,
    pub size: u16,
    pub desc: Vec<Desc>,
    pub avail: Vec<u16>,
    pub avail_idx: u16,
    pub used: Vec<(u32, u32)>,
    pub used_idx: u16,
    /// Index stores the driver made, and whether a barrier preceded each.
    pub idx_stores: Vec<(u16, bool)>,
    barrier_pending: bool,
}

pub type Handle = Rc<RefCell<Shared>>;

pub fn shared(size: u16) -> Handle {
    Rc::new(RefCell::new(Shared {
        bus: Bus::default(),
        size,
        desc: vec![Desc { addr: 0, len: 0, flags: 0, next: 0 }; size as usize],
        avail: vec![0; size as usize],
        avail_idx: 0,
        used: vec![(0, 0); size as usize],
        used_idx: 0,
        idx_stores: Vec::new(),
        barrier_pending: false,
    }))
}

/// The queue's view of the ring memory.
pub struct Ring(pub Handle);

impl RingMemory for Ring {
    fn write_desc(&mut self, index: u16, desc: Desc) {
        self.0.borrow_mut().desc[index as usize] = desc;
    }
    fn write_avail_entry(&mut self, pos: u16, head: u16) {
        self.0.borrow_mut().avail[pos as usize] = head;
    }
    fn barrier_before_avail_idx(&mut self) {
        self.0.borrow_mut().barrier_pending = true;
    }
    fn write_avail_idx(&mut self, idx: u16) {
        let mut shared = self.0.borrow_mut();
        let barrier = shared.barrier_pending;
        shared.barrier_pending = false;
        shared.avail_idx = idx;
        shared.idx_stores.push((idx, barrier));
    }
    fn barrier_before_notify(&mut self) {}
    fn read_used_idx(&mut self) -> u16 {
        self.0.borrow().used_idx
    }
    fn read_used_elem(&mut self, pos: u16) -> (u32, u32) {
        self.0.borrow().used[pos as usize]
    }
}

/// The queue's view of bus memory.
pub struct Dma(pub Handle);

impl DmaMemory for Dma {
    fn write_bytes(&mut self, addr: u64, bytes: &[u8]) {
        let mut shared = self.0.borrow_mut();
        shared.bus.store(addr, bytes);
        shared.bus.driver_writes.push(addr);
    }
    fn read_bytes(&mut self, addr: u64, out: &mut [u8]) {
        self.0.borrow().bus.load(addr, out);
    }
}

/// One chain the device has taken from the available ring.
#[derive(Clone, Debug)]
pub struct Taken {
    pub head: u16,
    pub kind: u32,
    pub sector: u64,
    pub data: Vec<Desc>,
    pub status: Desc,
}

pub struct Device {
    pub mem: Handle,
    pub disk: Vec<u8>,
    seen: u16,
}

impl Device {
    pub fn new(mem: &Handle, sectors: usize) -> Self {
        Self { mem: mem.clone(), disk: vec![0; sectors * 512], seen: 0 }
    }

    /// Take every chain published since the last call, in available order.
    pub fn take(&mut self) -> Vec<Taken> {
        let shared = self.mem.borrow();
        let mut out = Vec::new();
        while self.seen != shared.avail_idx {
            let head = shared.avail[(self.seen % shared.size) as usize];
            self.seen = self.seen.wrapping_add(1);
            let mut parts = Vec::new();
            let mut index = head;
            loop {
                let desc = shared.desc[index as usize];
                parts.push(desc);
                assert!(parts.len() <= shared.size as usize, "chain longer than the queue");
                if desc.flags & NEXT == 0 {
                    break;
                }
                index = desc.next;
            }
            let header = parts[0];
            assert_eq!((header.len, header.flags & WRITE), (16, 0), "header is 16 device-readable bytes");
            let mut bytes = [0u8; 16];
            shared.bus.load(header.addr, &mut bytes);
            let status = *parts.last().unwrap();
            assert_eq!((status.len, status.flags & WRITE), (1, WRITE), "status is one device-writable byte");
            out.push(Taken {
                head,
                kind: u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
                sector: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
                data: parts[1..parts.len() - 1].to_vec(),
                status,
            });
        }
        out
    }

    /// Carry out `chain` with `result` and return it on the used ring with
    /// the length the device wrote, or `len_override` in its place.
    pub fn complete(&mut self, chain: &Taken, result: u8, len_override: Option<u32>) {
        let mut written = 1u32;
        if result == status::OK {
            let mut offset = chain.sector as usize * 512;
            for desc in &chain.data {
                let len = desc.len as usize;
                let mut shared = self.mem.borrow_mut();
                match chain.kind {
                    request_type::IN => {
                        assert_eq!(desc.flags & WRITE, WRITE, "read data is device-writable");
                        shared.bus.store(desc.addr, &self.disk[offset..offset + len]);
                        written += len as u32;
                    }
                    request_type::OUT => {
                        assert_eq!(desc.flags & WRITE, 0, "write data is device-readable");
                        shared.bus.load(desc.addr, &mut self.disk[offset..offset + len]);
                    }
                    other => panic!("unexpected request type {other}"),
                }
                offset += len;
            }
        }
        let mut shared = self.mem.borrow_mut();
        shared.bus.store(chain.status.addr, &[result]);
        self.post(&mut shared, chain.head as u32, len_override.unwrap_or(written));
    }

    /// Return an arbitrary id on the used ring, as a broken device might.
    pub fn post_raw(&mut self, id: u32, len: u32) {
        let mut shared = self.mem.borrow_mut();
        self.post(&mut shared, id, len);
    }

    fn post(&self, shared: &mut Shared, id: u32, len: u32) {
        let pos = (shared.used_idx % shared.size) as usize;
        shared.used[pos] = (id, len);
        shared.used_idx = shared.used_idx.wrapping_add(1);
    }
}
