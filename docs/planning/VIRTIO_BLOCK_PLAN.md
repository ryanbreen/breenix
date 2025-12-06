# VirtIO Block Driver Implementation Plan

## Overview

Implement a VirtIO block device driver for Breenix, enabling disk I/O operations through QEMU's VirtIO infrastructure. The VirtIO device is already detected at PCI 00:04.0 with:
- BAR0: I/O port 0xc000
- BAR1: MMIO 0x81060000 (32-bit)
- BAR4: MMIO 0xc000000000 (64-bit)
- IRQ: 11

## Architecture Decision: Legacy vs Modern VirtIO

The detected device ID is 0x1001 (legacy transitional device), so we'll use the **VirtIO Legacy I/O port interface** rather than the modern MMIO interface. This is simpler and matches QEMU's default configuration.

### Legacy Interface Register Layout (BAR0 I/O ports)

| Offset | Size | Register |
|--------|------|----------|
| 0x00   | 4    | Device Features |
| 0x04   | 4    | Guest Features |
| 0x08   | 4    | Queue Address |
| 0x0C   | 2    | Queue Size |
| 0x0E   | 2    | Queue Select |
| 0x10   | 2    | Queue Notify |
| 0x12   | 1    | Device Status |
| 0x13   | 1    | ISR Status |
| 0x14+  | -    | Device-specific config (capacity, etc.) |

## Implementation Steps

### Phase 1: VirtIO Transport Layer

**File**: `kernel/src/drivers/virtio/mod.rs`

1. **Device Status Constants**
   - ACKNOWLEDGE, DRIVER, DRIVER_OK, FEATURES_OK, FAILED

2. **VirtIO Device Structure**
   ```rust
   pub struct VirtioDevice {
       io_base: u16,          // BAR0 I/O port base
       device_features: u32,  // Features offered by device
       driver_features: u32,  // Features we support
   }
   ```

3. **Device Initialization**
   - Reset device (write 0 to status)
   - Set ACKNOWLEDGE bit
   - Set DRIVER bit
   - Read device features
   - Negotiate features (write driver features)
   - Set FEATURES_OK
   - Verify FEATURES_OK is still set
   - Configure device-specific settings
   - Set DRIVER_OK

### Phase 2: Virtqueue Implementation

**File**: `kernel/src/drivers/virtio/queue.rs`

1. **Virtqueue Data Structures**
   ```rust
   #[repr(C, align(16))]
   pub struct VirtqDesc {
       addr: u64,    // Physical address of buffer
       len: u32,     // Buffer length
       flags: u16,   // NEXT, WRITE, INDIRECT
       next: u16,    // Next descriptor index (if NEXT set)
   }

   #[repr(C)]
   pub struct VirtqAvail {
       flags: u16,
       idx: u16,
       ring: [u16; QUEUE_SIZE],
       used_event: u16,  // Only if VIRTIO_F_EVENT_IDX
   }

   #[repr(C)]
   pub struct VirtqUsed {
       flags: u16,
       idx: u16,
       ring: [VirtqUsedElem; QUEUE_SIZE],
       avail_event: u16,
   }

   pub struct Virtqueue {
       desc: *mut VirtqDesc,
       avail: *mut VirtqAvail,
       used: *mut VirtqUsed,
       free_head: u16,
       num_free: u16,
       last_used_idx: u16,
       queue_size: u16,
   }
   ```

2. **Memory Allocation**
   - Allocate contiguous physical pages for the virtqueue
   - Calculate offsets for desc/avail/used rings
   - Initialize descriptor chain (free list)

3. **Queue Operations**
   - `allocate_descriptors(count)` - Allocate descriptor chain
   - `add_buffer(descs, data)` - Add buffer to available ring
   - `get_buf()` - Poll used ring for completed buffers
   - `notify()` - Write to queue notify register

### Phase 3: Block Device Driver

**File**: `kernel/src/drivers/virtio/block.rs`

