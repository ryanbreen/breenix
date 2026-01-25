//! Virtqueue Implementation
//!
//! Implements the split virtqueue used by VirtIO legacy devices.
//!
//! # Memory Layout
//!
//! A virtqueue consists of three parts, all contiguous in physical memory:
//! 1. Descriptor table - array of VirtqDesc
//! 2. Available ring - guest writes, device reads
//! 3. Used ring - device writes, guest reads
//!
//! The queue address written to the device is the physical page number
//! of the start of this region.

use crate::memory::frame_allocator;
use core::sync::atomic::{fence, Ordering};
#[cfg(target_arch = "x86_64")]
use x86_64::structures::paging::PhysFrame;

/// Descriptor flags
pub mod desc_flags {
    /// Buffer continues via the next field
    pub const NEXT: u16 = 1;
    /// Buffer is write-only (for device)
    pub const WRITE: u16 = 2;
}

/// Virtqueue descriptor
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VirtqDesc {
    /// Physical address of the buffer
    pub addr: u64,
    /// Length of the buffer
    pub len: u32,
    /// Descriptor flags
    pub flags: u16,
    /// Index of next descriptor if NEXT flag is set
    pub next: u16,
}


/// Available ring structure
#[repr(C)]
pub struct VirtqAvail {
    /// Available ring flags
    pub flags: u16,
    /// Index where next available entry will be written
    pub idx: u16,
    /// Ring of descriptor indices (actual size is queue_size)
    pub ring: [u16; 256], // Max queue size
}

/// Used ring element
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VirtqUsedElem {
    /// Index of start of used descriptor chain
    pub id: u32,
    /// Total length of descriptor chain written
    pub len: u32,
}

/// Used ring structure
#[repr(C)]
pub struct VirtqUsed {
    /// Used ring flags
    pub flags: u16,
    /// Index where next used entry will be written by device
    pub idx: u16,
    /// Ring of used elements (actual size is queue_size)
    pub ring: [VirtqUsedElem; 256], // Max queue size
}

/// A virtqueue for device I/O
pub struct Virtqueue {
    /// Pointer to descriptor table
    desc: *mut VirtqDesc,
    /// Pointer to available ring
    avail: *mut VirtqAvail,
    /// Pointer to used ring
    used: *mut VirtqUsed,
    /// Physical address of the queue (for device configuration)
    phys_addr: u64,
    /// Head of free descriptor chain
    free_head: u16,
    /// Number of free descriptors
    num_free: u16,
    /// Last seen used index
    last_used_idx: u16,
    /// Queue size (power of 2)
    queue_size: u16,
    /// Allocated physical frames (stored for future deallocation)
    #[allow(dead_code)] // Stored for eventual Drop implementation
    frames: [Option<PhysFrame>; 4],
    /// Number of allocated frames
    #[allow(dead_code)] // Stored for eventual Drop implementation
    num_frames: usize,
}

impl Virtqueue {
    /// Calculate required pages for a queue of given size
    fn required_pages(queue_size: u16) -> usize {
        // Descriptor table size: queue_size * 16 bytes
        let desc_size = (queue_size as usize) * 16;
        // Available ring size: 4 + 2 * queue_size + 2 (for used_event)
        let avail_size = 6 + 2 * (queue_size as usize);
        // Used ring size: 4 + 8 * queue_size + 2 (for avail_event)
        let used_size = 6 + 8 * (queue_size as usize);

        // Available ring must be aligned to 2 bytes (naturally aligned)
        // Used ring must be aligned to 4 bytes
        let desc_avail_size = desc_size + avail_size;
        // Align used ring to 4KB boundary for simplicity
        let total_size = desc_avail_size + 4096 + used_size;

        // Round up to pages
        (total_size + 4095) / 4096
    }

    /// Allocate and initialize a new virtqueue
    ///
    /// # Arguments
    /// * `queue_size` - Size of the queue (must be power of 2, max 256)
    /// * `phys_offset` - Physical memory offset for virtual address calculation
    ///
    /// # Returns
    /// Tuple of (Virtqueue, physical_address) on success
    pub fn new(queue_size: u16) -> Result<Self, &'static str> {
        if queue_size == 0 || queue_size > 256 || !queue_size.is_power_of_two() {
            return Err("Invalid queue size");
        }

        let num_pages = Self::required_pages(queue_size);
        if num_pages > 4 {
            return Err("Queue requires too many pages");
        }

