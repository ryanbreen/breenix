# GPU Compositing Attack Plan

## Post-Mortem and Implementation Guide

This document catalogs every attempt made to implement per-window GPU compositing,
identifies the specific root causes of failure, and provides a step-by-step plan
to implement it correctly.

---

## 1. What Was Tried: Complete Catalog

### Attempt 1: Pre-Allocated Texture Pool (8 display-sized textures at init)

**What was done:**
- Created 8 TEXTURE_2D resources (IDs 10-17) at display size (1280x960) during
  `virgl_init()`, before any SUBMIT_3D
- Each texture: `RESOURCE_CREATE_3D(TEXTURE_2D, B8G8R8X8_UNORM, BIND_SAMPLER_VIEW|BIND_SCANOUT)`
- Heap-allocated backing (4.9MB each, 39MB total), paged scatter-gather ATTACH_BACKING
- Primed each with TRANSFER_TO_HOST_3D
- `virgl_composite_gpu_batch()` built single SUBMIT_3D:
  - Pipeline setup: create_sub_ctx(1), set_sub_ctx(1), tweaks, surface(10), blend(11),
    DSA(12), rasterizer(13), VS(14), FS(15), VE(16), sampler_state(18)
  - Background quad: sampler_view(17) on COMPOSITE_TEX(res 5), draw_vbo
  - Per-window quads: sampler_view(40+i) on window tex(res 10+slot), draw_vbo each

**Result:** BLACK screen on first boot. `prlctl capture` showed black even after
90 seconds. Later proved this was capture timing -- after 17+ minutes the display
was actually working. Incorrectly diagnosed as "ATTACH_BACKING poisons pipeline."

**What was actually proven:**
- The display DID work after sufficient time (message 149, 154)
- 4 window textures with ATTACH_BACKING did NOT poison the pipeline (message 286 agent)
- Content was visible: bounce spheres, bcheck 23/23, btop, terminal (message 142)

**What went wrong:**
- Premature conclusion that "ATTACH_BACKING on secondary textures poisons VirGL"
- `prlctl capture` timing issue misinterpreted as rendering failure
- 39MB heap allocation caused OOM on some boots (heap exhaustion, message 143)
- Z-order issue: window content quads drawn at same depth covered window frames

### Attempt 2: Lazy Per-Window Texture Creation

**What was done:**
- Textures created lazily via `init_window_texture()` when windows register
- Various bind flag combinations tested:
  - `BIND_SAMPLER_VIEW` only
  - `BIND_SAMPLER_VIEW | BIND_SCANOUT`
  - `BIND_RENDER_TARGET | BIND_SAMPLER_VIEW | BIND_SCANOUT | BIND_SHARED` (0x14000A)

**Result:** Window content BLACK. Background from COMPOSITE_TEX rendered correctly.
Per-window textured quads rendered as invisible/black.

**What was proven:**
- When per-window quads sampled from COMPOSITE_TEX (instead of their own texture),
  they rendered correctly (message 129) -- proving multi-quad, NDC coords, shaders,
  and UV mapping all work
- `copy_window_pages_to_backing()` confirmed working -- first_pixel values showed
  real application content (0x00648CDC, 0x000A0A19, etc.) not zeros (message 489-492)
- TRANSFER_TO_HOST_3D returned success for per-window textures
- Test colors (red/green/blue/yellow) injected at `init_window_texture()` time were
  overwritten by actual app content before Phase A2 ran -- proving data flow works

**Hypothesis that emerged:** "TRANSFER_TO_HOST_3D only works for resources created
before the first SUBMIT_3D" (message 135). This was tested: a 64x64 test texture
created during init showed RED. Pool textures created during init all worked. But
this hypothesis was DISPROVEN by the Linux probe VM tests (message 2707).

### Attempt 3: Interleaved Z-Order Rendering

**What was done:**
- For each window back-to-front: (a) frame quad from COMPOSITE_TEX at full window
  bounds, (b) content quad from per-window texture at content area
- Cursor overlay quad at end (sampling cursor area from COMPOSITE_TEX)

