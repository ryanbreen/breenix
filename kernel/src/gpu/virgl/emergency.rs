//! Terminal submission through reserved split-ring descriptors (VirtIO 1.3 CSD01,
//! sections 2.7.5, 2.7.13 and 2.7.14). The caller has stopped normal producers.
//! No regular queue bookkeeping or request ownership is changed: interrupted normal
//! chains and their DMA buffers stay held. A timeout also retains the terminal chain.
use super::{
    queue::{Segment, PANIC_DESCRIPTORS},
    wire, Payload, Request,
};
use crate::gpu::dma::{self, PAGE};
use alloc::vec::Vec;
use core::sync::atomic::{fence, Ordering};

pub(crate) struct Command {
    request: Request,
    segments: Vec<Segment>,
}
impl Command {
    pub(crate) fn new(request: Request) -> Self {
        let segments = request.segments();
        Self { request, segments }
    }
    /// Only the terminal owner patches these preallocated command pages, before send.
    pub(crate) fn patch(&self, word: usize, value: u32) {
        if let Payload::Words(w) = &self.request.payload {
            w.page(word / (PAGE / 4))
                .write(word % (PAGE / 4) * 4, &value.to_le_bytes());
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Queue {
    size: u16,
    desc: u64,
    avail: u64,
    used: u64,
    doorbell: u64,
}
impl Queue {
    pub(super) fn new(size: u16, desc: u64, avail: u64, used: u64, doorbell: u64) -> Self {
        Self {
            size,
            desc,
            avail,
            used,
            doorbell,
        }
    }
    /// Finish published commands before issuing the terminal bind, so an
    /// outstanding normal SET_SCANOUT cannot replace it afterwards.
    pub(crate) fn drain(&self, deadline: u64) -> bool {
        if self.doorbell == 0 {
            return false;
        }
        dma::invalidate(self.avail, PAGE);
        // SAFETY: the terminal latch has stopped the normal available-ring writer.
        let avail = unsafe { core::ptr::read_volatile((self.avail + 2) as *const u16) };
        // The interrupted producer may have published idx before claiming its kick.
        // VirtIO section 4.1.4.4: notify queue 0 of those already-published buffers.
        unsafe { core::ptr::write_volatile(self.doorbell as *mut u16, 0) };
        while crate::arch_impl::aarch64::timer::rdtsc() < deadline {
            dma::invalidate(self.used, PAGE);
            let used = unsafe { core::ptr::read_volatile((self.used + 2) as *const u16) };
            if used == avail {
                return true;
            }
            if avail.wrapping_sub(used) > self.size {
                return false;
            }
            core::hint::spin_loop();
        }
        false
    }

    /// One request at a time; reuse the reserved descriptors only after their head
    /// returns. Other heads belong to the frozen normal queue and are left intact.
    pub(crate) fn exec(&self, c: &Command, deadline: u64) -> bool {
        if c.segments.len() > PANIC_DESCRIPTORS as usize || self.doorbell == 0 {
            return false;
        }
        let head = self.size - PANIC_DESCRIPTORS;
        dma::invalidate(self.used, PAGE);
        dma::invalidate(self.avail, PAGE);
        // SAFETY: queue pages are pinned by the normal device. Its producers have
        // quiesced; only this terminal owner writes the available ring now.
        let (mut used, avail) = unsafe {
            (
                core::ptr::read_volatile((self.used + 2) as *const u16),
                core::ptr::read_volatile((self.avail + 2) as *const u16),
            )
        };
        c.request.publish();
        for (i, s) in c.segments.iter().enumerate() {
            let id = head + i as u16;
            let d = self.desc + id as u64 * 16;
            let next = i + 1 < c.segments.len();
            // SAFETY: descriptors excluded from Queue::reset's normal free list.
            unsafe {
                core::ptr::write_volatile(d as *mut u64, s.addr);
                core::ptr::write_volatile((d + 8) as *mut u32, s.len);
                core::ptr::write_volatile(
                    (d + 12) as *mut u16,
                    u16::from(next) | if s.write { 2 } else { 0 },
                );
                core::ptr::write_volatile((d + 14) as *mut u16, if next { id + 1 } else { 0 });
            }
        }
        dma::clean(self.desc, PAGE);
        // SAFETY: slot is inside the available ring.
        unsafe {
            core::ptr::write_volatile(
                (self.avail + 4 + 2 * (avail % self.size) as u64) as *mut u16,
                head,
            )
        };
        dma::clean(self.avail, PAGE);
        fence(Ordering::SeqCst);
        unsafe { core::ptr::write_volatile((self.avail + 2) as *mut u16, avail.wrapping_add(1)) };
        dma::clean(self.avail, PAGE);
        fence(Ordering::SeqCst);
        unsafe { core::ptr::write_volatile(self.doorbell as *mut u16, 0) };
        while crate::arch_impl::aarch64::timer::rdtsc() < deadline {
            dma::invalidate(self.used, PAGE);
            let end = unsafe { core::ptr::read_volatile((self.used + 2) as *const u16) };
            fence(Ordering::SeqCst);
            if end.wrapping_sub(used) > self.size {
                return false;
            }
            while used != end {
                let at = self.used + 4 + 8 * (used % self.size) as u64;
                let (id, len) = unsafe {
                    (
                        core::ptr::read_volatile(at as *const u32),
                        core::ptr::read_volatile((at + 4) as *const u32),
                    )
                };
                used = used.wrapping_add(1);
                if id >= self.size as u32 {
                    return false;
                }
                if id != head as u32 {
                    continue;
                }
                if len != wire::HDR as u32 {
                    return false;
                }
                c.request.resp.observe();
                return c.request.resp.u32_at(wire::HDR_TYPE) == wire::resp::OK_NODATA
                    && c.request.fence.is_none_or(|f| {
                        c.request.resp.u32_at(wire::HDR_FLAGS) & wire::FLAG_FENCE != 0
                            && c.request.resp.u64_at(wire::HDR_FENCE) == f
                    });
            }
            core::hint::spin_loop();
        }
        false
    }
}
