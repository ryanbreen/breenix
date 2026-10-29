// Live native DMA ownership. Metadata holds no reference to the allocation.
use alloc::{format, string::String};
use core::panic::Location;

#[derive(Clone, Copy)]
struct Record {
    site: &'static Location<'static>,
    owner: &'static str,
    resource: u32,
    context: u32,
    pages: usize,
}

const CAPACITY: usize = 4096;
static LIVE: spin::Mutex<[Option<Record>; CAPACITY]> = spin::Mutex::new([None; CAPACITY]);

pub struct Allocation(usize);
impl Allocation {
    // Exhaustion refuses the allocation; no unaccounted native DMA is admitted.
    #[track_caller]
    pub fn new(pages: usize, owner: &'static str) -> Option<Self> {
        let mut live = LIVE.lock();
        let slot = live.iter().position(Option::is_none)?;
        live[slot] = Some(Record { site: Location::caller(), owner, resource: 0, context: 0, pages });
        Some(Self(slot))
    }

    pub fn identify(&self, owner: &'static str, resource: u32, context: u32) {
        if let Some(r) = LIVE.lock()[self.0].as_mut() {
            r.owner = owner;
            if resource != 0 { r.resource = resource; }
            if context != 0 { r.context = context; }
        }
    }
}
impl Drop for Allocation {
    fn drop(&mut self) { LIVE.lock()[self.0] = None; }
}

pub fn census() -> String {
    // Copy bounded metadata, then format outside the lock. Taking the snapshot
    // neither clones backing Arcs nor changes the DMA lifetime it observes.
    let mut snapshot = alloc::vec::Vec::with_capacity(CAPACITY);
    snapshot.extend(LIVE.lock().iter().flatten().copied());
    let pages: usize = snapshot.iter().map(|r| r.pages).sum();
    let count = snapshot.iter().count();
    let mut text = format!("gpu-dma pages={pages} allocations={count}\n");
    for r in snapshot.iter() {
        text.push_str(&format!("gpu-dma owner={} resource={} context={} pages={} site={}:{}\n",
            r.owner, r.resource, r.context, r.pages,
            r.site.file().strip_prefix("kernel/src/").unwrap_or(r.site.file()), r.site.line()));
    }
    text
}