**Result:** Z-order fixed for the pre-allocated pool path where textures worked,
but the lazily-created textures still showed BLACK content.

### Attempt 4: Background-Only in gpu_batch (Isolation Test)

**What was done:**
- Disabled all per-window quads in `virgl_composite_gpu_batch()` -- drew ONLY the
  background quad from COMPOSITE_TEX (message 307)
- This should produce identical output to `virgl_composite_single_quad()`

**Result:** BLACK even with background only! (message 929)

**Critical finding:** `virgl_composite_single_quad()` worked perfectly, but
`virgl_composite_gpu_batch()` with IDENTICAL background-only code produced BLACK.

**What was tested to explain this:**
- Delegation: `gpu_batch()` calling `single_quad()` internally -- result unknown
  due to build caching (message 2947-2959)
- Stack overflow: ruled out, 2MB stack vs ~12KB usage (message 2955)
- Code inlining: attempted to inline single_quad code into gpu_batch body (message 2959)
- Build caching: cargo builds completing in 0.06s when real recompilation takes
  5-6s, suggesting stale binaries were deployed (message 3011)

**Root cause: ALMOST CERTAINLY BUILD CACHING**

The 0.06-0.07s build times prove that cargo was not recompiling gpu_pci.rs.
Multiple test iterations deployed the SAME stale binary with the original
gpu_batch code, regardless of source changes. This explains why:
- "Identical" code in gpu_batch still produced BLACK (old binary still running)
- Delegation to single_quad appeared not to work (old binary still running)
- Changes to bind flags, handle IDs, etc. had no effect (old binary still running)

### Attempt 5: Multi-Draw Test (Positive Control)

**What was done:**
- Modified `virgl_composite_single_quad()` to add ONE extra draw_vbo after the
  fullscreen background quad (message 290)
- Second quad: top-right corner with flipped UV mapping

**Result:** WORKED (message 293). Both quads visible -- normal background and
upside-down test rectangle. This proved multiple draw_vbo calls in one SUBMIT_3D
batch work on Parallels.

### Attempt 6: Revert

All per-window texture code was removed. Reverted to CPU blit via
`virgl_composite_single_quad()`.

---

## 2. Linux Probe VM Evidence

Eight C test programs were run on the Linux probe VM (Parallels ARM64, Ubuntu 24.04.4,
kernel 6.8.0, virtio_gpu DRM card1, 3D accel highest).

### Definitive Test: `poison_fixed.c`

1. Create display TEXTURE_2D (B8G8R8X8_UNORM, RT|SV|SCANOUT, 1024x768)
2. Map + fill with BLUE via CPU + TRANSFER_TO_HOST
3. Establish DRM scanout (AddFB + SetCrtc)
4. Create N extra TEXTURE_2D (SV|SCANOUT, 128x128), map, fill, TRANSFER_TO_HOST
5. VirGL SUBMIT_3D: create_sub_ctx + CLEAR to RED
6. Re-display (AddFB + SetCrtc)

**Results:**
- 0 extra textures: RED (pass)
- 1 extra texture: RED (pass)
- 8 extra textures: RED (pass)
- 32 extra textures: RED (pass)

### EGL Multi-Texture Test: `gl_multi_texture_test.c`

Used Mesa's EGL surfaceless + GBM + GLES2 pipeline on `/dev/dri/renderD128`:
- Created 2 FBO textures, rendered different colors into each
- Composited both as textured quads onto a third surface

**Result:** Multi-texture VirGL rendering CONFIRMED WORKING on Parallels.

### Key Insight from Linux

Linux's DRM driver uses the exact same protocol: `DRM_IOCTL_VIRTGPU_RESOURCE_CREATE` +
`DRM_IOCTL_VIRTGPU_MAP` + `DRM_IOCTL_VIRTGPU_TRANSFER_TO_HOST`. For each resource:
1. RESOURCE_CREATE_3D: target=TEXTURE_2D(2), format=B8G8R8X8_UNORM(2),
   bind=SV|SCANOUT(0x40008)
2. ATTACH_BACKING: automatic via GEM BO, per-page scatter-gather
3. TRANSFER_TO_HOST_3D: box={0,0,0,w,h,1}, level=0

