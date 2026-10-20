//! Explicit 64-bit page-number trees for guest-backed objects.

use alloc::sync::Arc;
use super::wire;
use crate::gpu::dma::{Pages, PAGE};

/// A tree owns its page lists and retains the data they address. The device sees IPA
/// page numbers, including on firmware boots where a kernel VA is not its device address.
pub struct PageTree {
    pub data: Arc<Pages>,
    tables: Pages,
    depth: u32,
    root: usize,
}

impl PageTree {
    pub fn table_pages(count: usize) -> usize {
        let leaves = count.div_ceil(PAGE / 8);
        leaves + usize::from(leaves > 1)
    }

    pub fn new(data: Arc<Pages>) -> Option<Self> {
        let count = data.count();
        let per_page = PAGE / 8;
        if count == 0 || count > per_page * per_page {
            return None;
        }
        let leaves = count.div_ceil(per_page);
        let root = if leaves == 1 { 0 } else { leaves };
        let tables = Pages::new(Self::table_pages(count))?;
        for i in 0..count {
            tables.write(i * 8, &(data.page_addr(i) >> 12).to_le_bytes());
        }
        if leaves > 1 {
            for i in 0..leaves {
                tables.write(root * PAGE + i * 8, &(tables.page_addr(i) >> 12).to_le_bytes());
            }
        }
        tables.maintain(0, tables.bytes(), false);
        data.maintain(0, data.bytes(), false);
        Some(Self { data, tables, depth: if leaves == 1 { wire::PT64_1 } else { wire::PT64_2 }, root })
    }

    pub fn depth(&self) -> u32 { self.depth }
    pub fn root_ppn(&self) -> u64 { self.tables.page_addr(self.root) >> 12 }
}