        // Allocate contiguous physical frames
        // IMPORTANT: VirtIO legacy requires physically contiguous memory for the queue
        let mut frames: [Option<PhysFrame>; 4] = [None; 4];
        let mut base_phys: Option<u64> = None;
        let mut prev_phys: Option<u64> = None;

        for (i, frame_slot) in frames.iter_mut().take(num_pages).enumerate() {
            let frame = frame_allocator::allocate_frame()
                .ok_or("Failed to allocate frame for virtqueue")?;

            let frame_phys = frame.start_address().as_u64();

            if i == 0 {
                base_phys = Some(frame_phys);
            } else if let Some(prev) = prev_phys {
                // Verify frames are contiguous
                if frame_phys != prev + 4096 {
                    log::error!(
                        "VirtIO queue: Non-contiguous frames allocated! prev={:#x}, curr={:#x}",
                        prev, frame_phys
                    );
                    // Continue anyway - early boot allocations are usually contiguous
                    // but log the warning for debugging
                }
            }

            prev_phys = Some(frame_phys);
            *frame_slot = Some(frame);
        }

        let phys_addr = base_phys.ok_or("No frames allocated")?;

        log::debug!(
            "VirtIO queue: Allocated {} pages starting at phys={:#x}",
            num_pages, phys_addr
        );

        // Get virtual addresses using physical memory offset
        let phys_offset = crate::memory::physical_memory_offset();
        let virt_base = phys_addr + phys_offset.as_u64();

        // Calculate layout offsets per VirtIO legacy spec
        // QEMU calculates: avail = desc + queue_size * 16
        //                  used = ALIGN(avail + 4 + queue_size * 2, 4096)
        // Note: The 4 bytes are for flags(2) + idx(2). The used_event field
        // comes AFTER ring[queue_size], so we don't include it in alignment calc.
        let desc_size = (queue_size as usize) * 16;
        let avail_offset = desc_size;
        // avail_size for alignment: flags(2) + idx(2) + ring[queue_size](2*n)
        // NOT including used_event which comes after
        let avail_size_for_align = 4 + 2 * (queue_size as usize);
        // Used ring starts at next 4096-byte boundary after avail (per VIRTIO_PCI_VRING_ALIGN)
        let used_offset = ((avail_offset + avail_size_for_align + 4095) / 4096) * 4096;

        // Get pointers
        let desc = virt_base as *mut VirtqDesc;
        let avail = (virt_base + avail_offset as u64) as *mut VirtqAvail;
        let used = (virt_base + used_offset as u64) as *mut VirtqUsed;

        // Zero the memory
        // Full avail size includes used_event at the end: flags(2) + idx(2) + ring(2*n) + used_event(2)
        let avail_size_full = 6 + 2 * (queue_size as usize);
        // Full used size includes avail_event at the end: flags(2) + idx(2) + ring(8*n) + avail_event(2)
        let used_size_full = 6 + 8 * (queue_size as usize);
        unsafe {
            core::ptr::write_bytes(desc, 0, queue_size as usize);
            core::ptr::write_bytes(avail as *mut u8, 0, avail_size_full);
            core::ptr::write_bytes(used as *mut u8, 0, used_size_full);
        }

        // Initialize descriptor free list
        unsafe {
            for i in 0..(queue_size - 1) {
                (*desc.add(i as usize)).next = i + 1;
            }
            (*desc.add((queue_size - 1) as usize)).next = 0;
        }

        log::debug!(
            "VirtIO queue: Layout - desc_offset=0, avail_offset={}, used_offset={}",
            avail_offset, used_offset
        );
        log::debug!(
            "VirtIO queue: Pointers - desc={:p}, avail={:p}, used={:p}",
            desc, avail, used
        );