Resources can be created at ANY time -- there is no requirement that they be
created before the first SUBMIT_3D. The "must create before first SUBMIT_3D"
hypothesis was a misinterpretation of a build caching artifact.

---

## 3. Identified Root Causes

### Root Cause 1: Build Caching (CONFIRMED -- High Confidence)

**Evidence:** Cargo build times of 0.06-0.07s vs 5-6s for real recompilation.
Multiple iterations of "change code, rebuild, deploy, test" were deploying the
exact same stale binary. This made it appear that:
- Changes to gpu_batch had no effect (stale binary)
- gpu_batch with "identical" code to single_quad still failed (stale binary)
- Bind flag changes didn't help (stale binary)
- Handle changes didn't help (stale binary)

**Why:** `gpu_pci.rs` is 4262 lines. Touch detection may not propagate through
the dependency chain correctly, or Parallels VM deployment may reuse a cached
disk image.

**Fix:** Always `touch kernel/src/drivers/virtio/gpu_pci.rs` before building.
Always verify build time is >3 seconds. Always check the `.elf` timestamp.
Always use `run.sh --parallels` which handles the full pipeline including
userspace rebuild and fresh VM creation.

### Root Cause 2: Per-Window Texture Sampling (UNRESOLVED -- Needs Investigation)

**What we know:**
- Per-window quads sampling from COMPOSITE_TEX: WORKS (same batch, same shader)
- Per-window quads sampling from their own texture: BLACK
- TRANSFER_TO_HOST_3D returns success for per-window textures
- Backing data is confirmed non-zero (real app content in first_pixel)

**What we DON'T know (because of build caching):**
- Whether any of the "fixes" tried would have actually worked if properly deployed
- Whether the 64x64 test texture created at init worked because of timing or
  because of size
- Whether the handle allocation scheme actually collided

**Possible sub-causes (all plausible, none definitively confirmed or eliminated):**

**2a. Missing CTX_ATTACH_RESOURCE for per-window textures**

If `virgl_ctx_attach_resource_cmd()` was not called for per-window texture
resources, the VirGL context would not have access to them. The sampler_view
would reference a resource the context cannot see, producing BLACK.

The `init_composite_texture()` function (WORKS) calls:
```
virgl_ctx_attach_resource_cmd(state, VIRGL_CTX_ID, RESOURCE_COMPOSITE_TEX_ID)
```

The per-window `init_window_texture()` code DOES call this. So this is likely
not the issue, but MUST be verified in the new implementation.

**2b. create_sampler_view format encoding error for per-window textures**

Memory note: "create_sampler_view format MUST include texture target -- bits
[24:31] must contain PIPE_TEXTURE_2D << 24. Without it, host creates
BUFFER-targeted sampler view -> black."

The `create_sampler_view` function in virgl.rs correctly encodes:
```rust
let fmt_target = (format & 0x00FF_FFFF) | ((target & 0xFF) << 24);
```

And the call in the batch used `pipe::TEXTURE_2D` for target. This encoding
is correct. However, if the wrong format constant was passed (e.g., a raw
number instead of the pipe constant), it would fail silently.

The background sampler_view(17) uses `vfmt::B8G8R8X8_UNORM, pipe::TEXTURE_2D`.
The per-window sampler_view(40+i) should use the same. If they used a different
format (e.g., B8G8R8A8_UNORM for the texture resource but B8G8R8X8_UNORM for the
sampler_view, or vice versa), the host could reject it silently.

**2c. Texture not primed (TRANSFER_TO_HOST_3D before first sample)**

COMPOSITE_TEX is primed during init. Per-window textures ARE primed during
`init_window_texture()`. But if the priming TRANSFER_TO_HOST_3D was called
AFTER the first SUBMIT_3D that tried to sample from the texture (due to
timing), the host might have cached the texture as empty.

On the other hand, per-frame TRANSFER_TO_HOST_3D uploads should override
this. So this is unlikely but should be verified.

**2d. TRANSFER_TO_HOST_3D stride mismatch**

