//! BreenFS-only dirty pages; the mount serializes this per-VNode state.
//! Clean pages belong to the buffer cache (E24 coordinator ledger line 2).
use super::preempt::preemption_point;
use super::format::BLOCK_SIZE;
use crate::{fs::errno::Errno, syscall::errno::ENOMEM};
use alloc::{collections::BTreeMap, sync::Arc, vec::Vec};

#[cfg(test)]
pub mod measurement {
    extern crate std;
    std::thread_local! {
        static VISITS: core::cell::Cell<u64> = const { core::cell::Cell::new(0) };
        static INDEXES: core::cell::Cell<u64> = const { core::cell::Cell::new(0) };
        static DRAINED: core::cell::Cell<u64> = const { core::cell::Cell::new(0) };
    }
    pub fn take_visits() -> u64 {
        VISITS.with(|count| count.replace(0))
    }
    pub(super) fn visited() {
        VISITS.with(|count| count.set(count.get() + 1));
    }
    pub(super) fn indexed() { INDEXES.with(|n| n.set(n.get() + 1)); }
    pub(super) fn drained() { DRAINED.with(|n| n.set(n.get() + 1)); }
    pub fn take_work() -> [u64; 3] {
        [take_visits(), INDEXES.with(|n| n.replace(0)), DRAINED.with(|n| n.replace(0))]
    }
}

// Pages are shared, never changed in place: a frozen generation's map stays
// readable while its preparation drains a clone (E24 coordinator 597/601(2)).
type Page = Arc<Vec<u8>>;