        Ok(Virtqueue {
            desc,
            avail,
            used,
            phys_addr,
            free_head: 0,
            num_free: queue_size,
            last_used_idx: 0,
            queue_size,
            frames,
            num_frames: num_pages,
        })
    }

    /// Get the physical address of the queue (for device configuration)
    pub fn phys_addr(&self) -> u64 {
        self.phys_addr
    }

    /// Get the queue size
    #[allow(dead_code)] // Part of public virtqueue API
    pub fn queue_size(&self) -> u16 {
        self.queue_size
    }

    /// Allocate a descriptor from the free list
    fn alloc_desc(&mut self) -> Option<u16> {
        if self.num_free == 0 {
            return None;
        }

        let idx = self.free_head;
        unsafe {
            self.free_head = (*self.desc.add(idx as usize)).next;
        }
        self.num_free -= 1;
        Some(idx)
    }

    /// Free a descriptor back to the free list
    fn free_desc(&mut self, idx: u16) {
        unsafe {
            (*self.desc.add(idx as usize)).next = self.free_head;
        }
        self.free_head = idx;
        self.num_free += 1;
    }

    /// Free a descriptor chain starting at the given index
    pub fn free_chain(&mut self, mut head: u16) {
        loop {
            let desc = unsafe { &*self.desc.add(head as usize) };
            let flags = desc.flags;
            let next = desc.next;

            self.free_desc(head);

            if flags & desc_flags::NEXT == 0 {
                break;
            }
            head = next;
        }
    }

    /// Add a buffer to the queue
    ///
    /// # Arguments
    /// * `phys_addr` - Physical address of the buffer
    /// * `len` - Length of the buffer
    /// * `device_writable` - If true, buffer is for device to write (read request)
    ///
    /// # Returns
    /// The descriptor index (for tracking), or None if queue is full
    #[allow(dead_code)] // Part of public virtqueue API
    pub fn add_buf(
        &mut self,
        phys_addr: u64,
        len: u32,
        device_writable: bool,
    ) -> Option<u16> {
        let idx = self.alloc_desc()?;

        // Set up descriptor using volatile writes for device-shared memory
        unsafe {
            let desc = self.desc.add(idx as usize);
            let flags = if device_writable { desc_flags::WRITE } else { 0 };
            core::ptr::write_volatile(&mut (*desc).addr, phys_addr);
            core::ptr::write_volatile(&mut (*desc).len, len);
            core::ptr::write_volatile(&mut (*desc).flags, flags);
            core::ptr::write_volatile(&mut (*desc).next, 0);
        }

        // Memory fence before updating avail ring
        fence(Ordering::SeqCst);

        // Add to available ring using volatile writes
        unsafe {
            let avail_idx = core::ptr::read_volatile(&(*self.avail).idx);
            let ring_idx = avail_idx as usize % self.queue_size as usize;
            core::ptr::write_volatile(&mut (*self.avail).ring[ring_idx], idx);
            fence(Ordering::SeqCst);
            core::ptr::write_volatile(&mut (*self.avail).idx, avail_idx.wrapping_add(1));
        }

        Some(idx)
    }

    /// Add a chained buffer (multiple descriptors)
    ///
    /// Used for requests that need header + data + status buffers.
    ///
    /// # Arguments
    /// * `buffers` - Slice of (phys_addr, len, device_writable) tuples
    ///
    /// # Returns
    /// The head descriptor index, or None if not enough descriptors
    pub fn add_chain(
        &mut self,
        buffers: &[(u64, u32, bool)],
    ) -> Option<u16> {
        if buffers.is_empty() || buffers.len() > self.num_free as usize {
            return None;
        }

        let mut head: Option<u16> = None;
        let mut prev_idx: Option<u16> = None;

        for (i, &(phys_addr, len, device_writable)) in buffers.iter().enumerate() {
            let idx = self.alloc_desc()?;

            if head.is_none() {
                head = Some(idx);
            }

            // Link previous descriptor to this one
            if let Some(prev) = prev_idx {
                unsafe {
                    let prev_desc = self.desc.add(prev as usize);
                    // Use volatile writes for device-shared memory
                    core::ptr::write_volatile(&mut (*prev_desc).next, idx);
                    let old_flags = core::ptr::read_volatile(&(*prev_desc).flags);
                    core::ptr::write_volatile(&mut (*prev_desc).flags, old_flags | desc_flags::NEXT);
                }
            }

            // Set up this descriptor using volatile writes
            unsafe {
                let desc = self.desc.add(idx as usize);
                let flags = if device_writable { desc_flags::WRITE } else { 0 }
                    | if i < buffers.len() - 1 { desc_flags::NEXT } else { 0 };

                // Write all descriptor fields using volatile
                core::ptr::write_volatile(&mut (*desc).addr, phys_addr);
                core::ptr::write_volatile(&mut (*desc).len, len);
                core::ptr::write_volatile(&mut (*desc).flags, flags);
                core::ptr::write_volatile(&mut (*desc).next, 0);
            }

            prev_idx = Some(idx);
        }

        // Memory fence: ensure all descriptor writes are visible before updating avail ring
        fence(Ordering::SeqCst);

        // Add head to available ring
        if let Some(head_idx) = head {
            unsafe {
                // Must use volatile for all shared memory with device
                let avail_idx = core::ptr::read_volatile(&(*self.avail).idx);
                let ring_idx = avail_idx as usize % self.queue_size as usize;

                // Write the descriptor index to the ring
                core::ptr::write_volatile(&mut (*self.avail).ring[ring_idx], head_idx);
                fence(Ordering::SeqCst);

                // Update the avail.idx to make it visible to the device
                core::ptr::write_volatile(&mut (*self.avail).idx, avail_idx.wrapping_add(1));
                fence(Ordering::SeqCst);
            }
        }

        head
    }

    /// Check if there are completed buffers in the used ring
    pub fn has_used(&self) -> bool {
        // Memory fence to ensure we see device writes
        fence(Ordering::SeqCst);
        // Must use volatile read - device writes to this memory
        let used_idx = unsafe { core::ptr::read_volatile(&(*self.used).idx) };
        used_idx != self.last_used_idx
    }

    /// Get the next completed buffer from the used ring
    ///
    /// # Returns
    /// Some((descriptor_head, bytes_written)) if a buffer is available, None otherwise
    pub fn get_used(&mut self) -> Option<(u16, u32)> {
        fence(Ordering::SeqCst);

        unsafe {
            // Must use volatile reads - device writes to this memory
            let used_idx = core::ptr::read_volatile(&(*self.used).idx);
            if used_idx == self.last_used_idx {
                return None;
            }

            let ring_idx = self.last_used_idx as usize % self.queue_size as usize;
            let elem_ptr = &(*self.used).ring[ring_idx];
            let id = core::ptr::read_volatile(&elem_ptr.id) as u16;
            let len = core::ptr::read_volatile(&elem_ptr.len);

            self.last_used_idx = self.last_used_idx.wrapping_add(1);

            Some((id, len))
        }
    }

    /// Get number of free descriptors
    #[allow(dead_code)] // Part of public virtqueue API
    pub fn num_free(&self) -> u16 {
        self.num_free
    }

    /// Debug: get current used ring idx from device
    pub fn debug_used_idx(&self) -> u16 {
        fence(Ordering::SeqCst);
        unsafe { core::ptr::read_volatile(&(*self.used).idx) }
    }

    /// Debug: get our last_used_idx
    pub fn debug_last_used_idx(&self) -> u16 {
        self.last_used_idx
    }

    /// Debug: dump descriptor chain starting at head
    #[allow(dead_code)] // Debug utility for diagnosing virtqueue issues
    pub fn debug_dump_chain(&self, mut head: u16) {
        let mut i = 0;
        loop {
            // Use volatile reads to see what device would see
            unsafe {
                let desc = self.desc.add(head as usize);
                let addr = core::ptr::read_volatile(&(*desc).addr);
                let len = core::ptr::read_volatile(&(*desc).len);
                let flags = core::ptr::read_volatile(&(*desc).flags);
                let next = core::ptr::read_volatile(&(*desc).next);
                log::debug!(
                    "  desc[{}]: addr={:#x} len={} flags={:#x} next={}",
                    head, addr, len, flags, next
                );
                if flags & desc_flags::NEXT == 0 || i > 10 {
                    break;
                }
                head = next;
            }
            i += 1;
        }
        // Dump avail ring state using volatile reads
        unsafe {
            let flags = core::ptr::read_volatile(&(*self.avail).flags);
            let idx = core::ptr::read_volatile(&(*self.avail).idx);
            let ring0 = core::ptr::read_volatile(&(*self.avail).ring[0]);
            log::debug!(
                "  avail: flags={:#x} idx={} ring[0]={} queue_phys={:#x}",
                flags, idx, ring0, self.phys_addr
            );
            // Also show used ring state
            let used_flags = core::ptr::read_volatile(&(*self.used).flags);
            let used_idx = core::ptr::read_volatile(&(*self.used).idx);
            log::debug!(
                "  used: flags={:#x} idx={} last_used_idx={}",
                used_flags, used_idx, self.last_used_idx
            );
        }
    }
}

// Safety: The queue is accessed with proper synchronization
unsafe impl Send for Virtqueue {}
unsafe impl Sync for Virtqueue {}
