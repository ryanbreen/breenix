# Compositor CPU Reduction Attack Plan

Baseline: BWM at 70% CPU, 155Hz loop, 6451us/iter, 13 syscalls/iter, 50% idle polling.

## Priority 1: Event-Driven Compositor Wait [DONE]

**Actual savings: ~14% CPU (34% -> 20%)**
**Status: Merged (PR #254)**

New syscall op=23 `compositor_wait(timeout_ms, last_registry_gen)` that:
- Blocks in kernel until woken by mark_window_dirty, mouse, or registry change
- Returns bitmask: bit0=dirty, bit1=mouse, bit2=registry + packed registry_gen
- USB HID mouse handler wakes compositor on movement
- REGISTRY_GENERATION atomic bumped on window register
- Removed duplicate blocking from op=16 composite_windows

BWM main loop restructured: only blits when DIRTY flag set, only discovers
windows when REGISTRY flag set, no idle polling.

## Priority 2: MAP_SHARED Client Windows + Occluded Blit [DONE]

**Actual savings: ~36% CPU (70% -> 34%)**
**Status: Merged (PR #253)**

- op=21 `map_window_buffer`: maps client window physical pages into BWM read-only
- op=22 `check_window_dirty`: lightweight generation check without pixel copy
- Occluded blit: span-based row clipping skips pixels covered by higher-z windows
- Eliminates: read_window_buffer syscalls, kernel page copies, pixel_cache, z-repair

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
- kernel/src/syscall/graphics.rs (composite_windows handler)
- kernel/src/drivers/virtio/gpu_pci.rs virgl_composite_windows signature

## Priority 4: Reduce clock_gettime Calls [DONE]

**Status: Done (part of PR #253)**

Reduced from 5x now_monotonic() per iteration to 2 (start + end).

## Priority 5: Batch Window Discovery [DONE]

**Status: Done (part of PR #254)**

REGISTRY_GENERATION atomic in kernel, compositor_wait returns generation.
BWM only calls list_windows when registry changed.

## Actual Results

| After Attack | CPU | FPS |
|-------------|-----|-----|
| Baseline | 70% | ~100 |
| +Priority 2 (MAP_SHARED + occluded blit) | 34% | ~130 |
| +Priority 1 (compositor_wait) | ~20% | ~186 |

Total reduction: 70% -> 20% (71% reduction), FPS: 100 -> 186 (86% increase).
