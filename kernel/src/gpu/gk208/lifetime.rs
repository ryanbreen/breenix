//! Breenix-owned mapping and consumer references. No hardware reset is attempted.
//! CPU-only cancellation is separate from trusted engine drain/reset receipts.
use super::{
    memory::allocator::{Allocation, Allocator, Pin},
    vm::{self, Flags, Io, Space},
};
use alloc::{sync::Arc, vec::Vec};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Consumer {
    Queued,
    Readback,
    Scanout,
}
impl Consumer {
    fn index(self) -> usize {
        match self {
            Self::Queued => 0,
            Self::Readback => 1,
            Self::Scanout => 2,
        }
    }
}
struct Mapping {
    va: u64,
    allocation: Allocation,
    _pin: Pin,
}
struct Residency {
    mappings: Vec<Mapping>,
    tables: Vec<Pin>,
}
/// Ticket identity includes its originating residency, not a reusable physical VA.
/// Dropping a caller ticket does not drop the context's queued reference.
pub struct Ticket {
    owner: Arc<Residency>,
    sequence: u64,
}
struct Pending {
    ticket: Ticket,
    consumer: Consumer,
}
pub struct Context {
    space: Option<Space>,
    residency: Arc<Residency>,
    pending: Vec<Pending>,
    next: u64,
    failed: bool,
    device: bool,
    ever_device: bool,
}
impl Context {
    pub fn new(a: &mut Allocator, io: &mut impl Io) -> Result<Self, &'static str> {
        let space = Space::new(a, io)?;
        let tables = space
            .allocations()
            .map(|x| a.pin(x))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            space: Some(space),
            residency: Arc::new(Residency {
                mappings: Vec::new(),
                tables,
            }),
            pending: Vec::new(),
            next: 1,
            failed: false,
            device: false,
            ever_device: false,
        })
    }
    pub fn space(&self) -> &Space {
        self.space.as_ref().expect("retired context")
    }
    fn mutable(&self) -> Result<(), &'static str> {
        if self.failed
            || self.device
            || self.space.is_none()
            || !self.pending.is_empty()
            || Arc::strong_count(&self.residency) != 1
        {
            return Err("GPU context is retained");
        }
        Ok(())
    }
    fn refresh_tables(&mut self, a: &Allocator) -> Result<(), &'static str> {
        let pins = self
            .space()
            .allocations()
            .map(|x| a.pin(x))
            .collect::<Result<Vec<_>, _>>()?;
        Arc::get_mut(&mut self.residency)
            .ok_or("GPU context has outstanding references")?
            .tables = pins;
        Ok(())
    }
    pub fn map(
        &mut self,
        a: &mut Allocator,
        io: &mut impl Io,
        va: u64,
        allocation: Allocation,
        flags: Flags,
    ) -> Result<(), &'static str> {
        self.mutable()?;
        let pin = a.pin(allocation)?;
        let r = Arc::get_mut(&mut self.residency).unwrap();
        r.mappings
            .try_reserve(1)
            .map_err(|_| "mapping reference storage exhausted")?;
        // Retain even a partially published mapping if device publication fails.
        r.mappings.push(Mapping {
            va,
            allocation,
            _pin: pin,
        });
        let result = self
            .space
            .as_mut()
            .unwrap()
            .map(a, io, va, allocation.range, flags);
        if result.is_err() {
            self.failed = true;
        }
        self.refresh_tables(a)?;
        result
    }
    pub fn remap(
        &mut self,
        a: &Allocator,
        io: &mut impl Io,
        va: u64,
        allocation: Allocation,
        flags: Flags,
    ) -> Result<(), &'static str> {
        self.mutable()?;
        let index = self
            .residency
            .mappings
            .iter()
            .position(|m| m.va == va && m.allocation.range.size == allocation.range.size)
            .ok_or("no complete owned mapping")?;
        let pin = a.pin(allocation)?;
        let r = Arc::get_mut(&mut self.residency).unwrap();
        r.mappings
            .try_reserve(1)
            .map_err(|_| "mapping reference storage exhausted")?;
        r.mappings.push(Mapping {
            va,
            allocation,
            _pin: pin,
        });
        let result = self
            .space
            .as_mut()
            .unwrap()
            .remap(io, va, allocation.range, flags);
        if result.is_ok() {
            Arc::get_mut(&mut self.residency)
                .unwrap()
                .mappings
                .remove(index);
        } else {
            self.failed = true;
        }
        result
    }
    pub fn unmap(&mut self, io: &mut impl Io, va: u64) -> Result<(), &'static str> {
        self.mutable()?;
        let index = self
            .residency
            .mappings
            .iter()
            .position(|m| m.va == va)
            .ok_or("no owned mapping")?;
        let size = self.residency.mappings[index].allocation.range.size;
        let result = self.space.as_mut().unwrap().unmap(io, va, size);
        if result.is_ok() {
            Arc::get_mut(&mut self.residency)
                .unwrap()
                .mappings
                .remove(index);
        } else {
            self.failed = true;
        }
        result
    }
    pub fn hold(&mut self, consumer: Consumer) -> Result<Ticket, &'static str> {
        if self.failed || self.space.is_none() || self.next == u64::MAX {
            return Err("GPU context not accepting work");
        }
        self.pending
            .try_reserve(1)
            .map_err(|_| "consumer reference storage exhausted")?;
        let sequence = self.next;
        self.next += 1;
        self.pending.push(Pending {
            ticket: Ticket {
                owner: self.residency.clone(),
                sequence,
            },
            consumer,
        });
        Ok(Ticket {
            owner: self.residency.clone(),
            sequence,
        })
    }
    pub fn holds(&self) -> [usize; 3] {
        let mut counts = [0; 3];
        for p in &self.pending {
            counts[p.consumer.index()] += 1;
        }
        counts
    }
    fn finish(&mut self, ticket: Ticket) -> Result<(), &'static str> {
        if !Arc::ptr_eq(&ticket.owner, &self.residency) {
            return Err("foreign completion ticket");
        }
        let index = self
            .pending
            .iter()
            .position(|p| p.ticket.sequence == ticket.sequence)
            .ok_or("stale completion ticket")?;
        self.pending.remove(index);
        Ok(())
    }
    /// Available only before device admission; this cannot release engine work.
    pub fn finish_cpu(&mut self, ticket: Ticket) -> Result<(), &'static str> {
        if self.ever_device {
            return Err("device completion required");
        }
        self.finish(ticket)
    }
    pub fn fail_eio(&mut self) {
        self.failed = true;
    }
    pub fn reset_failed(&mut self) {
        self.failed = true;
    }
    /// Match the full decoded instance address; leave unrelated contexts alone.
    pub fn attribute(&mut self, fault: &vm::Fault) -> bool {
        if self
            .space
            .as_ref()
            .is_some_and(|s| s.instance.range.start == fault.instance)
        {
            self.fail_eio();
            true
        } else {
            false
        }
    }
    /// CPU construction exercises may cancel an unsubmitted EIO after consumers
    /// end. This does not repair uncertain hardware publication or poison in Space.
    pub fn cancel_unsubmitted(&mut self) -> Result<(), &'static str> {
        if self.ever_device || !self.pending.is_empty() || Arc::strong_count(&self.residency) != 1 {
            return Err("device drain or reset required");
        }
        self.failed = false;
        Ok(())
    }
    pub fn retire(&mut self, a: &mut Allocator, io: &mut impl Io) -> Result<(), &'static str> {
        self.mutable()?;
        if let Err(e) = self.space.as_mut().unwrap().detach(io) {
            self.failed = true;
            return Err(e);
        }
        // Only successful detach permits dropping mapping/table references.
        let r = Arc::get_mut(&mut self.residency).unwrap();
        r.mappings.clear();
        r.tables.clear();
        self.space.take().unwrap().reclaim(a)
    }
    /// # Safety
    /// The scheduler must serialize this owner and pin it before publishing a
    /// channel, push buffer or scanout binding. Call only with owned storage.
    #[allow(dead_code)] // Public scheduler admission API consumed by milestone 6.
    pub unsafe fn admit_device(&mut self) -> Result<(), &'static str> {
        self.mutable()?;
        self.device = true;
        self.ever_device = true;
        Ok(())
    }
    /// # Safety
    /// An engine-ordered completion for this exact ticket must have been observed.
    /// For scanout, the display must have flipped away and stopped reading it.
    #[allow(dead_code)] // Public completion API consumed by milestone 6.
    pub unsafe fn finish_device(&mut self, ticket: Ticket) -> Result<(), &'static str> {
        self.finish(ticket)
    }
    /// # Safety
    /// Scheduling/bindings must be disabled, and actual device access drained.
    #[allow(dead_code)] // Public drain API consumed by milestone 6.
    pub unsafe fn confirm_drain(&mut self) {
        self.device = false;
    }
    /// # Safety
    /// A real reset must be confirmed to have stopped engine access to this space.
    /// Scanout reads VRAM directly and remains retained until display completion.
    #[allow(dead_code)] // Public reset API consumed by milestone 6; no M5 caller.
    pub unsafe fn confirm_reset(&mut self) {
        self.pending.retain(|p| p.consumer == Consumer::Scanout);
        self.device = false;
        self.failed = false;
        if let Some(s) = self.space.as_mut() {
            unsafe {
                s.reset_confirmed();
            }
        }
    }
}
impl Drop for Context {
    fn drop(&mut self) {
        if self.space.is_some() {
            // Abandoning an owner is not a drain receipt. Keep its residency
            // permanently pinned rather than permit another allocator release.
            core::mem::forget(self.residency.clone());
        }
    }
}
