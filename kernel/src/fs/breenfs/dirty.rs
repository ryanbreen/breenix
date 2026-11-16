//! BreenFS-only dirty pages; the mount serializes this per-VNode state.
//! Clean pages belong to the buffer cache (E24 coordinator ledger line 2).
use super::format::BLOCK_SIZE;
use crate::{fs::errno::Errno, syscall::errno::ENOMEM};
use alloc::{collections::BTreeMap, vec::Vec};

#[derive(Default)]
pub struct DirtyPages {
    pages: BTreeMap<u64, Vec<u8>>,
}
impl DirtyPages {
    pub fn bytes(&self) -> u64 {
        self.pages.len() as u64 * BLOCK_SIZE as u64
    }
    pub fn get(&self, page: u64) -> Option<&[u8]> {
        self.pages.get(&page).map(Vec::as_slice)
    }
    pub fn indexes(&self) -> impl Iterator<Item = u64> + '_ {
        self.pages.keys().copied()
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
            prepared.insert(index, page);
            done += take;
        }
        self.pages.append(&mut prepared);
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
            Some(page)
        } else {
            None
        };
        self.pages.split_off(&size.div_ceil(BLOCK_SIZE as u64));
        if let Some(page) = tail {
            self.pages.insert(index, page);
        }
        Ok(())
    }
    // Keep the source pages until publication succeeds. Runs stop at a hole,
    // EOF or 256 blocks, matching the data REF limit in design section 2.4.
    pub fn write_runs(
        &self,
        size: u64,
        mut write: impl FnMut(u64, &[u8]) -> Result<(), Errno>,
    ) -> Result<(), Errno> {
        let mut pages = self.pages.iter().peekable();
        while let Some((&first, page)) = pages.next() {
            let offset = first * BLOCK_SIZE as u64;
            if offset >= size {
                break;
            }
            let mut run = Vec::new();
            run.try_reserve_exact(256 * BLOCK_SIZE)
                .map_err(|_| Errno(ENOMEM))?;
            run.extend_from_slice(page);
            let mut next = first + 1;
            while run.len() < 256 * BLOCK_SIZE {
                if !pages.peek().is_some_and(|(index, _)| **index == next) {
                    break;
                }
                if let Some((_, page)) = pages.next() {
                    run.extend_from_slice(page);
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
