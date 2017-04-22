#![allow(dead_code)]

use alloc::boxed::Box;
use collections::VecDeque;

use core::fmt;
use core::mem;

use memory::{Frame, FrameAllocator, area_frame_allocator, page_table};
use memory::paging;
use memory::paging::Page;

use spin::Mutex;

use tiered_allocator;

#[cfg(target_arch="x86_64")]
const CACHE_LINE_SIZE: usize = 64;

#[cfg(target_arch="x86_64")]
const BASE_PAGE_SIZE: usize = 4096;

const MAX_SLABS: usize = 14;

const VIRT_START: usize = tiered_allocator::HEAP_START + tiered_allocator::HEAP_SIZE;

static mut VIRT_OFFSET: usize = 0;

#[cfg(target_arch="x86_64")]
type VAddr = usize;

static mut ZONE_ALLOCATOR_PTR: Option<Mutex<&'static mut ZoneAllocator>> = None;

pub fn zone_allocator() -> &'static Mutex<&'static mut ZoneAllocator> {
    unsafe {
        match ZONE_ALLOCATOR_PTR {
            Some(ref za) => za,
            None => {
                panic!("zone_allocator called before init");
            }
        }
    }
}

pub fn init() {
    unsafe {
        ZONE_ALLOCATOR_PTR = Some(Mutex::new(&mut *Box::into_raw(Box::new(ZoneAllocator::new()))));
    }
}

pub fn allocate(size: usize, align: usize) -> *mut u8 {
    // Use the static zone allocator to find this.
    // Note: since we lock here and the lock is not reentrant, we must make sure that no allocations
    // happen from inside the allocator or that they occur in the bootstrap allocator.
    let rvalue: *mut u8 = zone_allocator().lock().allocate(size, align).expect("OOM");
    rvalue
}

pub struct AreaFrameSlabPageProvider {}

impl AreaFrameSlabPageProvider {
    fn allocate_slabpage(&mut self, size: usize) -> Option<SlabPage> {

        let allocator = area_frame_allocator();

        let mut frames_per_slabpage = 1;
        if size > BASE_PAGE_SIZE {
            frames_per_slabpage = size / BASE_PAGE_SIZE;
        }

        unsafe {
            let start_page_address: VAddr = VIRT_START + (BASE_PAGE_SIZE * VIRT_OFFSET);

            println!("Allocating slabpage {:x}", VIRT_START);

            for i in 0..frames_per_slabpage {
                let frame: Option<Frame> = allocator.allocate_frame();
                match frame {
                    None => return None,
                    Some(f) => {
                        let page = Page::containing_address(start_page_address +
                                                            (BASE_PAGE_SIZE * i));
                        page_table().map_to(page, f.clone(), paging::WRITABLE, allocator);
                    }
                }

                VIRT_OFFSET += 1;
            }

            let slab_page: SlabPage = SlabPage {
                start_page_address: start_page_address as u64,
                size: size as u64,
                bitfield: [0; CACHE_LINE_SIZE - 16],
            };
            return Some(slab_page);
        }
    }

    #[allow(unused_variables)]
    fn release_slabpage(&mut self, page: &mut SlabPage) {
        println!("Trying to release page");
        // TODO: Let's maybe release memory at some point.
    }
}

/// A zone allocator.
///
/// Has a bunch of slab allocators and can serve
/// allocation requests for many different (MAX_SLABS) object sizes
/// (by selecting the right slab allocator).
pub struct ZoneAllocator {
    pager: AreaFrameSlabPageProvider,
    slabs: [SlabAllocator; MAX_SLABS],
}

impl fmt::Debug for ZoneAllocator {
    #[allow(unused_must_use)]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ZoneAllocator slab_count: {}\n", self.slabs.len());

        for slab_allocator in self.slabs.into_iter() {
            write!(f, "{:?}", slab_allocator);
        }

        Ok(())
    }
}

impl ZoneAllocator {
    pub fn new() -> ZoneAllocator {
        ZoneAllocator {
            pager: AreaFrameSlabPageProvider {},
            slabs: [SlabAllocator::new(8),
                    SlabAllocator::new(16),
                    SlabAllocator::new(32),
                    SlabAllocator::new(64),
                    SlabAllocator::new(128),
                    SlabAllocator::new(256),
                    SlabAllocator::new(512),
                    SlabAllocator::new(1024),
                    SlabAllocator::new(2048),
                    SlabAllocator::new(4096),
                    SlabAllocator::new(8192),
                    SlabAllocator::new(16384),
                    SlabAllocator::new(65536),
                    SlabAllocator::new(131072)],
        }
    }

