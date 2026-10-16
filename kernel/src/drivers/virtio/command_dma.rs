//! Stable request storage derived from VirtIO 1.3 CSD01, sections 2.7.5,
//! 2.7.6, 2.7.8, 2.7.13 and 2.7.14 (split-ring ownership), and 2.4 (reset).
//! https://docs.oasis-open.org/virtio/virtio/v1.3/csd01/virtio-v1.3-csd01.html
//!
//! Breenix policy (#1662): one allocation contains the command, optional
//! outgoing payload and response. Each starts at a 64-byte boundary matching
//! Breenix's cache-maintenance granule; no span is modified after publication;
//! the whole allocation is page-aligned for our kernel allocator. This ARM64
//! backend uses memory::heap's contiguous direct-mapped heap; a descriptor
//! span therefore has contiguous device addresses across its pages. Descriptors
//! refer to these stable slices, never a stack object. Publication transfers
//! ownership; only the exact used-ring return or a completed device reset can
//! release it. A timeout does neither. The caller stops the queue on error;
//! Drop retains device-owned storage, so there is at most one quarantined
//! request per failed device. This driver has no in-place reset/restart path:
//! a failed queue remains stopped for the boot. No interrupt alone reclaims it.

extern crate alloc;
use alloc::alloc::{alloc_zeroed, dealloc, Layout};

pub struct DmaRequest {
    base: *mut u8,
    layout: Layout,
    lengths: [usize; 3],
    offsets: [usize; 3],
    device_owned: bool,
}

impl DmaRequest {
    pub fn new(command: &[u8], payload: &[u8], response_size: usize) -> Result<Self, &'static str> {
        let lengths = [command.len(), payload.len(), response_size];
        let mut offsets = [0; 3];
        let mut size = 0usize;
        for (index, length) in lengths.iter().enumerate() {
            if *length > u32::MAX as usize {
                return Err("GPU DMA segment exceeds wire length");
            }
            offsets[index] = size;
            size = size
                .checked_add(*length)
                .and_then(|n| n.checked_add(63))
                .map(|n| n & !63)
                .ok_or("GPU DMA size overflow")?;
        }
        let layout =
            Layout::from_size_align(size.max(64), 4096).map_err(|_| "GPU DMA layout overflow")?;
        let base = unsafe { alloc_zeroed(layout) };
        if base.is_null() {
            return Err("GPU DMA allocation failed");
        }
        let request = Self {
            base,
            layout,
            lengths,
            offsets,
            device_owned: false,
        };
        unsafe {
            core::ptr::copy_nonoverlapping(command.as_ptr(), request.ptr(0), command.len());
            core::ptr::copy_nonoverlapping(payload.as_ptr(), request.ptr(1), payload.len());
            // Poison only response bytes. A short or missing response cannot
            // masquerade as zero-initialized success.
            core::ptr::write_bytes(request.ptr(2), 0xa5, response_size);
        }
        Ok(request)
    }

    pub fn ptr(&self, segment: usize) -> *mut u8 {
        unsafe { self.base.add(self.offsets[segment]) }
    }

    pub fn len(&self, segment: usize) -> usize {
        self.lengths[segment]
    }

    pub fn publish(&mut self) {
        self.device_owned = true;
    }

    /// Caller has validated the exact descriptor-chain return. A device error
    /// response still returns ownership, although it must remain an error.
    pub fn returned(&mut self) {
        self.device_owned = false;
    }
}

impl Drop for DmaRequest {
    fn drop(&mut self) {
        if !self.device_owned {
            unsafe {
                dealloc(self.base, self.layout);
            }
        }
    }
}
