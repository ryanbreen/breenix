# Compositor CPU Reduction Attack Plan

Baseline: BWM at 70% CPU, 155Hz loop, 6451us/iter, 13 syscalls/iter, 50% idle polling.

## Priority 1: Event-Driven Compositor Wait [IN PROGRESS]

**Expected savings: 30-40% CPU**
**Status: Implementing**

New syscall op=22 `compositor_wait(timeout_ms)` that combines:
- Dirty check for all windows (replaces 4x op=14 generation checks)
- Keyboard/mouse input poll (replaces poll + mouse_state)
- Window list change detection (replaces op=13 per-frame)
- Blocks if nothing pending (replaces sleep_ms(2))

Returns a bitmask of what's ready. Eliminates idle polling (500Hz -> 0Hz when idle).

Files:
- kernel/src/syscall/graphics.rs: Add op=22 handler
- libs/libbreenix/src/graphics.rs: Add compositor_wait() wrapper
- userspace/programs/src/bwm.rs: Restructure main loop around compositor_wait

## Priority 2: MAP_SHARED Client Windows (Zero-Copy Blit)

**Expected savings: 15-25% CPU**
**Status: Planned**

New syscall op=23 `map_window_buffer(buffer_id)` maps client window physical pages
into BWM's address space (read-only). BWM blits directly from mapped pages to
COMPOSITE_TEX. Eliminates:
- 4x read_window_buffer syscalls per iteration
- Kernel page-by-page copy under spinlock (up to 8.6MB for terminal)
- pixel_cache intermediate copy

Uses same infrastructure as map_compositor_texture (op=20).

Files:
- kernel/src/syscall/graphics.rs: Add op=23 handler (model on op=20)
- libs/libbreenix/src/graphics.rs: Add map_window_buffer() wrapper
- userspace/programs/src/bwm.rs: Replace blit_client_pixels with direct mapped reads

## Priority 3: Strip Vestigial Composite Allocations

**Expected savings: 3-5% CPU**
**Status: Planned**

handle_composite_windows (op=16) allocates Vec<WindowCompositeInfo> and clones
page_phys_addrs per window every frame. In MAP_SHARED path, page_phys_addrs is
NEVER used by virgl_composite_windows -- it's vestigial. Also registered_windows()
allocates a new Vec every call.

Fix: Remove Vec construction, pass only bg_dirty + dirty_rect to GPU driver.
Replace registered_windows() with direct-write to output buffer.

Files:
- kernel/src/syscall/graphics.rs lines 1110-1147
- kernel/src/drivers/virtio/gpu_pci.rs virgl_composite_windows signature

## Priority 4: Reduce clock_gettime Calls

**Expected savings: 1-2% CPU**
**Status: Planned**

5x now_monotonic() per iteration is pure measurement overhead. Feature-gate behind
a perf_instrumentation flag or reduce to 2 (start + end).

Files:
- userspace/programs/src/bwm.rs: lines 539, 545, 558, 690, 738, 777

## Priority 5: Batch Window Discovery

**Expected savings: 1-2% CPU**
**Status: Planned**

list_windows (op=13) called every iteration but windows only change at startup.
Add a registry generation counter in shared page. BWM checks counter, only calls
list_windows when it changes.

Files:
- kernel/src/syscall/graphics.rs: Add atomic generation to WindowRegistry
- userspace/programs/src/bwm.rs line 548: Check generation before calling

## Projected Totals

| After Attack | CPU Estimate |
|-------------|-------------|
| Baseline    | 70%         |
| +Priority 1 | 30-40%     |
| +Priority 2 | 15-20%     |
| +Priority 3 | 12-17%     |
| +Priority 4 | 11-16%     |
| +Priority 5 | 10-15%     |