    /// Return maximum size an object of size `current_size` can use.
    ///
    /// Used to optimize `realloc`.
    fn get_max_size(current_size: usize) -> Option<usize> {
        match current_size {
            0...8 => Some(8),
            9...16 => Some(16),
            17...32 => Some(32),
            33...64 => Some(64),
            65...128 => Some(128),
            129...256 => Some(256),
            257...512 => Some(512),
            513...1024 => Some(1024),
            1025...2048 => Some(2048),
            2049...4096 => Some(4096),
            4097...8192 => Some(8192),
            8193...16384 => Some(16384),
            16385...65536 => Some(65536),
            65537...131072 => Some(131072),
            _ => None,
        }
    }

    /// Figure out index into zone array to get the correct slab allocator for that size.
    fn get_slab_idx(requested_size: usize) -> Option<usize> {
        match requested_size {
            0...8 => Some(0),
            9...16 => Some(1),
            17...32 => Some(2),
            33...64 => Some(3),
            65...128 => Some(4),
            129...256 => Some(5),
            257...512 => Some(6),
            513...1024 => Some(7),
            1025...2048 => Some(8),
            2049...4096 => Some(9),
            4097...8192 => Some(10),
            8192...16384 => Some(11),
            16384...65536 => Some(12),
            65536...131072 => Some(13),
            _ => None,
        }
    }

    /// Tries to locate a slab allocator.
    ///
    /// Returns either a index into the slab array or None in case
    /// the requested allocation size can not be satisfied by
    /// any of the available slabs.
    fn try_acquire_slab(&mut self, size: usize) -> Option<usize> {
        ZoneAllocator::get_slab_idx(size).map(|idx| {
            if self.slabs[idx].size == 0 {
                self.slabs[idx].size = size;
            }
            idx
        })
    }

    /// Refills the SlabAllocator in slabs at `idx` with a SlabPage.
    ///
    /// # TODO
    ///  * Panics in case we're OOM (should probably return error).
    fn refill_slab_allocator<'b>(&'b mut self, idx: usize) {
        let frames_per_slabpage: usize = match idx {
            10 => 2,
            11 => 4,
            12 => 16,
            13 => 32,
            _ => 1,
        };
        match self.pager.allocate_slabpage(frames_per_slabpage) {
            Some(new_head) => {
                self.slabs[idx].insert_slab(new_head);
            }
            None => panic!("OOM refilling slab {}", idx),
        }
    }

    /// Allocate a pointer to a block of memory of size `size` with alignment `align`.
    ///
    /// Can return None in case the zone allocator can not satisfy the allocation
    /// of the requested size or if we do not have enough memory.
    /// In case we are out of memory we try to refill the slab using our local pager
    /// and re-try the allocation request once more before we give up.
    pub fn allocate<'b>(&'b mut self, size: usize, align: usize) -> Option<*mut u8> {

        match self.try_acquire_slab(size) {
            Some(idx) => {
                let mut p = self.slabs[idx].allocate(align);
                if p.is_none() {
                    self.refill_slab_allocator(idx);
                    p = self.slabs[idx].allocate(align);
                }
                return p;
            }
            None => {
                println!("Failed to acquire slab of size {}", size);
                return None;
            }
        }
    }

    /// Deallocates a pointer to a block of memory previously allocated by `allocate`.
    ///
    /// # Arguments
    ///  * `ptr` - Address of the memory location to free.
    ///  * `old_size` - Size of the block.
    ///  * `align` - Alignment of the block.
    ///
    #[allow(unused_variables)]
    pub fn deallocate<'b>(&'b mut self, ptr: *mut u8, old_size: usize, align: usize) {
        match self.try_acquire_slab(old_size) {
            Some(idx) => self.slabs[idx].deallocate(ptr),
            None => {
                panic!("Unable to find slab allocator for size ({}) with ptr {:?}.",
                       old_size,
                       ptr)
            }
        }
    }

    unsafe fn copy(dest: *mut u8, src: *const u8, n: usize) {
        let mut i = 0;
        while i < n {
            *dest.offset(i as isize) = *src.offset(i as isize);
            i += 1;
        }
    }

    pub fn reallocate<'b>(&'b mut self,
                          ptr: *mut u8,
                          old_size: usize,
                          size: usize,
                          align: usize)
                          -> Option<*mut u8> {
        // Return immediately in case we can still fit the new request in the current buffer
        match ZoneAllocator::get_max_size(old_size) {
            Some(max_size) => {
                if max_size >= size {
                    return Some(ptr);
                }
                ()
            }
            None => (),
        };

        // Otherwise allocate, copy, free:
        self.allocate(size, align).map(|new| {
            unsafe {
                ZoneAllocator::copy(new, ptr, old_size);
            }
            self.deallocate(ptr, old_size, align);
            new
        })
    }
}