If the backing buffer has a different stride (row pitch) than what's passed
to TRANSFER_TO_HOST_3D, the host reads garbage. For COMPOSITE_TEX, stride =
`tex_w * 4` (correct). For per-window textures, stride should be `win_w * 4`
if the backing is exactly win_w*win_h*4 bytes. If pool textures are
display-sized but the transfer uses window dimensions with window stride,
the host would read the right data. But if the stride is wrong, the upload
silently corrupts.

**2e. VirGL batch rejection by a bad command**

If ANY command in a SUBMIT_3D batch is malformed, virglrenderer may reject
the ENTIRE batch silently. A bad `create_sampler_view` for a per-window
texture could poison the whole batch, causing even the background quad to
go BLACK.

This would explain the "even background-only gpu_batch is BLACK" finding --
IF the gpu_batch code had additional commands (even unreachable ones) that
were malformed. However, the "background-only" test was supposed to remove
all per-window commands.

**Since build caching was occurring, we cannot know whether the
background-only test actually ran the modified code.**

### Root Cause 3: prlctl capture Timing (CONFIRMED)

`prlctl capture` returns BLACK for 60-90 seconds after boot with VirGL GPU
compositing. This caused multiple false "BLACK screen" diagnoses. The display
WAS rendering correctly -- it just wasn't visible to the capture API.

**Fix:** Always wait at least 90 seconds before capturing. Take multiple captures
5 seconds apart. Use VNC or direct visual inspection when possible.

---

## 4. Handle Allocation Analysis

### VirGL Object Handles (within SUBMIT_3D, single hash table per sub-context)

These are VirGL object handles -- NOT resource IDs. They share one namespace:

| Handle | Object Type | Used By |
|--------|------------|---------|
| 10 | surface | Render target surface on RESOURCE_3D_ID(2) |
| 11 | blend | Simple blend (dither, RGBA colormask) |
| 12 | DSA | Default depth-stencil-alpha |
| 13 | rasterizer | Default rasterizer |
| 14 | shader (VS) | Texture vertex shader |
| 15 | shader (FS) | Texture fragment shader |
| 16 | vertex_elements | 2-attribute VE (pos + texcoord) |
| 17 | sampler_view | Background sampler view on COMPOSITE_TEX(res 5) |
| 18 | sampler_state | Nearest filter, clamp-to-edge |
| 40+i | sampler_view | Per-window sampler view on window tex(res 10+i) |

### GPU Resource IDs (global, outside SUBMIT_3D)

| Resource ID | Type | Purpose |
|-------------|------|---------|
| 2 (RESOURCE_3D_ID) | TEXTURE_2D | VirGL render target (scanout) |
| 3 (RESOURCE_VB_ID) | BUFFER | Vertex buffer (INLINE_WRITE) |
| 5 (RESOURCE_COMPOSITE_TEX_ID) | TEXTURE_2D | Compositor background texture |
| 10-17 | TEXTURE_2D | Per-window texture slots |

### Collision Analysis

VirGL object handles (surface, blend, DSA, etc.) live in a SEPARATE namespace
from GPU resource IDs. Handle 10 (surface) does NOT collide with Resource ID 10
(window texture). These go through different code paths:
- Object handles: `create_surface(handle=10, ...)` inside SUBMIT_3D
- Resource IDs: `virgl_resource_create_3d_cmd(res_id=10, ...)` outside SUBMIT_3D

Within the VirGL object namespace, there are NO collisions in the scheme above:
- Handles 10-18 for pipeline objects
- Handles 40+ for per-window sampler_views

**HOWEVER:** If per-window sampler_view handles collide with ANY pipeline object
handle, virglrenderer replaces the existing object. Handle 17 (bg sampler_view)
is recreated per-frame, and handle 17 is also used for frame-quad sampler_views
in the z-order loop. Recreating handle 17 multiple times per batch (once for bg,
once per frame quad) is fine -- virglrenderer replaces the previous object.

### Recommended Handle Scheme for New Implementation

Use explicit, well-separated ranges:

