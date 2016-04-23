pub use self::area_frame_allocator::AreaFrameAllocator;
pub use self::paging::remap_the_kernel;

use self::paging::PhysicalAddress;
mod frame_allocator;
mod area_frame_allocator;
mod paging;

use multiboot2::BootInformation;

use slab_allocator;
use slab_allocator::SlabPageProvider;

use alloc::boxed::Box;

use self::frame_allocator::FrameAllocator;
use self::area_frame_allocator::AreaFrameSlabPageProvider;
use self::paging::Page;

pub const PAGE_SIZE: usize = 4096;

static mut AREA_FRAME_ALLOCATOR_PTR:Option<&'static mut AreaFrameAllocator> = None;

pub fn frame_allocator() -> &'static mut AreaFrameAllocator {
  unsafe {
    match AREA_FRAME_ALLOCATOR_PTR {
      Some(ref mut a) => a,
      None => { panic!("frame_allocator called before init"); },
    }
  }
}

pub fn init(boot_info: &BootInformation) {
  assert_has_not_been_called!("memory::init must be called only once");

  let memory_map_tag = boot_info.memory_map_tag().expect(
      "Memory map tag required");
  let elf_sections_tag = boot_info.elf_sections_tag().expect(
      "Elf sections tag required");

  let kernel_start = elf_sections_tag.sections()
      .filter(|s| s.is_allocated()).map(|s| s.addr).min().unwrap();
  let kernel_end = elf_sections_tag.sections()
      .filter(|s| s.is_allocated()).map(|s| s.addr + s.size).max()
      .unwrap();

  println!("kernel start: {:#x}, kernel end: {:#x}",
           kernel_start,
           kernel_end);
  println!("multiboot start: {:#x}, multiboot end: {:#x}",
           boot_info.start_address(),
           boot_info.end_address());

  unsafe {


    let mut allocator = AreaFrameAllocator::new(
        kernel_start as usize, kernel_end as usize,
        boot_info.start_address(), boot_info.end_address(),
        memory_map_tag.memory_areas());

    let mut active_table = paging::remap_the_kernel(&mut allocator, boot_info);

    use slab_allocator::{HEAP_START, HEAP_SIZE};

    let heap_start_page = Page::containing_address(HEAP_START);
    let heap_end_page = Page::containing_address(HEAP_START + HEAP_SIZE-1);

    for page in Page::range_inclusive(heap_start_page, heap_end_page) {
      active_table.map(page, paging::WRITABLE, &mut allocator);
    }

    AREA_FRAME_ALLOCATOR_PTR = Some(&mut *Box::into_raw(Box::new(allocator)));

    use self::paging::Page;
    // let mut alloc:&'static mut ZoneAllocator = &mut *(&mut ZoneAllocator::new(None) as *mut ZoneAllocator);
    let mut page_provider:&'static mut AreaFrameSlabPageProvider =
      &mut *(&mut AreaFrameSlabPageProvider::new(Some(frame_allocator()), active_table) as * mut AreaFrameSlabPageProvider);
    slab_allocator::init(Some(page_provider)); 
  }
  /*

  println!("We mapped {} frames", frame_allocator.allocated_frame_count());
  */
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Frame {
    number: usize,
}

impl Frame {
  fn containing_address(address: usize) -> Frame {
    Frame{ number: address / PAGE_SIZE }
  }

  fn start_address(&self) -> PhysicalAddress {
    self.number * PAGE_SIZE
  }

  fn clone(&self) -> Frame {
    Frame { number: self.number }
  }

  fn range_inclusive(start: Frame, end: Frame) -> FrameIter {
    FrameIter {
      start: start,
      end: end,
    }
  }
}

struct FrameIter {
    start: Frame,
    end: Frame,
}

impl Iterator for FrameIter {
    type Item = Frame;

    fn next(&mut self) -> Option<Frame> {
        if self.start <= self.end {
            let frame = self.start.clone();
            self.start.number += 1;
            Some(frame)
        } else {
            None
        }
    }
}