#[derive(Clone, Default)]
pub struct DirtyPages {
    pages: BTreeMap<u64, Page>,
}
impl DirtyPages {
    pub fn bytes(&self) -> u64 {
        self.pages.len() as u64 * BLOCK_SIZE as u64
    }
    pub fn get(&self, page: u64) -> Option<&[u8]> {
        self.pages.get(&page).map(|page| page.as_slice())
    }
    pub fn indexes(&self) -> impl Iterator<Item = u64> + '_ {
        #[cfg(test)]
        { self.pages.keys().copied().inspect(|_| measurement::indexed()) }
        #[cfg(not(test))]
        self.pages.keys().copied()
    }
    /// Project write_runs after admission, without changing pages. Adjacent
    /// writes share the eventual run and its metadata reservation.
    pub fn reservation_runs(&self, offset: u64, length: usize) -> (u64, u64, u64) {
        let first = offset / BLOCK_SIZE as u64;
        let last = if length == 0 { first } else {
            (offset + length as u64).div_ceil(BLOCK_SIZE as u64)
        };
        let old_pages = self.pages.len() as u64;
        let incoming_pages = last - first;
        let Some((&old_first, _)) = self.pages.first_key_value() else {
            return (incoming_pages, incoming_pages, incoming_pages.div_ceil(256));
        };
        let old_end = self.pages.last_key_value().unwrap().0 + 1;
        // Equal span and entry count identify a contiguous dirty range.
        // Project this union from its endpoints, rather than walking the growing
        // map for each small sequential write (#3040).
        if old_end - old_first == old_pages {
            if incoming_pages == 0 {
                return (0, old_pages, old_pages.div_ceil(256));
            }
            let overlap = old_end.min(last).saturating_sub(old_first.max(first));
            let added = incoming_pages - overlap;
            let pages = old_pages + added;
            let runs = if last < old_first || first > old_end {
                old_pages.div_ceil(256) + incoming_pages.div_ceil(256)
            } else {
                pages.div_ceil(256)
            };
            return (added, pages, runs);
        }
        let mut old = self.indexes().peekable();
        let mut incoming = (first..last).peekable();
        let mut pages = 0;
        let mut runs = 0;
        let mut previous = None;
        let mut run_pages = 0;
        loop {
            let index = match (old.peek(), incoming.peek()) {
                (Some(&a), Some(&b)) => a.min(b),
                (Some(&a), None) | (None, Some(&a)) => a,
                (None, None) => break,
            };
            #[cfg(test)]
            measurement::visited();
            if old.peek() == Some(&index) { old.next(); }
            if incoming.peek() == Some(&index) { incoming.next(); }
            if previous != Some(index) || run_pages == 256 {
                runs += 1;
                run_pages = 0;
            }
            pages += 1;
            run_pages += 1;
            previous = Some(index + 1);
        }
        (pages - self.pages.len() as u64, pages, runs)
    }
    pub fn clear(&mut self) {
        self.pages.clear();
    }
    // Prepare the touched pages before publishing any of them. Read failures
    // leave the previous dirty bytes intact, including partial-page overwrites.
    pub fn write(
        &mut self,
        offset: u64,
        bytes: &[u8],
        mut read: impl FnMut(u64, &mut [u8]) -> Result<(), Errno>,
    ) -> Result<(), Errno> {
        let mut prepared = BTreeMap::new();
        let mut done = 0;
        while done < bytes.len() {
            let pos = offset + done as u64;
            let index = pos / BLOCK_SIZE as u64;
            let within = (pos % BLOCK_SIZE as u64) as usize;
            let take = (bytes.len() - done).min(BLOCK_SIZE - within);
            let mut page = zero_page()?;
            if within != 0 || take != BLOCK_SIZE {
                if let Some(old) = self.get(index) {
                    page.copy_from_slice(old);
                } else {
                    read(index * BLOCK_SIZE as u64, &mut page)?;
                }
            }
            page[within..within + take].copy_from_slice(&bytes[done..done + take]);
            prepared.insert(index, Arc::try_new(page).map_err(|_| Errno(ENOMEM))?);
            done += take;
        }
        // Bulk append rebuilds the existing index even for a one-page write
        // (#3040). Insert the prepared pages into the retained index instead.
        for (index, page) in prepared {
            self.pages.insert(index, page);
        }
        Ok(())
    }
    // Only a mapped/dirty partial tail needs a page. Growing a hole does not.
    pub fn shrink(
        &mut self,
        size: u64,
        mapped_tail: bool,
        mut read: impl FnMut(u64, &mut [u8]) -> Result<(), Errno>,
    ) -> Result<(), Errno> {
        let index = size / BLOCK_SIZE as u64;
        let within = (size % BLOCK_SIZE as u64) as usize;
        let tail = if within != 0 && (mapped_tail || self.get(index).is_some()) {
            let mut page = zero_page()?;
            if let Some(old) = self.get(index) {
                page.copy_from_slice(old);
            } else {
                read(index * BLOCK_SIZE as u64, &mut page)?;
            }
            page[within..].fill(0);
            Some(Arc::try_new(page).map_err(|_| Errno(ENOMEM))?)
        } else {
            None
        };
        self.pages.split_off(&size.div_ceil(BLOCK_SIZE as u64));
        if let Some(page) = tail {
            self.pages.insert(index, page);
        }
        Ok(())
    }
    // Move source pages into the transaction's private extents. Runs stop at
    // a hole, EOF or 256 blocks (the data REF limit in design section 2.4).
    // Each page is freed once copied, so the dirty map and the transaction
    // do not both hold the generation's data (E24 ledger 309, C3). Pages past
    // EOF are dropped; the caller fails the mount if any write fails.
    pub fn write_runs(
        &mut self,
        size: u64,
        mut write: impl FnMut(u64, &[u8]) -> Result<(), Errno>,
    ) -> Result<(), Errno> {
        while let Some((first, page)) = self.pages.pop_first() {
            preemption_point();
            #[cfg(test)]
            measurement::drained();
            let offset = first * BLOCK_SIZE as u64;
            if offset >= size {
                self.pages.clear();
                break;
            }
            let mut run = Vec::new();
            run.try_reserve_exact(256 * BLOCK_SIZE)
                .map_err(|_| Errno(ENOMEM))?;
            run.extend_from_slice(&page);
            drop(page);
            let mut next = first + 1;
            while run.len() < 256 * BLOCK_SIZE
                && self
                    .pages
                    .first_key_value()
                    .is_some_and(|(index, _)| *index == next)
            {
                if let Some((_, page)) = self.pages.pop_first() {
                    #[cfg(test)]
                    measurement::drained();
                    run.extend_from_slice(&page);
                    next += 1;
                }
            }
            let length = (size - offset).min(run.len() as u64) as usize;
            write(offset, &run[..length])?;
        }
        Ok(())
    }
}
fn zero_page() -> Result<Vec<u8>, Errno> {
    let mut page = Vec::new();
    page.try_reserve_exact(BLOCK_SIZE)
        .map_err(|_| Errno(ENOMEM))?;
    page.resize(BLOCK_SIZE, 0);
    Ok(page)
}
