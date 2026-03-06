# Breenix Project Memory

## Core Debugging Principle: Parallels Is Hardware

**NEVER treat Parallels as something strange or as something that will violate a spec in unexpected ways.** Parallels is hardware as far as we're concerned. It implements the VirtIO GPU spec correctly — Linux proves this by achieving 1000+ FPS with VirGL on the same platform. When VirGL commands don't produce expected results, the problem is in OUR protocol usage, not in Parallels. Blaming the hardware is giving up on hard problem solving.

## 🚨 SMOKING GUN: Raw VirGL Rendering WORKS on Parallels 🚨

**PROVEN March 2026:** A hand-crafted VirGL CLEAR command (our bytes, NOT Mesa) produced a
visible BLUE screen on the Parallels display. Screenshot saved at `~/Downloads/linux-probe-virgl-test.png`.

**What the test did (`/tmp/gbm_virgl_test.c` on linux-probe VM):**
1. Used GBM/EGL to create the resource and set up the VirGL context (Mesa handles plumbing)
2. Injected a **raw VirGL CLEAR command** (hand-crafted, identical encoding to Breenix)
3. The BLUE clear overwrote Mesa's GREEN clear — the M3 Max GPU executed OUR command

**What this proves:**
- Our VirGL CLEAR command encoding is CORRECT
- The Apple M3 Max GPU executes raw VirGL commands through Parallels
- Hardware-accelerated GL rendering is fully achievable on Parallels

**Why virgl_raw_test.c (standalone, no Mesa) shows BLACK:**
- Mesa's RESOURCE_CREATE returns `size=3145728, stride=4096` (shared guest/host backing)
- Our raw RESOURCE_CREATE with IDENTICAL params returns `size=0, stride=0` (no guest backing)
- The difference: Mesa/GBM does capability negotiation (GETPARAM, GET_CAPS) before creating
  resources, which enables shared-memory resource mode
- Without shared backing, the host renders to its own GPU memory but the display can't see it

**The fix for Breenix:** Replicate Mesa's resource creation flow — specifically the shared-memory
backing that allows both VirGL rendering (host-side) and display scanout (guest-side) to access
the same memory. Our VirGL command encoding is already correct.

**NEVER AGAIN waste time questioning whether VirGL works on Parallels. It does. Period.**

## Bounce Demo Performance (March 2026)

### Batch Flush Optimization: 12 FPS → 76 FPS (6.3x speedup!)
- **Before:** 13 individual syscalls + 13 DSB barriers + 16ms sleep per frame = 12 FPS
- **After:** 1 batch flush syscall (op=8) + 1 DSB barrier + 1ms sleep = 76 FPS (13ms/frame)
- Batch flush (op=8) in `kernel/src/syscall/graphics.rs` copies multiple dirty rects in one syscall
- `libs/libbreenix/src/graphics.rs`: FlushRect struct + fb_flush_rects() wrapper
- `userspace/programs/src/bounce.rs`: Uses batch flush, sleep reduced 16ms→1ms
- Init auto-launches bounce: `userspace/programs/src/init.rs`

### BSS/Stack Overlap Bug → Heap Allocation Fix (RESOLVED)
- **Root cause:** PCI_3D_FRAMEBUFFER in BSS at phys ~0x41AEC000 overlapped Parallels boot stack at 0x42000000
- DMA reads from BSS backing returned corrupted/zero data → display showed BLACK
- **Fix:** Replaced BSS static with heap-allocated, page-aligned (4096) backing via `alloc::alloc::alloc_zeroed`
- Heap allocation lands at ~0x502E2000, well clear of stack region
- **Result:** BLUE screen displayed successfully via TRANSFER_TO_HOST_3D + SET_SCANOUT + RESOURCE_FLUSH
- PCI_FRAMEBUFFER (2D) is still BSS but never used for DMA on GOP path

## VirGL Display on Parallels — IN PROGRESS (March 2026)

### CRITICAL: VirGL Encoding Must Match Mesa Exactly
**Methodology that works:** Write a test program on Linux, validate it produces visible output,
then port the exact same bytes to Breenix. Use LD_PRELOAD ioctl interception to capture
Mesa's actual VirGL command bytes and compare against hand-crafted commands.
**Tool:** `scripts/parallels/virgl_intercept.c` — LD_PRELOAD .so that hex-dumps EXECBUFFER payloads.

### Known VirGL Encoding Bugs (FOUND March 2026)
1. **Blend colormask shift is WRONG:** We used `0xF << 28` but correct is `0xF << 27`
   - `virgl.rs` line 237: `self.push(0xF << 28)` → should be `self.push(0xF << 27)` = `0x78000000`
   - Mesa sends `0x78000000`, we sent `0xF0000000` — 1-bit shift error
   - This causes the GPU to NOT write color channels properly → BLACK screen
   - See `VIRGL_OBJ_BLEND_S2_RT_COLORMASK(x) = ((x) & 0xf) << 27` in virgl_hw.h
2. **Rasterizer flags differ from Mesa:** Our `0x20004002` vs Mesa's `0x60008082`
   - Mesa adds POINT_QUAD_RAST(bit7), FRONT_CCW(bit15), BOTTOM_EDGE_RULE(bit30)
   - Our scissor enable (bit14) is fine but Mesa doesn't use it for simple clears