```
Pipeline objects (created once per batch):
  100: surface (render target)
  101: blend
  102: DSA
  103: rasterizer
  104: VS shader
  105: FS shader
  106: vertex_elements
  107: sampler_state

Sampler views (re-created per draw):
  200: background sampler_view (COMPOSITE_TEX)
  201+i: per-window sampler_view (window texture i)
```

Resource IDs (unchanged):
```
  2: render target
  3: vertex buffer
  5: compositor texture
  10+i: per-window texture i
```

---

## 5. Exact VirGL Command Sequence for Multi-Texture Compositing

### Per-Window Texture Resource Setup (outside SUBMIT_3D, during init or window register)

For each window texture slot i (resource ID = 10+i):

```
1. RESOURCE_CREATE_3D:
     resource_id = 10 + i
     target = TEXTURE_2D (2)
     format = B8G8R8X8_UNORM (2)
     bind = BIND_SAMPLER_VIEW (0x8) | BIND_SCANOUT (0x40000)
     width = window_width (or display_width for pool)
     height = window_height (or display_height for pool)
     depth = 1, array_size = 1, last_level = 0, nr_samples = 0

2. ATTACH_BACKING (paged scatter-gather):
     resource_id = 10 + i
     nr_entries = num_pages
     entries = [{page_phys_addr, 4096}, ...] for each page

3. CTX_ATTACH_RESOURCE:
     ctx_id = VIRGL_CTX_ID (1)
     resource_id = 10 + i

4. TRANSFER_TO_HOST_3D (priming):
     resource_id = 10 + i
     box = {0, 0, 0, width, height, 1}
     level = 0
     stride = width * 4
```

### Per-Frame Upload (outside SUBMIT_3D, for each dirty window)

```
1. Cache clean backing memory (ARM64: DC CIVAC on dirty range)
2. TRANSFER_TO_HOST_3D:
     resource_id = 10 + slot
     box = {0, 0, 0, win_width, win_height, 1}
     stride = win_width * 4  (or pool_width * 4 if pool-sized backing)
```

### Per-Frame SUBMIT_3D Batch

```
// Pipeline setup (same as working virgl_composite_single_quad)
create_sub_ctx(1)
set_sub_ctx(1)
set_tweaks(1, 1)
set_tweaks(2, display_width)
create_surface(100, RESOURCE_3D_ID, B8G8R8X8_UNORM, 0, 0)
set_framebuffer_state(zsurf=0, cbufs=[100])
create_blend_simple(101)
bind_object(101, BLEND)
create_dsa_default(102)
bind_object(102, DSA)
create_rasterizer_default(103)
bind_object(103, RASTERIZER)

// Shaders (num_tokens=300 required by Parallels)
create_shader(104, VERTEX, 300, TEX_VS_TGSI)
bind_shader(104, VERTEX)
create_shader(105, FRAGMENT, 300, TEX_FS_TGSI)
bind_shader(105, FRAGMENT)

// Vertex elements: 2 attributes (position vec4 + texcoord vec4)
create_vertex_elements(106, [(0,0,0,R32G32B32A32_FLOAT), (16,0,0,R32G32B32A32_FLOAT)])
bind_object(106, VERTEX_ELEMENTS)

// Sampler state (shared by all draws)
create_sampler_state(107, CLAMP_TO_EDGE, CLAMP_TO_EDGE, CLAMP_TO_EDGE,
                     NEAREST, MIPFILTER_NONE, NEAREST)
bind_sampler_states(FRAGMENT, 0, [107])
set_min_samples(1)
set_viewport(display_width, display_height)

// CLEAR (optional -- background quad will cover entire screen)
clear_color(0.0, 0.0, 0.0, 1.0)

// === Draw 0: Background quad (fullscreen, from COMPOSITE_TEX) ===
create_sampler_view(200, RESOURCE_COMPOSITE_TEX_ID, B8G8R8X8_UNORM,
                    TEXTURE_2D, 0, 0, 0, 0, IDENTITY_SWIZZLE)
set_sampler_views(FRAGMENT, 0, [200])
resource_inline_write(VB_RES_ID, 0, 128, bg_quad_verts)  // fullscreen NDC
set_vertex_buffers([(32, 0, VB_RES_ID)])
draw_vbo(0, 4, TRIANGLE_FAN, 3)

// === Draw 1..N: Per-window quads (back-to-front z-order) ===
for each window (back to front):
    // Frame quad: from COMPOSITE_TEX at window bounds (includes frame/decorations)
    create_sampler_view(200, RESOURCE_COMPOSITE_TEX_ID, B8G8R8X8_UNORM,
                        TEXTURE_2D, 0, 0, 0, 0, IDENTITY_SWIZZLE)
    set_sampler_views(FRAGMENT, 0, [200])
    resource_inline_write(VB_RES_ID, offset, 128, frame_quad_verts)
    set_vertex_buffers([(32, 0, VB_RES_ID)])
    draw_vbo(start, 4, TRIANGLE_FAN, 3)

    // Content quad: from per-window texture at content area
    create_sampler_view(201 + i, window_res_id, B8G8R8X8_UNORM,
                        TEXTURE_2D, 0, 0, 0, 0, IDENTITY_SWIZZLE)
    set_sampler_views(FRAGMENT, 0, [201 + i])
    resource_inline_write(VB_RES_ID, offset, 128, content_quad_verts)
    set_vertex_buffers([(32, 0, VB_RES_ID)])
    draw_vbo(start, 4, TRIANGLE_FAN, 3)

// === Final: Cursor overlay (from COMPOSITE_TEX cursor area) ===
create_sampler_view(200, RESOURCE_COMPOSITE_TEX_ID, B8G8R8X8_UNORM,
                    TEXTURE_2D, 0, 0, 0, 0, IDENTITY_SWIZZLE)
set_sampler_views(FRAGMENT, 0, [200])
resource_inline_write(VB_RES_ID, offset, 128, cursor_quad_verts)
set_vertex_buffers([(32, 0, VB_RES_ID)])
draw_vbo(start, 4, TRIANGLE_FAN, 3)
```

