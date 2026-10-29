// Live native DMA ownership. Metadata holds no reference to the allocation.
use alloc::{format, string::String, vec::Vec};
use core::panic::Location;

#[derive(Clone, Copy)]
struct Record {
    site: &'static Location<'static>,
    owner: &'static str,
    resource: u32,
    context: u32,
    pages: usize,
}

const CENSUS_ROWS: usize = 4096;
struct Registry {
    slots: Vec<Option<Record>>,
    free: Vec<usize>,
}
static LIVE: spin::Mutex<Registry> = spin::Mutex::new(Registry { slots: Vec::new(), free: Vec::new() });

pub struct Allocation(usize);
impl Allocation {
    // Metadata grows with real allocations, rather than adding a DMA quota.
    #[track_caller]
    pub fn new(pages: usize, owner: &'static str) -> Option<Self> {
        let mut live = LIVE.lock();
        let slot = if let Some(slot) = live.free.pop() { slot } else {
            live.slots.try_reserve(1).ok()?;
            // Reserve a return slot now: Drop must not allocate metadata.
            let spare = live.slots.len() + 1 - live.free.len();
            live.free.try_reserve(spare).ok()?;
            let slot = live.slots.len();
            live.slots.push(None);
            slot
        };
        live.slots[slot] = Some(Record { site: Location::caller(), owner, resource: 0, context: 0, pages });
        Some(Self(slot))
    }

    pub fn identify(&self, owner: &'static str, resource: u32, context: u32) {
        if let Some(r) = LIVE.lock().slots[self.0].as_mut() {
            r.owner = owner;
            if resource != 0 { r.resource = resource; }
            if context != 0 { r.context = context; }
        }
    }
}
impl Drop for Allocation {
    fn drop(&mut self) {
        let mut live = LIVE.lock();
        live.slots[self.0] = None;
        live.free.push(self.0);
    }
}

pub fn census() -> String {
    // Copy metadata, then aggregate and format outside the lock. The snapshot
    // neither clones backing Arcs nor changes the DMA lifetime it observes.
    let mut snapshot = Vec::with_capacity(LIVE.lock().slots.len());
    snapshot.extend(LIVE.lock().slots.iter().flatten().copied());
    let pages: usize = snapshot.iter().map(|r| r.pages).sum();
    let count = snapshot.iter().count();
    let key = |r: &Record| (r.site.file(), r.site.line(), r.site.column(), r.owner, r.resource, r.context);
    snapshot.sort_unstable_by_key(key);
    let mut groups: Vec<Record> = Vec::new();
    let mut omitted_pages = 0;
    for r in snapshot {
        if let Some(last) = groups.last_mut().filter(|last| key(last) == key(&r)) {
            last.pages += r.pages;
        } else if groups.len() < CENSUS_ROWS {
            groups.push(r);
        } else {
            omitted_pages += r.pages;
        }
    }
    let mut text = format!("gpu-dma pages={pages} allocations={count} omitted_pages={omitted_pages}\n");
    for r in groups {
        text.push_str(&format!("gpu-dma owner={} resource={} context={} pages={} site={}:{}\n",
            r.owner, r.resource, r.context, r.pages,
            r.site.file().strip_prefix("kernel/src/").unwrap_or(r.site.file()), r.site.line()));
    }
    text
}