/// A slab allocator allocates elements of a fixed size.
///
/// It has a list of SlabPages stored inside `slabs` from which
/// it allocates memory.
pub struct SlabAllocator {
    /// Allocation size.
    size: usize,

    /// Memory backing store, to request new SlabPages.
    pager: AreaFrameSlabPageProvider,
    /// List of SlabPages.
    slabs: VecDeque<Option<SlabPage>>,
}

impl fmt::Debug for SlabAllocator {
    #[allow(unused_must_use)]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f,
               "   Slab Allocator allocation size: {}, allocated slabs: {}",
               self.size,
               self.slabs.len());

        if self.size < BASE_PAGE_SIZE {
            for slab in self.slabs.iter() {
                match slab {
                    &None => panic!("Invalid slab"),
                    &Some(ref s) => write!(f, "\n      {:?}", s),
                };
            }
        }

        write!(f, "\n")
    }
}

impl SlabAllocator {
    /// Create a new SlabAllocator.
    pub fn new(size: usize) -> SlabAllocator {
        SlabAllocator {
            size: size,
            pager: AreaFrameSlabPageProvider {},
            slabs: VecDeque::new(),
        }
    }

    /// Return object size of this allocator.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Try to allocate a new SlabPage and insert it.
    ///
    /// # TODO
    ///  * Amount is currently ignored.
    ///  * Panics on OOM (should return error!)
    #[allow(unused_variables)]
    fn refill_slab<'b>(&'b mut self, amount: usize) {

        match self.pager.allocate_slabpage(self.size) {
            Some(new_head) => {
                self.insert_slab(new_head);
            }
            None => panic!("OOM when allocating a new slab of size {}", self.size),
        }
    }

    /// Add a new SlabPage.
    pub fn insert_slab<'b>(&'b mut self, new_slab: SlabPage) {
        // This operation may attempt to allocate new memory on the heap because slabs is
        // a heap-allocated structure.
        self.slabs.push_front(Some(new_slab));
    }

    /// Tries to allocate a block of memory with respect to the `alignment`.
    ///
    /// Only searches within already allocated slab pages.
    fn allocate_in_existing_slabs<'b>(&'b mut self, alignment: usize) -> Option<*mut u8> {

        let size = self.size;

        for (_, slab_page) in self.slabs.iter_mut().enumerate() {

            match *slab_page {
                None => {
                    panic!("Invalid slab page");
                }
                Some(ref mut sp) => {
                    match sp.allocate(size, alignment) {
                        None => continue,
                        Some(obj) => {
                            return Some(obj as *mut u8);
                        }
                    }
                }
            }
        }

        None
    }

    /// Allocates a block of memory with respect to `alignment`.
    ///
    /// In case of failure will try to grow the slab allocator by requesting
    /// additional pages and re-try the allocation once more before we give up.
    pub fn allocate<'b>(&'b mut self, alignment: usize) -> Option<*mut u8> {

        // let size = self.size;
        // println!("Allocating {}", size);

        match self.allocate_in_existing_slabs(alignment) {
            None => {
                self.refill_slab(1);
                return self.allocate(alignment);
            }
            Some(obj) => return Some(obj),
        }
    }

    /// Deallocates a previously allocated block.
    ///
    /// # Bug
    /// This never releases memory in case the SlabPages are provided by the zone.
    pub fn deallocate<'b>(&'b mut self, ptr: *mut u8) {
        let page = (ptr as usize) & !(BASE_PAGE_SIZE - 1) as usize;
        let mut slab_page = unsafe { mem::transmute::<VAddr, &'static mut SlabPage>(page) };

        slab_page.deallocate(ptr, self.size);

        if slab_page.is_empty() {
            self.slabs.retain(|candidate| {
                match candidate {
                    &None => panic!("Invalid slab page"),
                    &Some(ref c) => {
                        return &slab_page.start_page_address as *const _ as u64 !=
                               &c.start_page_address as *const _ as u64
                    }
                };
            });
        }
    }
}