After SUBMIT_3D:
```
SET_SCANOUT(scanout=0, resource=RESOURCE_3D_ID)
RESOURCE_FLUSH(resource=RESOURCE_3D_ID, rect=0,0,display_w,display_h)
```

### TGSI Shaders (Proven Working)

Vertex shader:
```
VERT
DCL IN[0]
DCL IN[1]
DCL OUT[0], POSITION
DCL OUT[1], GENERIC[0]
  0: MOV OUT[0], IN[0]
  1: MOV OUT[1], IN[1]
  2: END
```

Fragment shader:
```
FRAG
PROPERTY FS_COLOR0_WRITES_ALL_CBUFS 1
DCL IN[0], GENERIC[0], LINEAR
DCL OUT[0], COLOR
DCL SAMP[0]
DCL SVIEW[0], 2D, FLOAT
  0: TEX OUT[0], IN[0], SAMP[0], 2D
  1: END
```

### NDC Coordinate Conversion

Screen pixel (px, py) to NDC:
```
ndc_x = (px / display_w) * 2.0 - 1.0
ndc_y = 1.0 - (py / display_h) * 2.0   // Y flipped
```

Texture UV for per-window content:
```
u_max = win_width / tex_width    // <1.0 if tex is pool-sized
v_max = win_height / tex_height
```

Quad vertices (4 verts, TRIANGLE_FAN, each 8 floats = 32 bytes):
```
top-left:     (ndc_x0, ndc_y0, 0, 1, u0, v0, 0, 0)
bottom-left:  (ndc_x0, ndc_y1, 0, 1, u0, v1, 0, 0)
bottom-right: (ndc_x1, ndc_y1, 0, 1, u1, v1, 0, 0)
top-right:    (ndc_x1, ndc_y0, 0, 1, u1, v0, 0, 0)
```

---

## 6. Step-by-Step Implementation Plan

### Phase 0: Eliminate Build Caching (MANDATORY FIRST STEP)

Before any code changes, establish a reliable build+deploy+verify loop:

1. **Add a build canary:** At the top of `virgl_composite_single_quad()`, add:
   ```rust
   static BUILD_ID: AtomicU32 = AtomicU32::new(0);
   let id = BUILD_ID.fetch_add(1, Ordering::Relaxed);
   if id == 0 {
       crate::serial_println!("[BUILD] gpu_pci.rs build={}", env!("BUILD_TIMESTAMP_PLACEHOLDER"));
       // Or simpler: manually increment a constant each rebuild
       crate::serial_println!("[BUILD] gpu_pci.rs version=42");
   }
   ```
   Change the version number with EVERY rebuild. If the serial log shows the
   wrong version number, the build cache is stale.

2. **Always touch before building:**
   ```bash
   touch kernel/src/drivers/virtio/gpu_pci.rs
   ```

3. **Verify build time:** Real recompilation of gpu_pci.rs takes >3 seconds.
   If cargo finishes in <1 second, the build is cached/stale.

4. **Always use `run.sh --parallels`:** This handles the full pipeline: userspace
   rebuild, ext2 disk creation, fresh VM, serial log truncation.

5. **Wait 90+ seconds** before `prlctl capture`. Take 3 captures 5s apart.

### Phase 1: Prove Multi-Texture Sampling Works (Minimal Test)

**Goal:** Two textures, two quads, one SUBMIT_3D batch. All in virgl_init().

**Steps:**

1. Create a second test texture (resource ID 20, small: 64x64) during virgl_init,
   AFTER COMPOSITE_TEX but BEFORE the Step 9 SUBMIT_3D:
   ```
   RESOURCE_CREATE_3D(res=20, TEXTURE_2D, B8G8R8X8_UNORM, SV|SCANOUT, 64, 64)
   ATTACH_BACKING(res=20, paged scatter-gather)
   CTX_ATTACH_RESOURCE(ctx=1, res=20)
   Fill backing with solid RED (0x00FF0000 in BGRX)
   cache clean
   TRANSFER_TO_HOST_3D(res=20, box=0,0,0,64,64,1, stride=256)
   ```

2. In Step 9 SUBMIT_3D batch, add a second draw_vbo AFTER the existing red quad:
   ```
   // Draw 1: existing fullscreen colored quad (constant buffer FS)
   // Draw 2: small textured quad at top-right from test texture res 20
   create_sampler_view(200, res=20, B8G8R8X8_UNORM, TEXTURE_2D, ...)
   set_sampler_views(FRAGMENT, 0, [200])
   // Switch to texture FS (create_shader + bind_shader)
   resource_inline_write(VB_RES_ID, 128, 128, small_quad_verts)
   set_vertex_buffers([(32, 0, VB_RES_ID)])
   draw_vbo(4, 4, TRIANGLE_FAN, 7)
   ```

3. **Verification:** Display should show dark blue background (CLEAR) with
   fullscreen red quad AND a small red rectangle at top-right (from the test
   texture). If the test texture quad is BLACK, the texture sampling is broken.
   If it's RED, it works and we can proceed.

4. **If BLACK:** Compare the create_sampler_view encoding byte-for-byte with
   the working COMPOSITE_TEX sampler_view. Check:
   - Is the format DWORD identical? (bits [0:23] = format, [24:31] = target)
   - Is CTX_ATTACH_RESOURCE called for resource 20?
   - Is the TRANSFER_TO_HOST_3D box correct?
   - Print the raw hex of the SUBMIT_3D buffer and diff the two sampler_view
     commands

### Phase 2: Per-Window Texture in Production Pipeline

Only proceed after Phase 1 passes.

1. **Add a `virgl_create_window_texture()` function** (not a pool -- one per window):
   ```rust
   fn virgl_create_window_texture(
       slot: usize, width: u32, height: u32
   ) -> Result<u32, &'static str> {
       let res_id = 10 + slot as u32;
       // Same pattern as init_composite_texture:
       // 1. Heap allocate backing (page-aligned)
       // 2. RESOURCE_CREATE_3D(res_id, TEXTURE_2D, B8G8R8X8_UNORM, SV|SCANOUT, w, h)
       // 3. virgl_attach_backing_paged(res_id, ptr, size)
       // 4. virgl_ctx_attach_resource_cmd(VIRGL_CTX_ID, res_id)
       // 5. cache clean + TRANSFER_TO_HOST_3D (prime)
       Ok(res_id)
   }
   ```