3. **Blend S0 missing dither:** Mesa sends `0x00000004` (dither=bit2), we send `0x00000000`
4. **Missing commands Mesa sends:** SET_TWEAKS, SET_POLYGON_STIPPLE, SET_BLEND_COLOR,
   SET_MIN_SAMPLES — may not be critical but should be added for compatibility

### What Works
- Heap-allocated backing fixes DMA (BSS at 0x41AEC000 overlapped stack)
- RESOURCE_CREATE_3D with B8G8R8X8_UNORM + BIND_SCANOUT succeeds
- SUBMIT_3D (VirGL clear/draw) returns OK with fence completion
- XRGB8888 (B8G8R8X8_UNORM) is REQUIRED — ARGB8888 causes EINVAL
- **gl_display.c (EGL/Mesa) renders at 120+ FPS on Linux probe VM** — proves VirGL works

### Linux Probe VM Findings
- VirGL rendering works at 120+ FPS via `virgl (Apple M3 Max (Compat))`
- gl_display.c (EGL/Mesa) shows bouncing balls — WORKING reference
- **Raw VirGL CLEAR on Mesa's context → BLUE screen** — our encoding is correct
- virgl_raw_test.c (standalone, no Mesa) shows BLACK — resource creation issue, NOT encoding
- The standalone test creates host-managed resources (size=0, stride=0, MAP fails)
- Mesa creates shared-memory resources (size=3145728, stride=4096, MAP works)
- Mesa does NO TRANSFER_TO_HOST for VirGL resources — rendering + display share memory
- Linux DRM SetCrtc + PageFlip → host SET_SCANOUT + RESOURCE_FLUSH internally

## VirGL Infrastructure

- `gpu_pci.rs`: VirGL 3D pipeline (CTX_CREATE, SUBMIT_3D with 3-desc chain, etc.)
- `virgl.rs`: VirGL command encoder (clear, shaders, draw, surfaces, etc.)
- SUBMIT_3D requires 3-descriptor virtqueue chain (header, payload, response)
- VirtioGpuCmdSubmit must be `repr(C, packed)` — 28 bytes, NOT 32
- GET_CAPSET_INFO returns 0x1200 (unsupported) despite num_capsets=1

## Linux Probe VM (Parallels)

- **OS:** Ubuntu 24.04.4 Server ARM64 (Linux 6.8.0-101-generic)
- **Name:** linux-probe, **IP:** 10.211.55.149
- **SSH:** `sshpass -p root ssh wrb@10.211.55.149`
- **Snapshot:** "baseline-with-devtools" — gcc, libdrm-dev, virgl_raw_test built
- **DRM:** card1 (virtio_gpu), renderD128. 3D accel = highest
- **Programs:** virgl_raw_test, dumb_blue_test, gl_display, modetest all built and tested
- **Finding:** DRM SetCrtc works for GBM-created resources (shared backing), fails for raw RESOURCE_CREATE (host-only backing)

## 🚨🚨🚨 Parallels VM Testing — FRESH VM EVERY TIME 🚨🚨🚨

**ABSOLUTE RULE: NEVER reuse the same VM name twice. NEVER use `deploy-to-vm.sh --boot`.**
**NEVER use `prlctl stop breenix-dev --kill` then restart the same VM.**
Every single test MUST create a brand new VM with a unique name (timestamp-based).
Reusing a VM name leads to stale disk images, cached framebuffers, and WRONG test results.

**Use `scripts/parallels/quick-test.sh`** which:
1. Deletes all old `breenix-*` VMs
2. Creates a fresh VM with a timestamped or unique name
3. Attaches freshly built disk images
4. Starts the VM and takes a screenshot

**The dispatch to agents MUST include this instruction.** When dispatching a build+test
agent, always tell it to use quick-test.sh or create a fresh uniquely-named VM.
NEVER tell agents to use `deploy-to-vm.sh --boot` or to restart `breenix-dev`.

## VirGL Debugging Methodology (PROVEN March 2026)

**Always validate on Linux FIRST, then port to Breenix.** The workflow:
1. Write/modify test program on Linux probe VM
2. Run it, screenshot, confirm it produces expected visual output
3. If it works on Linux, port the exact same VirGL bytes to Breenix
4. If it doesn't work on Linux, fix the VirGL encoding until it does
5. Use `virgl_intercept.c` LD_PRELOAD to capture Mesa's reference bytes

**Never blame Parallels.** If Mesa works (it does, 120+ FPS), the problem is always
in our VirGL encoding. Intercept Mesa's bytes and match them exactly.

## Key Architecture Notes

- Page tables set in `parallels-loader/src/page_tables.rs` — kernel inherits them
- MAIR: idx0=Device(0x00), idx1=Cacheable(0xFF), idx2=NC(0x44)
- GOP BAR0 region (0x10000000-0x10FFFFFF) uses NC_BLOCK, rest uses DEVICE_BLOCK
- `run.sh --parallels` handles full build+deploy+boot cycle
- Manual deploy: build-efi.sh + create_ext2_disk.sh + copy HDS files + prlctl start