1. **Block Request Structure**
   ```rust
   #[repr(C)]
   struct VirtioBlkReq {
       type_: u32,      // VIRTIO_BLK_T_IN, VIRTIO_BLK_T_OUT
       reserved: u32,
       sector: u64,     // Starting sector
   }

   const VIRTIO_BLK_T_IN: u32 = 0;   // Read
   const VIRTIO_BLK_T_OUT: u32 = 1;  // Write
   ```

2. **VirtioBlockDevice Structure**
   ```rust
   pub struct VirtioBlockDevice {
       device: VirtioDevice,
       queue: Virtqueue,
       capacity: u64,      // In sectors
       sector_size: u32,   // Usually 512 bytes
   }
   ```

3. **Operations**
   - `init(pci_device)` - Initialize from PCI device
   - `read_sector(sector, buf)` - Read one sector
   - `write_sector(sector, buf)` - Write one sector
   - `read_sectors(start, count, buf)` - Batch read
   - `write_sectors(start, count, buf)` - Batch write
   - `handle_interrupt()` - Process completed I/O

### Phase 4: IRQ Handler

**File**: `kernel/src/interrupts.rs` modifications

1. Add VirtIO IRQ handler (IRQ 11, vector 43)
   - Must follow strict timing requirements (no logging!)
   - Clear ISR status register
   - Wake any waiting tasks
   - Send EOI to PIC

2. Unmask IRQ 11 in PIC2 (modify `init_pic()`)

### Phase 5: Integration

1. **Driver Initialization**
   - Modify `kernel/src/drivers/mod.rs` to init VirtIO after PCI scan
   - Store reference to block device for filesystem layer

2. **Testing**
   - Create test that reads/writes to VirtIO block device
   - Verify data integrity (write pattern, read back, compare)

## File Structure

```
kernel/src/drivers/
├── mod.rs           # Existing - add virtio module
├── pci.rs           # Existing - complete
└── virtio/
    ├── mod.rs       # VirtIO transport layer
    ├── queue.rs     # Virtqueue implementation
    └── block.rs     # Block device driver
```

## Memory Requirements

For a standard virtqueue with 256 entries:
- Descriptors: 256 * 16 bytes = 4 KB
- Available ring: ~520 bytes
- Used ring: ~2056 bytes
- **Total**: ~8 KB (2 physical pages)

For block I/O buffers:
- Allocate per-request from kernel heap
- Or maintain a pool of pre-allocated DMA buffers

## DMA Buffer Strategy

Option A: **Per-request allocation** (simpler)
- Allocate physical frame for each I/O
- Map to kernel virtual address
- Free after completion

Option B: **Buffer pool** (better performance)
- Pre-allocate pool of DMA buffers at init
- Track in-use status
- Faster allocation, bounded memory use

**Recommendation**: Start with Option A for simplicity, optimize later if needed.

## Critical Constraints

1. **IRQ Handler** (from CLAUDE.md):
   - NO serial output or logging
   - NO heap allocations
   - NO locks that might contend
   - Target <1000 cycles

2. **DMA Addresses**:
   - Must use physical addresses in descriptors
   - Convert via `frame.start_address().as_u64()`

3. **Memory Barriers**:
   - Use `core::sync::atomic::fence(Ordering::SeqCst)` before notify
   - Ensure descriptor writes are visible to device

## Testing Plan

1. **Unit tests** (compile-time):
   - Structure sizes and alignment
   - Virtqueue layout calculations

2. **Integration tests** (via GDB):
   - Device detection and initialization
   - Queue setup and notification
   - Single sector read
   - Single sector write
   - Data integrity verification

## Success Criteria

- [ ] VirtIO device initialized successfully
- [ ] Virtqueue allocated and configured
- [ ] Can read sector 0 (boot sector or partition table)
- [ ] Can write and read back data
- [ ] IRQ handler properly acknowledges completions
- [ ] No panics, no warnings, clean builds
