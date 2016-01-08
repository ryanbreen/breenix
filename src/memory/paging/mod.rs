use memory::PAGE_SIZE; // needed later

mod entry;
mod table;

pub use self::entry::*;
use self::table::{Table, Level4};
use core::ptr::Unique;
use memory::FrameAllocator;
use memory::Frame;

const ENTRY_COUNT: usize = 512;

pub type PhysicalAddress = usize;
pub type VirtualAddress = usize;

#[derive(Debug, Clone, Copy)]
pub struct Page {
   number: usize,
}

impl Page {

  fn containing_address(address: VirtualAddress) -> Page {
    assert!(address < 0x0000_8000_0000_0000 || address >= 0xffff_8000_0000_0000,
        "invalid address: 0x{:x}", address);
    Page { number: address / PAGE_SIZE }
  }

  fn start_address(&self) -> usize {
    self.number * PAGE_SIZE
  }

  fn p4_index(&self) -> usize {
      (self.number >> 27) & 0o777
  }
  fn p3_index(&self) -> usize {
      (self.number >> 18) & 0o777
  }
  fn p2_index(&self) -> usize {
      (self.number >> 9) & 0o777
  }
  fn p1_index(&self) -> usize {
      (self.number >> 0) & 0o777
  }
}

pub struct InactivePageTable {
    p4_frame: Frame,
}

impl InactivePageTable {
    pub fn new(frame: Frame) -> InactivePageTable {
        // TODO zero and recursive map the frame
        InactivePageTable { p4_frame: frame }
    }
}

pub struct RecursivePageTable {
  p4: Unique<Table<Level4>>,
}

impl RecursivePageTable {
  pub unsafe fn new() -> RecursivePageTable {
    RecursivePageTable { p4: Unique::new(table::P4) }
  }

  fn p4(&self) -> &Table<Level4> {
    unsafe { self.p4.get() }
  }

  fn p4_mut(&mut self) -> &mut Table<Level4> {
    unsafe { self.p4.get_mut() }
  }

  pub fn translate(&self, virtual_address: VirtualAddress) -> Option<PhysicalAddress> {
    let offset = virtual_address % PAGE_SIZE;
    self.translate_page(Page::containing_address(virtual_address))
        .map(|frame| frame.number * PAGE_SIZE + offset)
  }

  fn translate_page(&self, page: Page) -> Option<Frame> {
    let p3 = self.p4().next_table(page.p4_index());

    let huge_page = || {
        p3.and_then(|p3| {
            let p3_entry = &p3[page.p3_index()];
            // 1GiB page?
            if let Some(start_frame) = p3_entry.pointed_frame() {
                if p3_entry.flags().contains(HUGE_PAGE) {
                    // address must be 1GiB aligned
                    assert!(start_frame.number % (ENTRY_COUNT * ENTRY_COUNT) == 0);
                    return Some(Frame {
                        number: start_frame.number + page.p2_index() * ENTRY_COUNT +
                                page.p1_index(),
                    });
                }
            }
            if let Some(p2) = p3.next_table(page.p3_index()) {
                let p2_entry = &p2[page.p2_index()];
                // 2MiB page?
                if let Some(start_frame) = p2_entry.pointed_frame() {
                    if p2_entry.flags().contains(HUGE_PAGE) {
                        // address must be 2MiB aligned
                        assert!(start_frame.number % ENTRY_COUNT == 0);
                        return Some(Frame { number: start_frame.number + page.p1_index() });
                    }
                }
            }
            None
        })
    };

    p3.and_then(|p3| p3.next_table(page.p3_index()))
      .and_then(|p2| p2.next_table(page.p2_index()))
      .and_then(|p1| p1[page.p1_index()].pointed_frame())
      .or_else(huge_page)
  }

  pub fn map_to<A>(&mut self, page: Page, frame: Frame, flags: EntryFlags, allocator: &mut A)
        where A: FrameAllocator
  {
      let mut p3 = self.p4_mut().next_table_create(page.p4_index(), allocator);
      let mut p2 = p3.next_table_create(page.p3_index(), allocator);
      let mut p1 = p2.next_table_create(page.p2_index(), allocator);

      assert!(p1[page.p1_index()].is_unused());
      p1[page.p1_index()].set(frame, flags | PRESENT);
  }

  pub fn map<A>(&mut self, page: Page, flags: EntryFlags, allocator: &mut A)
      where A: FrameAllocator
  {
      let frame = allocator.allocate_frame().expect("out of memory");
      self.map_to(page, frame, flags, allocator)
  }

  pub fn identity_map<A>(&mut self,
                       frame: Frame,
                       flags: EntryFlags,
                       allocator: &mut A)
    where A: FrameAllocator
  {
    let page = Page::containing_address(frame.start_address());
    self.map_to(page, frame, flags, allocator)
  }

  fn unmap<A>(&mut self, page: Page, allocator: &mut A)
    where A: FrameAllocator
  {
    assert!(self.translate(page.start_address()).is_some());

    let p1 = self.p4_mut()
                 .next_table_mut(page.p4_index())
                 .and_then(|p3| p3.next_table_mut(page.p3_index()))
                 .and_then(|p2| p2.next_table_mut(page.p2_index()))
                 .expect("mapping code does not support huge pages");
    let frame = p1[page.p1_index()].pointed_frame().unwrap();
    p1[page.p1_index()].set_unused();
    unsafe {
        ::x86::tlb::flush(page.start_address());
    }
    // TODO free p(1,2,3) table if empty
    allocator.deallocate_frame(frame);
  }

}

pub fn test_paging<A>(allocator: &mut A)
    where A: FrameAllocator
{
    let mut page_table = unsafe { RecursivePageTable::new() };

    // address 0 is mapped
    println!("Some = {:?}", page_table.translate(0));
     // second P1 entry
    println!("Some = {:?}", page_table.translate(4096));
    // second P2 entry
    println!("Some = {:?}", page_table.translate(512 * 4096));
    // 300th P2 entry
    println!("Some = {:?}", page_table.translate(300 * 512 * 4096));
    // second P3 entry
    println!("None = {:?}", page_table.translate(512 * 512 * 4096));
    // last mapped byte
    println!("Some = {:?}", page_table.translate(512 * 512 * 4096 - 1));

    let addr = 42 * 512 * 512 * 4096; // 42th P3 entry
    let page = Page::containing_address(addr);
    let frame = allocator.allocate_frame().expect("no more frames");
    println!("None = {:?}, map to {:?}",
             page_table.translate(addr),
             frame);
    page_table.map_to(page, frame, EntryFlags::empty(), allocator);
    println!("Some = {:?}", page_table.translate(addr));
    println!("next free frame: {:?}", allocator.allocate_frame());

    println!("{:#x}", unsafe {
        *(Page::containing_address(addr).start_address() as *const u64)
    });

    page_table.unmap(Page::containing_address(addr), allocator);
    println!("None = {:?}", page_table.translate(addr));

    // Uncomment the below to demonstrate a page fault for accessing free memory.
/*
    println!("{:#x}", unsafe {
        *(Page::containing_address(addr).start_address() as *const u64)
    });
*/
}