/// Holds a reference to allocated data.
///
/// Objects life within data and meta tracks the objects status.
/// Currently, `bitfield` and `id`
pub struct SlabPage {
    /// Pointer to page.
    start_page_address: u64,

    size: u64,

    /// A bit-field to track free/allocated memory within `data`.  This should pad otu the struct to
    /// exactly the cache line size of 64 bytes (64 - (64/8 + 64/8))
    bitfield: [u8; CACHE_LINE_SIZE - 16],
}

unsafe impl Send for SlabPage {}
unsafe impl Sync for SlabPage {}

impl fmt::Debug for SlabPage {
    #[allow(unused_must_use)]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} {:o} ", self.size, self.start_page_address);
        self.bitfield.iter().map(|b| write!(f, "{:02X}", b));
        Ok(())
    }
}

impl SlabPage {
    /// Tries to find a free block of memory that satisfies `alignment` requirement.
    ///
    /// # Notes
    /// * We pass size here to be able to calculate the resulting address within `data`.
    fn first_fit(&mut self, size: usize, alignment: usize) -> Option<(usize, usize)> {
        assert!(alignment.is_power_of_two());

        if self.size as usize >= BASE_PAGE_SIZE {
            // If this is a jumbo slab page, we just store a single value in the bitfield.
            match self.bitfield[0] {
                1 => return None,
                _ => {
                    self.bitfield[0] = 1;
                    return Some((0, self.start_page_address as usize));
                }
            };
        }

        for (base_idx, b) in self.bitfield.iter().enumerate() {
            for bit_idx in 0..8 {
                let idx: usize = base_idx * 8 + bit_idx;
                let offset = idx * size;

                let offset_inside_data_area =
                    offset <= (BASE_PAGE_SIZE as usize - CACHE_LINE_SIZE as usize - size);
                if !offset_inside_data_area {
                    return None;
                }

                let addr: usize = self.start_page_address as usize + offset;
                // let addr: usize = self.data as usize + offset;
                let alignment_ok = addr % alignment == 0;
                let block_is_free = b & (1 << bit_idx) == 0;

                if alignment_ok && block_is_free {
                    return Some((idx, addr));
                }
            }
        }
        None
    }

    /// Check if the current `idx` is allocated.
    ///
    /// # Notes
    /// In case `idx` is 3 and allocation size of slab is
    /// 8. The corresponding object would start at &data + 3 * 8.
    fn is_allocated(&self, idx: usize) -> bool {
        let base_idx = idx / 8;
        let bit_idx = idx % 8;

        (self.bitfield[base_idx] & (1 << bit_idx)) > 0
    }

    /// Sets the bit number `idx` in the bit-field.
    fn set_bit(&mut self, idx: usize) {
        let base_idx = idx / 8;
        let bit_idx = idx % 8;
        self.bitfield[base_idx] |= 1 << bit_idx;
    }

    /// Clears bit number `idx` in the bit-field.
    fn clear_bit(&mut self, idx: usize) {
        let base_idx = idx / 8;
        let bit_idx = idx % 8;
        self.bitfield[base_idx] &= !(1 << bit_idx);
    }

    /// Deallocates a memory object within this page.
    fn deallocate(&mut self, ptr: *mut u8, size: usize) {
        let page_offset = (ptr as usize) & 0xfff;
        assert!(page_offset % size == 0);
        let idx = page_offset / size;
        assert!(self.is_allocated(idx));

        self.clear_bit(idx);
    }

    /// Tries to allocate an object within this page.
    ///
    /// In case the Slab is full, returns None.
    fn allocate(&mut self, size: usize, alignment: usize) -> Option<*mut u8> {
        match self.first_fit(size, alignment) {
            Some((idx, addr)) => {
                self.set_bit(idx);
                // println!("base addr is {:o} for {} of {}", addr, size, self.size);
                Some(unsafe { mem::transmute::<usize, *mut u8>(addr) })
            }
            None => None,
        }
    }

    /// Checks if we can still allocate more objects within the page.
    fn is_full(&self) -> bool {
        self.bitfield.iter().filter(|&x| *x != 0xff).count() == 0
    }

    /// Checks if the page has currently no allocation.
    fn is_empty(&self) -> bool {
        self.bitfield.iter().filter(|&x| *x > 0x00).count() == 0
    }
}
