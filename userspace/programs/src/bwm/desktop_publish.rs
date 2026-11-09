//! Reader-gated publication for the current compositor claim.
use libbreenix::{error::Error, window::{self, abi::{self, op}}};

/// Mappings belong to this compositor claim; external mappings count as readers.
pub(super) struct Publisher {
    page: abi::MapTree,
    readers: abi::MapTree,
    serializations: u64,
}
impl Publisher {
    pub(super) fn new() -> Result<Self, Error> {
        let mut page = abi::MapTree::default();
        window::call(op::MAP_TREE, &mut page)?;
        let readers = match window::map_tree_readers(0) {
            Ok(readers) => readers,
            Err(error) => {
                let _ = libbreenix::memory::munmap(page.addr as *mut u8, page.len as usize);
                return Err(error);
            }
        };
        Ok(Self { page, readers, serializations: 0 })
    }
    pub(super) fn publish(&mut self, windows: &[super::desk::Window], stack: &super::stack::Stack,
        screen: [i64; 2]) -> Result<(), String> {
        // Only this mapped atomic load is added to an unattached main-loop turn.
        let readers = unsafe { &*(self.readers.addr as *const abi::TreeReaderState) }
            .readers.load(core::sync::atomic::Ordering::Acquire);
        if readers == 0 { return Ok(()); }
        {
            let mut before = abi::CompositorStatus::default();
            window::call(op::COMPOSITOR_STATUS, &mut before).map_err(|e| e.to_string())?;
            // A registration may have arrived after this turn's discovery. Do not
            // stamp its scene version onto an incomplete local stack.
            let mut listed = Vec::new();
            super::session::list_windows(&mut listed).map_err(|e| e.to_string())?;
            if listed.len() != windows.len() || listed.iter().any(|info| {
                !windows.iter().any(|win| win.window_id == info.id
                    && win.owner_pid == info.owner_pid
                    && win.content_width() == info.width as usize
                    && win.content_height() == info.height as usize)
            }) { return Ok(()); }
            let tree = super::desktop_tree::build(stack, windows.iter().map(|win| {
                super::desktop_tree::WindowState { id: win.window_id,
                    title: core::str::from_utf8(&win.title[..win.title_len]).unwrap_or("�"),
                    owner: win.owner_pid, bounds: [i64::from(win.x), i64::from(win.y),
                        win.width as i64, win.height as i64], chromeless: win.chromeless }
            }), screen, before.scene_version);
            let mut after = abi::CompositorStatus::default();
            window::call(op::COMPOSITOR_STATUS, &mut after).map_err(|e| e.to_string())?;
            if before.claim_epoch != after.claim_epoch || before.scene_version != after.scene_version { return Ok(()); }
            let page = unsafe { breengel::tree_page::Page::from_mapping(self.page.addr as *mut u8, self.page.len as usize)? };
            // MAP_TREE id 0 is writable only by the claimant, on this thread.
            breengel::tree::serialize_for_commit(readers, &mut self.serializations, || unsafe { page.publish(&tree) })
        }
    }
}
impl Drop for Publisher {
    fn drop(&mut self) {
        for map in [self.page, self.readers] {
            let _ = libbreenix::memory::munmap(map.addr as *mut u8, map.len as usize);
        }
    }
}
