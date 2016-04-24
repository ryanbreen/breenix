use memory::Frame;
use memory::frame_allocator::FrameAllocator;
use memory::paging;
use memory::paging::{Page,ActivePageTable};
use multiboot2::{MemoryAreaIter, MemoryArea};

pub struct AreaFrameAllocator {
  next_free_frame: Frame,
  current_area: Option<&'static MemoryArea>,
  areas: MemoryAreaIter,
  kernel_start: Frame,
  kernel_end: Frame,
  multiboot_start: Frame,
  multiboot_end: Frame,
  allocated_frame_count: usize,
}

unsafe impl Sync for AreaFrameAllocator {}

impl AreaFrameAllocator {

  pub fn new(kernel_start: usize, kernel_end: usize,
        multiboot_start: usize, multiboot_end: usize,
        memory_areas: MemoryAreaIter) -> AreaFrameAllocator
  {
    let mut allocator = AreaFrameAllocator {
      next_free_frame: Frame::containing_address(0),
      current_area: None,
      areas: memory_areas,
      kernel_start: Frame::containing_address(kernel_start),
      kernel_end: Frame::containing_address(kernel_end),
      multiboot_start: Frame::containing_address(multiboot_start),
      multiboot_end: Frame::containing_address(multiboot_end),
      allocated_frame_count: 0,
    };
    allocator.choose_next_area();
    allocator
  }

  fn choose_next_area(&mut self) {
    self.current_area = self.areas.clone().filter(|area| {
      let address = area.base_addr + area.length - 1;
      Frame::containing_address(address as usize) >= self.next_free_frame
    }).min_by_key(|area| area.base_addr);

    if let Some(area) = self.current_area {
      let start_frame = Frame::containing_address(area.base_addr as usize);
      if self.next_free_frame < start_frame {
        self.next_free_frame = start_frame;
      }
    }
  }

  pub fn allocated_frame_count(&self) -> usize {
    self.allocated_frame_count
  }
}

impl FrameAllocator for AreaFrameAllocator {
  fn allocate_frame(&mut self) -> Option<Frame> {

    //println!("am i even me? {:x}", &self as *const _ as u64);
    //println!("time to get a frame from area {} {:x}", self.allocated_frame_count, &self.current_area as *const _ as u64);
    if let Some(area) = self.current_area {
      // "clone" the frame to return it if it's free. Frame doesn't
      // implement Clone, but we can construct an identical frame.
      let frame = Frame{ number: self.next_free_frame.number };

      // the last frame of the current area
      let current_area_last_frame = {
        let address = area.base_addr + area.length - 1;

        Frame::containing_address(address as usize)
      };

      if frame > current_area_last_frame {
        // all frames of current area are used, switch to next area
        self.choose_next_area();
      } else if frame >= self.kernel_start && frame <= self.kernel_end {
        // `frame` is used by the kernel
        self.next_free_frame = Frame{ number: self.kernel_end.number + 1 };
      } else if frame >= self.multiboot_start && frame <= self.multiboot_end {
        // `frame` is used by the multiboot information structure
        self.next_free_frame = Frame{ number: self.multiboot_end.number + 1 };
      } else {
        // frame is unused, increment `next_free_frame` and return it
        self.next_free_frame.number += 1;
        self.allocated_frame_count += 1;

        return Some(frame);
      }
      // `frame` was not valid, try it again with the updated `next_free_frame`
      self.allocate_frame()
    } else {
      println!("No free frames left!");
      None // no free frames left
    }
  }

  fn deallocate_frame(&mut self, _frame: Frame) {
    
  }
}

