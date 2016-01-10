use super::Page;

pub struct TemporaryPage {
    page: Page,
}

use super::{ActivePageTable, VirtualAddress};
use memory::Frame;

impl TemporaryPage {
    /// Maps the temporary page to the given frame in the active table.
    /// Returns the start address of the temporary page.
    pub fn map(&mut self, frame: Frame, active_table: &mut ActivePageTable)
        -> VirtualAddress
    {
        use super::entry::WRITABLE;

        assert!(active_table.translate_page(self.page).is_none(),
                "temporary page is already mapped");
        active_table.map_to(self.page, frame, WRITABLE, ???);
        self.page.start_address()
    }

    /// Unmaps the temporary page in the active table.
    pub fn unmap(&mut self, active_table: &mut ActivePageTable) {
        active_table.unmap(self.page, ???)
    }
}