2. **Call from graphics.rs** when a window registers (lazy init).

3. **Per-frame upload:** In `virgl_composite_windows()`, for each dirty window:
   - Copy MAP_SHARED pages to contiguous backing (`copy_window_pages_to_backing`)
   - Cache clean the backing
   - TRANSFER_TO_HOST_3D

4. **Modify `virgl_composite_single_quad()` to accept window quads:**

   Rather than creating a new gpu_batch function, EXTEND the existing working
   function. This eliminates the "two functions, one works, one doesn't" problem.

   Add a parameter for optional per-window quads:
   ```rust
   fn virgl_composite_single_quad_with_windows(
       windows: &[WindowQuadInfo],
   ) -> Result<(), &'static str>
   ```

   Start with ZERO windows (identical to current single_quad). Add windows
   one at a time, testing after each addition.

5. **Verification at each step:**
   - 0 windows: identical to current display (regression test)
   - 1 window: background + one window content quad (should show window content)
   - N windows: background + all windows in z-order

### Phase 3: Z-Order Interleaving

Once per-window texture sampling is confirmed working:

1. For each window (back to front):
   - Draw frame quad from COMPOSITE_TEX (window bounds including title bar + border)
   - Draw content quad from per-window texture (content area only)
2. Final cursor overlay quad from COMPOSITE_TEX

### Phase 4: Remove CPU Blit from BWM

1. BWM stops calling `blit_client_pixels()` for windows with GPU textures
2. BWM only composites background, decorations, and cursor into COMPOSITE_TEX
3. Per-window content goes directly from MAP_SHARED pages to GPU texture backing

---

## 7. Diagnostic Checklist

When per-window texture sampling produces BLACK, check these in order:

1. **Build canary:** Does the serial log show the expected build version number?
   If not, STOP -- you are running stale code.

2. **CTX_ATTACH_RESOURCE:** Was `virgl_ctx_attach_resource_cmd(1, res_id)` called
   for this texture resource? Check serial log for the attach message.

3. **create_sampler_view format DWORD:** Print the raw u32 value of the fmt_target
   parameter. It MUST be `(B8G8R8X8_UNORM & 0x00FFFFFF) | (TEXTURE_2D << 24)` =
   `0x02000002`. If it's `0x00000002`, the texture target is missing -> BLACK.

4. **TRANSFER_TO_HOST_3D box:** Print the box parameters. Width and height must
   match the resource dimensions, not zero. Stride must be `width * 4`.

5. **Backing data:** Print first_pixel of the backing buffer after copy. If it's
   zero, the copy failed. If it's non-zero, the data is present.

6. **Batch rejection:** If EVEN the background quad is BLACK, a command earlier
   in the batch is poisoning it. Binary search: comment out the last half of
   commands, test, narrow down.

7. **prlctl capture timing:** Wait 90 seconds. Take 3 captures. If all 3 are
   black, it's a real rendering issue. If the 3rd shows content, it's timing.

---

## 8. What NOT to Do

1. **Do NOT create a separate gpu_batch function.** Extend the working
   `virgl_composite_single_quad()` incrementally. The original failure was
   partly caused by having two "identical" functions where one worked and
   one didn't -- a situation made impossible to debug by build caching.

2. **Do NOT pre-allocate a pool of 8 display-sized textures.** Each texture
   is 4.9MB of heap. 8 textures = 39MB. This caused OOM on some boots.
   Create textures at actual window dimensions, lazily.

3. **Do NOT conclude "ATTACH_BACKING poisons the pipeline"** without first
   verifying the build canary. This was a false conclusion caused by stale
   binaries.

4. **Do NOT change multiple variables at once.** Change one thing, verify the
   build canary, wait 90 seconds, capture. One variable per test cycle.

5. **Do NOT use `prlctl capture` as the sole verification method.** Also check
   serial output for SUBMIT_3D success/failure, check for error responses,
   and use visual inspection via the VM window when possible.
