//! ARM64 address-space identifiers (ASIDs) with generations.
//!
//! Each user address space runs under its own ASID, carried in TTBR0_EL1
//! bits [63:48] (TCR_EL1.A1 = 0). Translations of non-global pages are tagged
//! with the ASID they were walked under, so a switch between address spaces
//! needs no TLB invalidation: the new ASID cannot match the old space's
//! entries (Arm ARM, "Translation Lookaside Buffers", the ASID rules for
//! non-global entries).
//!
//! The ASID field holds 8 or 16 bits (ID_AA64MMFR0_EL1.ASIDBits). There are
//! more address spaces over a machine's life than ASIDs, so ASIDs are handed
//! out in generations:
//!
//! - Within a generation an ASID is given to at most one address space, and is
//!   not given out again, so a CPU's stale entries under that ASID can only
//!   belong to the one space that owned it.
//! - When a generation runs out, the next one starts. Each CPU drops its TLB
//!   once before it runs an ASID of the new generation; that is a local
//!   invalidation, taken on that CPU's next bind, not a broadcast.
//! - An ASID that a CPU may still be running at the rollover (its last bind,
//!   or either TTBR0 shadow word the exception-return corridor installs
//!   verbatim) keeps its owner across the rollover: the owner's context moves
//!   to the new generation in place. That is what lets a CPU keep running an
//!   ASID it bound before the rollover without binding again.
//! - The other address spaces keep their stale contexts and get a fresh ASID
//!   the next time they are bound.
//!
//! The invariant this keeps, and the host tests check, is: whenever a CPU
//! runs ASID `a` for address space `S`, each entry its TLB holds under `a`
//! was walked from `S`'s tables.
//! claim-lint:ok: checked by `no_live_asid_serves_two_address_spaces_across_rollovers` in tools/asid-host-tests/src/lib.rs
//!
//! This module is pure bookkeeping: it executes no instruction and takes no
//! lock. `ttbr0.rs` owns the lock, the barriers and the TLB maintenance.

use alloc::vec::Vec;

/// The kernel root's ASID. Kernel TTBR0 tables are non-global under it, so no
/// user ASID matches them.
pub const KERNEL_ASID: u16 = 0;

const ASID_BITS_IN_CONTEXT: u32 = 16;
const ASID_MASK: u64 = (1 << ASID_BITS_IN_CONTEXT) - 1;

/// An address space's context: generation in bits [63:16], ASID in [15:0].
/// A context of 0 means "not bound yet"; generations start at 1.
#[inline]
const fn context(generation: u64, asid: u16) -> u64 {
    (generation << ASID_BITS_IN_CONTEXT) | asid as u64
}

#[inline]
const fn generation_of(ctx: u64) -> u64 {
    ctx >> ASID_BITS_IN_CONTEXT
}

#[inline]
const fn asid_of(ctx: u64) -> u16 {
    (ctx & ASID_MASK) as u16
}

/// What a bind hands back to the CPU that asked for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    /// The ASID to install with the root.
    pub asid: u16,
    /// The CPU must invalidate its own TLB (a local, non-broadcast
    /// invalidation) before it runs this ASID: a rollover happened since it
    /// last did.
    pub flush_local: bool,
}

/// A root could not be registered without more storage. The caller allocates
/// a vector of at least `capacity` entries outside the lock and hands it to
/// [`AsidSpace::grow`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NeedsCapacity {
    pub capacity: usize,
}

/// The allocator and the registry of live address spaces, keyed by root.
pub struct AsidSpace {
    /// Number of ASIDs, including the kernel's: 256 or 65536.
    count: u32,
    generation: u64,
    /// One bit per ASID: given out (or kept) in this generation.
    used: Vec<u64>,
    /// Next ASID to try, for a next-fit search.
    next: u32,
    /// Per CPU: must drop its TLB before running a new-generation ASID.
    pending_flush: Vec<bool>,
    /// Per CPU: the context of its last bind, in the current generation (a
    /// rollover moves it), or 0 before the CPU's first bind.
    active: Vec<u64>,
    /// Live address spaces: (root, context), sorted by root.
    spaces: Vec<(u64, u64)>,
    rollovers: u64,
    unregistered_binds: u64,
}

impl AsidSpace {
    /// An allocator for `asid_bits`-bit ASIDs on `cpus` CPUs. The hardware
    /// sizes are 8 and 16 bits; the host model also uses narrower ones so that
    /// rollovers come often. A rollover needs a free ASID beyond the up to
    /// three per CPU it keeps, so `asid_bits` must leave room for that.
    /// Each CPU starts with a pending local flush, so what its TLB holds from
    /// boot is dropped before its first user ASID runs.
    pub fn new(asid_bits: u32, cpus: usize) -> Self {
        let count = 1u32 << asid_bits.min(16);
        assert!(count as usize > 3 * cpus + 1, "too few ASIDs for {cpus} CPUs");
        let mut used = Vec::new();
        used.resize(count.div_ceil(64) as usize, 0);
        let mut space = Self {
            count,
            generation: 1,
            used,
            next: 1,
            pending_flush: alloc::vec![true; cpus],
            active: alloc::vec![0; cpus],
            spaces: Vec::new(),
            rollovers: 0,
            unregistered_binds: 0,
        };
        space.mark(KERNEL_ASID);
        space
    }

