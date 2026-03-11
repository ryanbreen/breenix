# GPU-Only Rendering Attack Plan

## Problem

The current rendering pipeline wastes CPU on work the GPU should do:

1. **BWM compositing**: CPU-blits window pixels into compositor texture row-by-row
   (`blit_client_pixels`), then does TRANSFER_TO_HOST_3D to upload to GPU. Linux ftrace
   proved this transfer is unnecessary: Mesa's per-frame path is just
   **SUBMIT_3D -> SET_SCANOUT -> RESOURCE_FLUSH** with zero CPU transfers.

2. **Bounce (and all Breengel clients)**: Software-renders pixels into shared memory
   buffers. Bounce draws circles pixel-by-pixel on CPU. All rendering should use VirGL
   GPU primitives (DRAW_VBO with shaders).

3. **Per-window texture "limitation" was a bug**: The note "per-window VirGL textures
   DON'T work" was a bug in our resource creation, not a Parallels limitation. Linux
   probe VM proved multiple VirGL textures work correctly on identical hardware.

## Target Architecture

```
Client (bounce, bterm, etc.)           BWM Compositor
  |                                       |
  | VirGL SUBMIT_3D                       | VirGL SUBMIT_3D
  | (draw geometry into                   | (draw textured quads for
  |  per-window texture)                  |  each window texture onto
  |                                       |  compositor surface)
  v                                       v
  GPU renders to                          GPU composites all windows
  window texture                          -> SET_SCANOUT -> RESOURCE_FLUSH
```

Zero CPU pixel copying. Zero TRANSFER_TO_HOST_3D per frame.

## Phase 1: Fix Per-Window VirGL Textures

**Goal**: Create multiple VirGL TEXTURE_2D resources that can be rendered to and sampled from.

### Debugging Approach (Linux-first, per proven methodology)

1. On Linux probe VM, write a test program that:
   - Creates 2+ RESOURCE_CREATE_3D textures (TEXTURE_2D, B8G8R8X8_UNORM)
   - ATTACH_BACKING with paged scatter-gather for each
   - SUBMIT_3D: render different colors into each texture (set as render target, CLEAR)
   - SUBMIT_3D: sample from both textures as textured quads onto a third surface
   - SET_SCANOUT + RESOURCE_FLUSH
   - Verify both textures display correctly

2. If it works on Linux (expected), capture the exact VirGL byte sequence with
   virgl_intercept.c LD_PRELOAD.

3. Port the exact bytes to Breenix. If it fails, diff against the Linux bytes to find
   the resource creation/backing bug.

### Likely Bug Candidates

- Missing ATTACH_BACKING on new resources (paged scatter-gather required)
- Missing CTX_ATTACH_RESOURCE for new resources
- Missing "priming" TRANSFER_TO_HOST_3D (required once per resource, not per frame)
- Wrong bind flags (need RENDER_TARGET | SAMPLER_VIEW at minimum)
- Handle collisions in virglrenderer hash table (handles must be globally unique)

### Files
- `kernel/src/drivers/virtio/gpu_pci.rs` — resource creation, backing attachment
- `kernel/src/drivers/virtio/virgl.rs` — VirGL command encoding

## Phase 2: GPU-Based BWM Compositing

**Goal**: BWM composites windows using GPU textured quads instead of CPU blit.

### Architecture

1. Each registered window gets a VirGL TEXTURE_2D resource (created once)
2. Window pixel data lives in the texture's backing pages (MAP_SHARED to client)
3. Per-frame, BWM issues one SUBMIT_3D batch:
   - For each visible window: create_sampler_view on window texture, bind as FS input,
     DRAW_VBO textured quad at window position
   - Background quad rendered first, windows in z-order on top
4. SET_SCANOUT + RESOURCE_FLUSH (matches Linux per-frame sequence)

### Key Change: No TRANSFER_TO_HOST_3D Per Frame

The current pipeline does TRANSFER_TO_HOST_3D every frame to upload pixel data. Linux
proves this is unnecessary — the host reads directly from the GPU texture's backing
pages when rendering via SUBMIT_3D. The one-time "priming" TRANSFER_TO_HOST_3D at
resource creation is sufficient.

### Window Dirty Tracking

When a client calls mark_window_dirty, BWM knows to include that window in the next
SUBMIT_3D batch. Clean windows can be skipped (their texture is already on the GPU
from the previous frame).

### Files
- `userspace/programs/src/bwm.rs` — compositor main loop, replace blit_client_pixels
- `kernel/src/syscall/graphics.rs` — window buffer syscalls, texture resource management
- `kernel/src/drivers/virtio/gpu_pci.rs` — per-window resource creation

## Phase 3: Client-Side GPU Rendering (Bounce)

**Goal**: Bounce renders spheres using VirGL DRAW_VBO instead of CPU pixel pushing.

### Architecture

1. Bounce creates its window (gets a VirGL texture resource as render target)
2. Each frame, bounce issues VirGL commands via a new syscall:
   - Set window texture as render target
   - CLEAR background
   - For each sphere: DRAW_VBO with colored vertices (triangle fan or instanced quad
     with circle fragment shader)
3. Calls mark_window_dirty to trigger BWM compositing

### New API: Breengel GPU Drawing

Breengel needs a GPU drawing API so clients don't need to encode raw VirGL:

```rust
// Proposed Breengel GPU API
impl Window {
    fn begin_frame(&mut self);
    fn clear(&mut self, color: Color);
    fn draw_circle(&mut self, cx: i32, cy: i32, radius: i32, color: Color);
    fn draw_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: Color);
    fn draw_text(&mut self, text: &[u8], x: i32, y: i32, color: Color);
    fn end_frame(&mut self); // triggers SUBMIT_3D + mark_dirty
}
```

Under the hood, these accumulate VirGL commands and submit in one batch.

### Files
- `libs/breengel/src/lib.rs` — GPU drawing API
- `userspace/programs/src/bounce.rs` — convert to GPU rendering
- `kernel/src/syscall/graphics.rs` — new syscall for client SUBMIT_3D

## Phase 4: Text Rendering on GPU

**Goal**: bterm, bcheck, btop render text using GPU textured quads with a font atlas.

### Architecture

1. Upload bitmap font as a VirGL texture (one-time)
2. Each glyph = textured quad sampling from the font atlas
3. Text rendering becomes a batch of DRAW_VBO calls with texture coordinates

This eliminates the biggest CPU cost in terminal rendering — drawing characters
pixel-by-pixel into framebuffers.

## Verification

Each phase should be verified independently:

- **Phase 1**: Create 2 textures, render different colors, sample both in one frame
- **Phase 2**: BWM composites without CPU blit, no TRANSFER_TO_HOST_3D per frame
- **Phase 3**: Bounce renders at 60+ FPS with ~0% CPU (only physics simulation)
- **Phase 4**: bterm scrolls smoothly with minimal CPU

## Priority Order

Phase 1 (fix per-window textures) unblocks everything else. Start there.
Phase 2 (GPU compositing) gives the biggest immediate win — eliminates the CPU blit.
Phase 3 (client GPU rendering) makes bounce truly GPU-rendered.
Phase 4 (text on GPU) is the final polish for terminal/text apps.