    /// Number of ASIDs, the kernel's included.
    pub fn asid_count(&self) -> u32 {
        self.count
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn rollovers(&self) -> u64 {
        self.rollovers
    }

    /// Binds of a root that was not registered. Each took a one-off ASID.
    pub fn unregistered_binds(&self) -> u64 {
        self.unregistered_binds
    }

    pub fn registered(&self) -> usize {
        self.spaces.len()
    }

    fn find(&self, root: u64) -> Result<usize, usize> {
        self.spaces.binary_search_by_key(&root, |&(r, _)| r)
    }

    /// Whether one more [`register`](Self::register) fits without growing.
    pub fn has_room(&self) -> Result<(), NeedsCapacity> {
        if self.spaces.len() < self.spaces.capacity() {
            Ok(())
        } else {
            Err(NeedsCapacity { capacity: (self.spaces.capacity() * 2).max(16) })
        }
    }

    /// Record a new address space. It has no ASID until it is first bound.
    /// Does not allocate: when the registry is full it says how much it needs.
    pub fn register(&mut self, root: u64) -> Result<(), NeedsCapacity> {
        match self.find(root) {
            Ok(index) => {
                // A root frame is registered again only after it was
                // retired and reissued; the new space must not inherit the
                // old one's ASID.
                self.spaces[index].1 = 0;
                Ok(())
            }
            Err(index) => {
                self.has_room()?;
                self.spaces.insert(index, (root, 0));
                Ok(())
            }
        }
    }

    /// Move the registry into `storage` if it is larger. Returns whichever
    /// vector the caller should drop, outside the lock.
    pub fn grow(&mut self, mut storage: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
        if storage.capacity() <= self.spaces.capacity() {
            return storage;
        }
        storage.clear();
        storage.extend_from_slice(&self.spaces);
        core::mem::swap(&mut storage, &mut self.spaces);
        storage
    }

    /// Forget a retired address space, before its root frame can be reissued.
    /// Its ASID stays used until the generation ends, so no other space gets
    /// it while a CPU may still hold its entries.
    pub fn unregister(&mut self, root: u64) {
        if let Ok(index) = self.find(root) {
            self.spaces.remove(index);
        }
    }

    /// The ASID `root` runs under in the current generation, for TLB
    /// maintenance targeted at that one space. A space with no current ASID
    /// gets no target, and the caller invalidates across ASIDs instead.
    pub fn current_asid(&self, root: u64) -> Option<u16> {
        let ctx = self.spaces[self.find(root).ok()?].1;
        (ctx != 0 && generation_of(ctx) == self.generation).then_some(asid_of(ctx))
    }

    fn mark(&mut self, asid: u16) {
        self.used[asid as usize / 64] |= 1 << (asid % 64);
    }

    fn is_used(&self, asid: u16) -> bool {
        self.used[asid as usize / 64] & (1 << (asid % 64)) != 0
    }

    fn take_free(&mut self) -> Option<u16> {
        for step in 0..self.count {
            let candidate = (self.next + step) % self.count;
            let asid = candidate as u16;
            if asid != KERNEL_ASID && !self.is_used(asid) {
                self.mark(asid);
                self.next = candidate + 1;
                return Some(asid);
            }
        }
        None
    }

    /// Keep `asid` across the rollover from `old` generation: mark it used and
    /// move its owner, if one is registered, to the new generation.
    fn keep(&mut self, asid: u16, old: u64) {
        if asid == KERNEL_ASID {
            return;
        }
        self.mark(asid);
        let stale = context(old, asid);
        let current = context(self.generation, asid);
        if let Some(entry) = self.spaces.iter_mut().find(|(_, ctx)| *ctx == stale) {
            entry.1 = current;
        }
    }

    /// Start a new generation. `in_use(cpu)` names the two ASIDs that CPU's
    /// TTBR0 shadow words carry, which its exception-return path may install
    /// without binding again.
    fn rollover(&mut self, in_use: &dyn Fn(usize) -> [u16; 2]) {
        let old = self.generation;
        self.generation += 1;
        self.rollovers += 1;
        self.used.iter_mut().for_each(|word| *word = 0);
        self.mark(KERNEL_ASID);
        self.next = 1;
        for cpu in 0..self.active.len() {
            self.pending_flush[cpu] = true;
            let active = self.active[cpu];
            if active != 0 {
                self.keep(asid_of(active), old);
                self.active[cpu] = context(self.generation, asid_of(active));
            }
            for asid in in_use(cpu) {
                self.keep(asid, old);
            }
        }
    }

    /// Give `root` an ASID to run under on `cpu`, starting a new generation
    /// first when the current one has no free ASID. Does not allocate.
    pub fn bind(&mut self, cpu: usize, root: u64, in_use: &dyn Fn(usize) -> [u16; 2]) -> Binding {
        let index = self.find(root).ok();
        let current = |space: &Self| {
            index
                .map(|i| space.spaces[i].1)
                .filter(|&ctx| ctx != 0 && generation_of(ctx) == space.generation)
        };
        let ctx = match current(self) {
            Some(ctx) => ctx,
            None => {
                let asid = match self.take_free() {
                    Some(asid) => Some(asid),
                    None => {
                        self.rollover(in_use);
                        // The rollover may have kept this space's ASID.
                        if current(self).is_some() { None } else { self.take_free() }
                    }
                };
                match (asid, index) {
                    (Some(asid), Some(i)) => {
                        let ctx = context(self.generation, asid);
                        self.spaces[i].1 = ctx;
                        ctx
                    }
                    (Some(asid), None) => {
                        self.unregistered_binds += 1;
                        context(self.generation, asid)
                    }
                    (None, _) => current(self).expect("rollover keeps a bound space"),
                }
            }
        };
        let flush_local = core::mem::replace(&mut self.pending_flush[cpu], false);
        self.active[cpu] = ctx;
        Binding { asid: asid_of(ctx), flush_local }
    }
}
