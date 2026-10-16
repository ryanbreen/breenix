# GPU rendering: the `gpu` effort's design and contract

Written 2026-10-16 against `origin/main` `45ff7aa6`. This document is the design and contract of the `gpu`
effort ("GPU rendering"), which comes after `sdk` and before `breengel` in `docs/efforts/path.json`, and the
plan for rebuilding BreenGL on it. Section 5 records the rulings it needed: R2 to R8 are decided, and R1 is
pending with the operator.

**The operator's ruling, 2026-10-16:** the GPU effort comes before BreenGL lands, and the platform is GPU only.
*"I do think it's important that we reverse the order of things so that the GPU effort happens before we land
BreenGL. I just don't think a CPU rendering canvas is where I want to go with this platform. I intend for it to
be GPU only."* It followed a request the same day to see the BreenGL cases rendered on a GL canvas instead of a
scaled-up 5×7 bitmap font on a 1280×800 software-composited framebuffer.

**What the ruling decides, and what it does not.** It decides the order (`gpu`, then BreenGL rebuilt on it,
then `window-manager`, `ui-testing`, `gui-apps`) and that every pixel the platform draws is drawn by a GPU.
It does not by itself change any contract expectation. Changes this plan makes to BreenGL's contract are of
two kinds, kept apart throughout: those the GPU-only rule forces (a CPU mechanism the contract tests no longer
exists), and those that answer the request for real text and display scale, which are separate rulings
(section 5, R3 and R4, both granted).

**Status.** A plan and a contract. Nothing in it has been built or booted. Facts about the tree come from
reading it on 2026-10-16; one host computation was run (the circle closed form in section 1.5); everything
else is proposed work. The contract parts (sections 1.3 to 1.10) are completed and frozen in G0, after the
Parallels and VMware spikes report (section 1.11); until then a spike finding may change them.

**References.** `§N` is a section of BreenGL's contract, `docs/gui/breengel.md`, whose suite is
`docs/suites/breengel.json`; both are on the `breengel/suite` branch at `0585917d` (the showcase merge,
#1658). `plan §N` is a section of `docs/gui/plan.md`. "Section N" is a section of this document. #1625
(`breengel/suite` into main) and #1654 (`breengel/catchup-x86` into `breengel/suite`) are the BreenGL PRs open
when this was written.

## 0. Decisions at a glance

1. **A new effort, `gpu` ("GPU rendering"), goes into the path after `sdk` and before `breengel`.** It
   delivers the kernel GPU core, two independently written device backends (VirGL and SVGA3D DX), a
   Breenix-authored GL-style API (`libs/bgl`, "BGL"), display modes up to 2560×1600 with a scale factor,
   present, and exact device readback on ARM64 QEMU, Parallels and VMware. Its suite is `gpu` (168 cases,
   9 categories, 9 milestones).
2. **Every target renders on a hardware GPU; no software renderer counts.** Each run carries a host-side
   renderer attestation (section 1.6.5), and the gate refuses a run whose renderer is software or unknown.
   llvmpipe may exercise guest transport code on throwaway branches; it never produces a verdict.
3. **The macOS host for ARM64 QEMU uses UTM's matched graphics stack** (QEMU 10.0.12-utm with its ANGLE on
   Metal, epoxy and virglrenderer forks), built as a command-line binary that serves both the headless gate
   and the interactive window (section 1.6.1).
4. **x86-64 is out of the GPU era for now (ruling R1).** The x86 gate host is VGA only, so x86-64 is
   measured up to the shell and no further. `gpu`, `breengel`, `window-manager`, `ui-testing` and `gui-apps`
   are measured on ARM64 QEMU, Parallels and VMware (their `targets` in `docs/efforts/path.json`); x86-64
   joins them when a dedicated Breenix hardware box replaces the current x86 gate host (section 1.6.4).
5. **Exactness comes from a declared deterministic rendering profile.** Exact pipelines draw integer-cornered
   rectangles, decide membership of non-rectangular shapes with integer predicates in the fragment shader,
   using multi-word integer arithmetic wherever the contract's input range needs more than 32 bits, read
   textures with integer texel fetches, and blend in the shader with integer arithmetic against a GPU copy
   of the destination. TrueType coverage keeps the contract's existing binary32 definition and is computed
   bit-exactly on the GPU by integer emulation of binary32. No tolerance anywhere, no per-target goldens, no
   skips.
6. **The kernel owns the shaders in the first version.** Userspace submits a validated, Breenix-defined
   command stream naming pipelines from a fixed catalogue; the kernel translates it to VirGL/TGSI or
   SVGA3D/DX tokens, bounds the GPU work each submission can cause, and runs a watchdog.
7. **BreenGL is rebuilt on BGL and becomes GPU-only.** Of the suite's 393 cases, 348 keep their meaning; 25
   change and 2 go because the GPU-only rule removes what they test; rulings R3 and R4 change 15 more and
   remove 3 more. 11 new cases cover GPU composition, the render-completion handoff, CPU-rendering
   enforcement and HiDPI. The rebuilt suite has 399 cases.
8. **The CPU raster survives only as test oracle,** in `libs/libgfx-ref` (libgfx and libfont's rasterizer,
   moved unchanged) and the suites' formula code, enforced by a structure test, runtime upload accounting and
   planted violations (section 2.4). The pre-driver boot console and panic screen stay CPU-written on the
   firmware framebuffer, because they run before any GPU driver (ruling R7).
9. **Every graphical program is ported or retired before the APIs it uses go away** (section 2.6). Nothing is
   left building against a removed API or running into ENOSYS.
10. **`breengel/suite` is carried forward, not discarded.** Audited pieces the GPU effort needs are harvested
    to main first; after `gpu` lands, main is merged into `breengel/suite` and BreenGL's lanes replace its
    drawing layer. #1625 becomes a draft and lands later as the GPU-based BreenGL. #1654 stops.
11. **Build on ARM64 QEMU first** (effort lanes run on ARM64 QEMU only), with Parallels and VMware
    feasibility spikes on `scratch/*` branches cut from main in G0, before the contract freezes (ruling R2).
    The catch-up then goes Parallels, then VMware.

## 1. The `gpu` effort

### 1.1 The entry

- **id:** `gpu`
- **title:** `GPU rendering`
- **outcome:** "Breenix renders on the GPU on every target: a clean-room, GL-style Breenix API whose command
  streams the kernel validates and bounds per process and translates for VirGL (ARM64 QEMU and Parallels)
  or SVGA3D DX (VMware), executed by a hardware GPU on each host, presented at 1280×800 and at
  2560×1600 with HiDPI scaling, and read back from the device pixel-exact under a declared deterministic
  rendering profile."
- **targets:** ARM64 QEMU, Parallels and VMware (decision 4).
- **suite:** `gpu`; **subsystems:** 17 (Graphics & display), 22 (libbreenix), 25 (GUI stack). No POSIX
  `area`.

The outcome claims a GPU path, not that every program uses it: between `gpu`'s landing and BreenGL's, the
existing programs on main still draw with `fb_mmap` and `fb_draw` through the existing display path. The
"every pixel by the GPU" claim is BreenGL's outcome (section 3.2), made when those paths are removed.

The full JSON is in section 3.2.

### 1.2 Layers

```
apps, bwm, the suite stage
  └─ BreenGL (libs/breengel, libbui, libicon, libimg, libfont): shapes, text, images, widgets, windows
       └─ BGL (libs/bgl): Breenix's GL-style API: contexts, targets, textures, pipelines, draws, fences
            └─ syscall 416 `gpu(op, arg, len)` (new; 416 to 419 are free on both architectures)
                 └─ kernel/src/gpu/: handles, quotas, validation, work bounds, fences, watchdog, readback, present, display ownership
                      ├─ kernel/src/gpu/virgl/: VirGL + TGSI encoder → drivers/virtio/gpu_pci.rs (PCI virtio-gpu)
                      │      ARM64 QEMU (virtio-gpu-gl-pci), Parallels (its virtio-gpu PCI)
                      └─ kernel/src/gpu/svga/: SVGA3D DX + shader-token encoder → drivers/vmware/svga3.rs (VMware)
```

The effort renders full-screen under one display owner (the suite runner, or a case it gives the display
to). Windows, composition of several clients and the window interface stay in `breengel`; nothing here
waits on bwm.

### 1.3 BGL, the GL-style API

BGL borrows OpenGL's concepts and names none of its entry points; it claims no OpenGL profile or
conformance. It is a userspace crate (`no_std` + `alloc`, integer-only, so the soft-float x86 userspace pays
no float cost) over syscall 416.

| Object | What it is |
|---|---|
| `Context` | One per process by default (more on request, up to the quota). Owns a command buffer and its fences. |
| `Target` | A render target: width and height 1 to 16,384 (or the device's lower maximum, reported), format `B8G8R8A8_UNORM` or `R8G8B8A8_UNORM`, single sample. Also sampleable. |
| `Texture` | Sampleable only: the two target formats plus `R8_UNORM` (coverage masks, glyph atlases, bitmap-font tables). Uploaded from user memory with any stride, as 4-byte or 3-byte pixels. |
| `Buffer` | Geometry and parameter storage for one submission (rect lists, crossing lists, integer parameters). |
| `Fence` | A completion of one submission, owned by the submitting context. |

Operations: `create_target`, `create_texture`, `upload(texture, rect, bytes, stride, format, purpose)`,
`copy(src, src_rect, dst, dst_point)`, `begin(target)`, `set_clip(rect)` / `push_clip` / `pop_clip`,
`draw(pipeline, params, geometry)`, `submit() -> Fence`, `wait(fence, timeout)`,
`readback(target, rect, dst, stride) -> ObservationRecord`, `modes()`, `set_mode(mode)`,
`present(target) -> Fence`. A frame is one `submit`, so one system call carries any number of draws.
`purpose` is `Image` or `Fixture` and is counted per process (section 2.4).

**The pipeline catalogue** (version 1; each pipeline is a fixed vertex + fragment program pair the kernel
holds for both backends, with the specification sections it was written from). "Width" is the integer width
the fragment program needs for the contract's whole input range (section 1.5, item 5).

| Pipeline | Draws | Exact rule it implements | Width |
|---|---|---|---|
| `solid` | integer rectangles in one colour | coverage of half-open pixel rectangles | none |
| `copy` | a texture rectangle to a target point | integer texel fetch, format conversion by channel | 32 |
| `copy_scaled` | integer-factor replication | texel `(floor((x − x₀)/n), floor((y − y₀)/n))` | 64 |
| `nearest_scaled` | arbitrary-size nearest scaling | §23.4's `floor(xW/w)` mapping | 32 |
| `box_filter` | arbitrary downscale by bin means | §23.3's bin formula, rounded half up | 64 |
| `over` | premultiplied source over destination | `div255` source-over (§2) against a destination copy | 32 |
| `over_straight` | straight-alpha source | premultiply once, then `over` | 32 |
| `coverage` | an R8 mask blended with a colour | the §2 coverage blend; LCD variant per channel | 32 |
| `coverage_spans` | TrueType coverage from sorted sub-scanline crossings | §8.3's accumulation, `span` and `to_byte`, in emulated binary32 (section 2.1) | binary32 |
| `bitmap_text` | 5×7 and 16 px bitmap-font glyphs from their uploaded tables | §8.1's bit test and §8.2's coverage cells at integer scale, blended by §2 | 64 |
| `line` | a contract line | §5.3's walk, evaluated per pixel in closed form from the clipped start state | 96 |
| `circle_fill` | contract filled circles | §5.4's `(x − cx)² + (y − cy)² ≤ r²` | 64 |
| `circle_outline` | contract circle outlines | §5.5's set: for `r ≥ 1`, the recorded first-octant `x` at `y` is the largest `x` with `x² − x + y² ≤ r²` | 64 |
| `rounded_rect` | contract rounded rectangles | §5.6's corner predicate | 96 |
| `gradient_v` | contract vertical gradients | §5.7's integer interpolation and division | 64 |
| `triangle` | triangles with integer vertices | edge functions with an explicit top-left rule, evaluated in the shader | 96 |
| `probe_int` | one integer routine per pixel, selected by parameter | writes the routine's result bytes; used only by `profile` cases | 96 |

Geometry is integers. The kernel builds vertex positions as IEEE-754 bit patterns from integers with integer
code (exact for magnitudes below 2²⁴; every vertex is a corner of a clipped rectangle inside a target of at
most 16,384), because neither kernel target uses the FPU.

### 1.4 Syscall 416

`gpu(op: u32, arg: *mut u8, len: usize) -> isize`, the same on both architectures, with the window
interface's conventions (§17.1): `len` must equal the argument size, reserved fields zero, negative errno
returns, `copy_from_user`/`copy_to_user` everywhere with no side effect on EFAULT, and no kernel output on
any path (§22).

| Op | Name | Notes |
|---|---|---|
| 0 | `VERSION` | returns 1 |
| 1 | `CAPS` | the profile record: backend, feature bits, formats, maximum sizes, catalogue version, the device's host renderer string where its capability set carries one |
| 2, 3 | `CONTEXT_CREATE`, `CONTEXT_DESTROY` | per-process; EAGAIN past the quota |
| 4, 5 | `RESOURCE_CREATE`, `RESOURCE_DESTROY` | targets, textures, buffers; ENOMEM against the GPU memory quota and `RLIMIT_AS` for guest backing; destroy drops the handle, and storage is freed when the resource's hold set is empty (section 1.4.1) |
| 6 | `UPLOAD` | user bytes to a texture or buffer rectangle, clipped, with its `purpose`; visible to later submissions only |
| 7 | `SUBMIT` | a BGL command stream, validated whole before anything is sent (EINVAL with the failing command's index), with its work estimate checked (EDQUOT past the cap); returns a fence |
| 8 | `WAIT` | fence wait with timeout; ETIMEDOUT, EINTR, EIO for an errored or reset fence; killable within 250 ms |
| 9 | `READBACK` | a target rectangle into user memory, with a 1,024-byte `ObservationRecord` version 2 (section 1.8) |
| 10, 11 | `MODES`, `SET_MODE` | display owner only for `SET_MODE` |
| 12 | `PRESENT` | display owner only; returns the present fence |
| 13 | `COUNTERS` | per-context submissions, draws, instances, fragment area, work estimate, uploads by purpose, readbacks |

**Dispatch without a Tier-1 edit.** 416 is not added to `SyscallNumber`; it is dispatched in
`dispatch_numbered` (`kernel/src/syscall/mod.rs`, about line 889), which both architectures' entry paths
already fall through to for unlisted numbers (`kernel/src/syscall/handler.rs` line 563 on x86-64,
`kernel/src/arch_impl/aarch64/syscall_entry.rs` line 187 on ARM64). No Tier-1 file changes in this effort.
The harvest's x86-64 dispatch of 415 takes the same route (section 3.3); the earlier `handler.rs` change for
414 and 415 (`652a4494`) was reverted (`0f53872e`) and is not reapplied.

#### 1.4.1 Submission, completion and lifetime

- **Validation atomicity.** A stream that fails validation sends nothing to the device. This is a property of
  validation only; it says nothing about execution.
- **Execution and publication.** Every target a submission writes is *pending* until the submission's fence
  completes. Completed OK, the target's generation advances and its contents are published. Completed with a
  device error, or lost to a watchdog reset, every target the submission wrote is marked *invalid*: readback
  returns EIO naming the submission, sampling it in a later submission is refused, and it becomes valid again
  only when a later submission writes it whole (a full clear, a full copy or a full upload). No partial result
  of a failed submission is ever observed as valid.
- **The work estimate.** For each draw the validator computes fragments × per-fragment steps, where the
  per-fragment steps are a constant of the pipeline, except `coverage_spans`, whose steps for a row are the
  number of crossings on that row's 8 sub-scanlines (read from the buffer's per-sub-scanline offsets in
  `O(rows × 8)`). Limits per submission: 65,536 draws, 1,048,576 instances, 64 MiB of buffers, 2³¹ estimated
  fragment-steps; per context, 4 submissions in flight. Every shader loop has a trip count fixed by the
  pipeline or clamped by a validated parameter, so the estimate bounds the device work.
- **The watchdog.** A submission whose fence has not completed 2 s after it was sent is treated as hung: its
  context is quarantined (its pending fences complete with EIO `device-hung`, its targets become invalid,
  further submissions to it are refused) and the kernel destroys the device context. If the device's queue
  makes no progress for another 2 s, the kernel resets the device (virtio-gpu reset and reinitialization; the
  SVGA3 restart path the VMware corrections already carry), every context gets EIO `device-reset`, and the
  display owner and compositor recreate their resources; the console is redrawn. virtio-gpu defines no
  per-context cancellation, so destroying a context is not assumed to stop host work; only the reset is.
- **Kills.** `WAIT` returns within 250 ms of a kill, but the killed process's queued GPU work is not cancelled:
  its contexts are quarantined, and its resources stay held until their fences complete or the watchdog
  resets the device, and are freed then (`isolation/killed-work-drains`).
- **Hold sets.** A resource is freed only when its handle is dropped and its hold set is empty. Holders: each
  submission that reads or writes it (until its fence), a committed window buffer and a remnant (until
  retired), the compositor's sampling submission (until its fence), the scanout binding (until another
  target is presented and that present's fence completes), a capture or readback (until its transfer fence),
  and an `ACQUIRE` replay copy (until its fence).
- **The render-completion handoff** (used by BreenGL's windows, section 2.1): each target records the fence of
  the last accepted submission that writes it. `COMMIT` records that fence with the commit and from then on
  refuses submissions that write the committed buffer. The compositor samples a commit only after that fence
  has completed OK; the kernel waits for it on the CPU before sending the compositor's submission, because
  fences on different device contexts are not ordered against each other. A commit whose fence completes with
  an error is refused (the window keeps its previous front and the client gets the commit's failure).

### 1.5 The deterministic rendering profile

Device readback makes the pixels observable; the profile is what makes them the same on four host stacks.
This section is the profile's contract. It is completed (the width derivations and the closed forms with
their proofs) before any `pipelines` case is written, and the `profile` category checks it on each target:

1. **Targets:** single sample, 8-bit UNORM, explicit channel order and top-left origin, no sRGB encoding or
   decoding, no dithering, no mipmaps.
2. **Coverage:** exact pipelines draw only axis-aligned rectangles with integer corners. With pixel centres at
   half-integers, no centre lies on such an edge, so the APIs' tie rules never apply. No exact pipeline relies
   on the device rasterizing a non-axis-aligned edge, so the profile makes no claim about hardware triangle
   rasterization; the two-triangle split of a rectangle is checked by `rect-coverage` (every pixel inside
   written, none outside).
3. **Shapes:** every non-rectangular shape (lines, circles, rounded corners, triangles, glyph coverage, bitmap
   glyphs) is a bounding rectangle whose fragment program decides each pixel from `floor(fragment position)`
   plus integer parameters, and discards the rest.
4. **Texels:** integer-addressed fetches only (TGSI `TXF`, SM4 `ld`); no sampler filtering, wrapping or LOD in
   an exact pipeline. The validator refuses a filtered sampler in the exact profile.
5. **Numbers.** Fragment programs compute with 32-bit integer words. Where the contract's input range (i32
   coordinates, sizes and radii, targets up to 16,384) needs more, values are carried as two or three words,
   with products built from 16 × 16-bit partial products (no reliance on a device's high-word multiply) and
   division by restoring long division with a fixed iteration count, so each pipeline's per-fragment cost is
   constant. The width each pipeline needs (section 1.3's table):
   - `line`: `dx`, `dy` < 2³², the error terms < 2³⁴, the closed form's products < 2⁶⁵: three words.
   - `circle_fill`, `circle_outline`: a primitive whose clipped bounding rectangle is empty emits no draw, and
     inside it `|x − cx|, |y − cy| ≤ r < 2³¹`, so the sums are below 2⁶³: two words.
   - `rounded_rect`: `2r < 2³²`, so the squares are below 2⁶⁴ and their sum below 2⁶⁵: three words.
   - `gradient_v`: the numerator `top × (h − 1 − i) + bottom × i + (h − 1)/2` is below 2⁴⁰, divided by
     `h − 1 < 2³¹`: two words. (`255 × h` alone overflows 32 bits.)
   - `triangle`: edge functions of i32 vertices are below 2⁶⁵ in magnitude: three words.
   - `box_filter`: a bin sum is at most `2²⁸ × 255`: two words.
   Floats carry only integers below 2²⁴ (the fragment position, before `floor`) and the final conversion of an
   8-bit value `k` as `k / 255`, which both APIs' UNORM conversion rules round back to `k`.
   `profile/unorm-roundtrip` checks all 256 values per channel; `profile/integer-ops` and
   `profile/wide-arithmetic` run every routine through `probe_int` against literal results, including the
   extremes of each range; `profile/large-coordinates` runs each shape's predicate routine through `probe_int`
   at coordinates near ±2³¹, and `pipelines/line-extreme` and the shape cases draw the pipelines themselves
   there. The circle outline's closed form was checked on the host on 2026-10-16 against §5.5's walk for
   every `r` from 1 to 4,000 and 300 sampled radii up to 3,000,000, with no mismatch; its proof is added here
   before `pipelines/circle-outline` is written, and the line's closed form gets the same treatment before
   `pipelines/line-*` is written.
6. **binary32 emulation.** `coverage_spans` reproduces §8.3's f32 operations (subtract, add, multiply,
   float-to-integer truncation, `ceil`, comparisons) with IEEE-754 binary32 round-to-nearest-even implemented
   in integer words, in the operation order section 2.1 states. Native device floats are not used for
   coverage. `profile/binary32-emulation` checks the routines against literal results; a host model of the
   same routines is checked against native f32 before the shader is written.
7. **Blending.** Fixed-function blending is off in every exact pipeline. A blended draw first copies the
   union of its write rectangles to a scratch texture on the GPU (`RESOURCE_COPY_REGION` on VirGL,
   `DX_PRED_COPY_REGION` on SVGA3D), then writes `over(src, dst)` computed in integers from the copy.
   **The hazard rule:** within one blended draw the write rectangles are pairwise disjoint. BGL starts a new
   draw, with its own destination copy, at the first primitive that overlaps an earlier one in the current
   draw, and the validator refuses a blended draw with overlapping rectangles (EINVAL with its index). Draws in
   one context execute in submission order on both device APIs, so each destination copy sees every earlier
   draw's writes, and overlapping blended primitives produce the sequential source-over result
   (`pipelines/over-overlap`). Unblended overlapping primitives rely on both APIs' primitive-order guarantee
   for writes to the same pixel (`pipelines/order`).
8. **Capabilities:** the profile needs integer shader operations, integer texel fetch, the two target formats
   as render target and sampleable, `R8_UNORM` sampling and region copies. A target that lacks any of them
   fails initialization with the missing capability named, and every `gpu` case then fails with that reason.
   Nothing falls back.

A profile violation is a failing case to fix, never a reason for a tolerance.

### 1.6 Per-target backends

| Target | Device | Guest backend | Host renderer | Readback (section 1.8) |
|---|---|---|---|---|
| ARM64 QEMU (macOS host) | `virtio-gpu-gl-pci` | VirGL over the PCI transport; the kernel has an ECAM PCI enumeration path on ARM64 (`kernel/src/drivers/mod.rs`, about line 169), which G1 confirms on QEMU's `virt` machine | virglrenderer on ANGLE's Metal backend in UTM's stack (section 1.6.1) | fenced `TRANSFER_FROM_HOST_3D` |
| Parallels | its virtio-gpu PCI device with VirGL | the same VirGL backend; `gpu_pci.rs` already does capsets, contexts, fenced submission and full-scanout readback | Parallels' own, on the host's GPU | the same |
| VMware Fusion | SVGA3 | SVGA3D DX contexts and objects on `svga3.rs`'s command buffers, MOBs and GB surfaces | Fusion's own, on the host's GPU | GPU copy of the target into a dedicated readback surface, then `READBACK_GB_SURFACE` |

**VirGL object handles (both VirGL targets).** virglrenderer keeps one object-handle space per context,
shared by every object type a command stream creates: blend, rasterizer and depth-stencil states, shaders,
vertex elements, sampler views, sampler states, surfaces, queries and stream-output targets. The contract
forbids reusing a handle across types: a handle the VirGL encoder has used for one object type is never used
for an object of another type in the same context, even after the first object is destroyed, and the encoder
allocates every object handle from one per-context allocator. The G0 spike found why: when a handle was
reused across types, the host flagged a context error and dropped the creation, the draws that depended on
it did nothing, and the readback showed the target as it was before them, with no error reaching the guest.

#### 1.6.1 ARM64 QEMU on the macOS host

The stock QEMU 11.0.3 that the ARM64 gates use today has `virtio-gpu-device` and `virtio-gpu-pci` only, no
virglrenderer linkage, and its Cocoa display has no GL scanout path; no packaged build offers one. The host
work is milestone G0.

- **The stack: UTM's matched set,** pinned at UTM's dependency manifest commit
  `4d73aaf0d284f1c22e3e1c1072e729988e134268`: QEMU **10.0.12-utm** with its patch set, WebKit's ANGLE built
  for its Metal and OpenGL backends, UTM's epoxy fork, and UTM's virglrenderer fork at `451505ad`. Mixing
  current packaged builds with one patch is not equivalent and is not attempted.
- **What Breenix builds:** `scripts/build-qemu-gl.sh` runs the graphics subset of UTM's dependency build
  (ANGLE, epoxy, virglrenderer, the SPICE server and protocol) and then builds `qemu-system-aarch64` as an
  executable from the same patched source, `--target-list=aarch64-softmmu --enable-hvf --enable-cocoa
  --enable-opengl --enable-virglrenderer --enable-spice`, into a per-pin cache directory outside the tree.
  UTM builds QEMU as a shared library for its app; turning it into a command-line binary is the adaptation
  G0 did. The result is a self-contained `QEMU-GL.app` (it loads nothing from Homebrew) whose `PINS` file
  records every source pin and bundled library version, with every component's licence texts; the docs note
  is `docs/gui/qemu-gl.md`.
- **ANGLE's backend:** ANGLE renders on Metal only when `ANGLE_DEFAULT_PLATFORM=metal`, as UTM sets it;
  without it ANGLE picks its OpenGL backend. The launcher sets it.
- **The renderer context is OpenGL ES 3.0.** ANGLE gives virglrenderer an OpenGL ES 3.0 context (`GL_VERSION`
  reads `OpenGL ES 3.0.0 (ANGLE …)`), so virglrenderer runs in its GLES mode, and every pipeline in the
  profile must run on what that mode offers. The G0 spike's pipelines (integer texel fetch, 32-bit integer
  routines, an integer source-over blend against a destination copy) did.
- **Two frontends, one binary.** UTM's QEMU fork has a GL path in its Cocoa display, drawing GL scanouts
  through ANGLE, so no separate viewer is needed:
  - *Headless (every gate):* `-display none -spice unix=on,addr=<socket>,disable-ticketing=on,gl=es`, with
    no client attached. G0 found that rendering and `TRANSFER_FROM_HOST_3D` proceed with no client, so no
    null client is needed.
  - *Interactive (`run.sh`, `scripts/boot-interactive.sh`):* the same binary with `-display cocoa,gl=es`.
    The bundle is marked `NSHighResolutionCapable`, and UTM's Cocoa display sizes its window at guest pixels
    divided by the backing scale, so a 2560×1600 guest fills a 1280×800-point window at full resolution.
- **Signing and App Nap:** the bundle is ad-hoc signed with `com.apple.security.hypervisor` after every
  install-name change, with App Nap disabled as the gates' QEMU app bundle has done since #1397.
- **Fallback (not needed):** UTM's stack became a command-line binary, so the fallback routes (upstream QEMU
  11.0.3 with SDL's GL on Apple's desktop OpenGL 4.1, or a Darwin headless ANGLE display written from
  scratch) were not taken.
- **Launcher changes:** `run.sh`, `scripts/boot-interactive.sh` and the ARM64 gate scripts gain the GL
  binary and `-device virtio-gpu-gl-pci`. The `gpu` suite's boots use it from G1. Every other ARM64 gate
  switches to it in `gpu`'s landing PR, only after one run of every ARM64 suite on the new binary matches
  main's result on the current one; a difference is fixed before landing, never waived (ruling R5).
- **Acceptance for G0**, all on the same binary, each with the guest workload described next, in **both**
  frontends:
  1. the binary enumerates `virtio-gpu-gl-pci`, and its signature verifies with the entitlement shown;
  2. a throwaway Breenix spike guest uploads an `R8_UNORM` and a `B8G8R8A8_UNORM` texture, draws a quad that
     reads them by integer texel fetch, a quad whose fragment program writes integer-routine results
     (including a two-word multiply), and a blended quad against a GPU destination copy, then reads the target
     back with `TRANSFER_FROM_HOST_3D`; every pixel equals its formula, and all 256 UNORM values round-trip;
  3. the host renderer attestation for those runs (section 1.6.5) names the Apple GPU through ANGLE's Metal
     backend, and the guest's capset renderer string, where the capset carries one, agrees with it;
  4. the same binary forced onto a software renderer runs the same guest, and the gate refuses that run
     naming the software renderer. WebKit's ANGLE has no SwiftShader backend, and Mesa's llvmpipe cannot
     stand in (UTM's SPICE and Cocoa displays need ANGLE's IOSurface support), so the planted run uses
     Apple's software OpenGL renderer through ANGLE's OpenGL backend
     (`scripts/qemu-gl/plant-software-renderer.sh`, booted with an empty `ANGLE_DEFAULT_PLATFORM`).

#### 1.6.2 Parallels

The smallest guest change. `gpu_pci.rs` and `virgl.rs` already encode surfaces, shaders, vertex state,
viewports, scissors, draws, samplers and blits, and read back the scanout with a fenced transfer. The work:
per-process VirGL contexts with only that process's resources attached (`CTX_ATTACH_RESOURCE`), owned
render-target and texture resources, the catalogue's TGSI programs, atomic validation in place of the
encoder's silent stop at capacity, the profile checks against Parallels' capset, and the 2560×1600 display
setting in `scripts/parallels/`. Parallels is treated as hardware: behaviour that looks wrong is first
reproduced on a Linux guest used as a black-box probe, and a Breenix defect is assumed until the probe shows
otherwise.

#### 1.6.3 VMware Fusion

The largest guest change: `svga3.rs` has command buffers, object tables, MOBs, GB surfaces and screen
targets, and no DX context, shader, view or draw. The backend, in order:

1. Read the DX capability and the device capability entries for every profile format (render target and
   shader-resource view), and fail initialization naming any that is missing.
2. Define and bind a DX context with its context object tables (`DX_DEFINE_CONTEXT` 1143,
   `DX_BIND_CONTEXT` 1145, `DX_SET_COTABLE` 1207) on MOB backing sized from the published structures.
3. Surfaces with render-target and shader-resource flags; render-target and shader-resource views; vertex
   and parameter buffers.
4. The catalogue's programs in the shader token format VMware's DX shader commands accept, written from
   VMware's published headers and Microsoft's published token format, with their input and output
   signatures.
5. Draw (`DX_DRAW` 1152), then present by binding the target's surface to the screen target if the device
   accepts that surface, or by a GPU copy into the bound screen-target surface otherwise.
6. Readback through the host-copy sequence recorded in §20.2 (section 1.8).

**Feasibility is settled in G0, not at the last catch-up.** The VMware spike (section 1.11) exercises the
whole profile, not one triangle: whether the device takes a bare token stream or a container; both target
formats as render target and shader-resource view; an `R8_UNORM` shader-resource view; uploads of both;
integer texel fetch (`ld`); integer ALU including a two-word multiply; `DX_PRED_COPY_REGION` of a destination
followed by a shader blend reading the copy; present to the screen target; and exact readback of the rendered
target through the host copy. This document's contract freezes only after this spike and the Parallels spike
report; a profile property VMware cannot meet changes the shared design then, not after the ARM64 lanes.

#### 1.6.4 x86-64

x86-64 is out of the GPU era for now (ruling R1). The current x86 gate host has no hardware GPU a guest
renderer can use: the gate's QEMU runs inside a virtual machine whose virtio GPU negotiates without
`VIRTIO_GPU_F_VIRGL`, and that QEMU (8.2.2) failed to load its GL display module in a help probe
(`undefined symbol: qemu_egl_display`). Its display stays VGA only, and x86-64 is measured up to the shell and
no further. The GPU-era efforts list ARM64 QEMU, Parallels and VMware as their targets, so x86-64 neither
counts toward them nor holds them back.

x86-64 joins those efforts when a dedicated Breenix hardware box replaces the current x86 gate host. Its GPU
path is designed for that box then, to the bar every other target meets: a hardware GPU, a host renderer
attestation (section 1.6.5) that refuses a software renderer, and an x86-64 spike whose acceptance is G0's,
item for item. The guest side it will need is known now: `gpu_pci.rs`'s modern PCI transport, queues, DMA
allocation and cache handling, ARM64-gated today, generalized to x86-64, with the kernel kept free of FPU use
(section 1.3). Until the box is in place no x86-64 GPU work runs, on effort lanes or on `scratch/*` branches.

#### 1.6.5 Host renderer attestation

Nothing in the device protocols lets the guest tell a GPU from a software renderer, so the hardware claim
rests on host-side evidence that the gate checks, recorded with every run:

- **ARM64 QEMU (on the macOS host).** A Breenix host patch in QEMU's VirGL glue writes the
  `GL_VENDOR`, `GL_RENDERER` and `GL_VERSION` of the context that executes the guest's commands to
  `<run dir>/host-renderer.json`, at the creation of each renderer context and at its first `SUBMIT_3D`. It
  comes from the same process and context as the guest's workload, not from a separate probe. The gate
  (`breenix-runs` and the ARM64 gate scripts) refuses a run whose file is missing, records no first
  `SUBMIT_3D` (creation alone says nothing about the renderer that executed the work), records a submission
  without its context's creation before it, names a software renderer
  (llvmpipe, softpipe, lavapipe, SwiftShader, or any renderer string containing "Software"), or names a
  renderer other than the target's declared one, and requires the guest's capset renderer string, where the
  capset carries one, to agree.
- **Parallels and VMware.** There is no host hook. The gate checks the VM's configuration has 3D acceleration
  on and records the host GPU time attributed to the hypervisor's processes over the `performance` category,
  from macOS's accelerator statistics. The G0 spikes establish the mechanism and its negative control (the
  same reading with 3D acceleration off). Where no host measurement separates GPU from software rendering,
  the configuration check is the basis of that target's hardware claim, and the record labels it so (ruling
  R6).
- **The boundary:** the record's boundary code 4 states that the host renderer and host window are outside
  the guest's view and names the attestation that covers the renderer.

### 1.7 Display modes, HiDPI and present

- **Modes.** Every target offers 1280×800 (scale 1) and 2560×1600 (scale 2); `MODES` lists them with the
  device's own maximum. The `gpu` suite tests both. Every GUI suite boot uses 2560×1600 at scale 2, and
  1280×800 stays a tested mode (ruling R4). The mode is chosen by the guest (the scanout resource or screen
  target it creates); host window scaling is outside the guest's view and outside every claim.
- **Scale.** The scale factor is published with the mode. BreenGL lays out in logical units and multiplies by
  the scale. UI text is DejaVu Sans (ruling R3), rasterized from outlines at the physical pixel size (a 16 px
  string at scale 2 from the outline at 32 px, never replicated from a 16 px raster).
- **Present.** Two scanout targets per display owner. `PRESENT` binds the drawn one (`SET_SCANOUT` and
  `RESOURCE_FLUSH` on virtio-gpu; the screen-target update on SVGA3) and returns a fence that completes when
  the device has answered. There is no vblank on these devices; the present rate is measured, not assumed.
  A minimal scanout bind of the current case's target arrives in G2 (section 1.11), so runs are watchable from
  then; modes, flipping and the ownership stack arrive in G7.
- **Ownership.** The display ownership stack (`take_over_display` 431, `give_back_display` 432, §18) keeps
  each owner's last presented target; giving the display back presents the previous owner's target again,
  exactly. After the GPU driver starts, the kernel's console is drawn by the GPU from a glyph atlas uploaded
  from its built-in font tables (`purpose` `Fixture`) through `bitmap_text`. Before it starts, the boot
  console and panic screen are CPU-written on the firmware framebuffer, and nowhere else (ruling R7).

### 1.8 Observation by device readback

`READBACK` and BreenGL's `SCANOUT_READBACK` share one record, `ObservationRecord` version 2: 1,024 bytes,
little-endian, reserved bytes zero, produced atomically with the pixels it describes.

| Offset | Field |
|---|---|
| 0 | `version: u32` (2), `size: u32` (1,024) |
| 8 | `serial: u64`: this observation, monotonic per device |
| 16 | `backend: u32`: 4 VirGL, 5 SVGA3D DX (1 virtio-gpu 2D, 2 GOP and 3 CPU-written SVGA3 screen target are retired) |
| 20 | `boundary: u32`: 4, the host renderer and host window are outside the guest's view; the renderer is covered by the run's host attestation (section 1.6.5) |
| 24 | `flags: u32`: bit 0 the observed target is the scanout, bit 1 the observation is of a composite; other bits invalid |
| 28 | `format: u32`: the target's format (0 `B8G8R8A8`, 1 `R8G8B8A8`) |
| 32 | `x, y, width, height: u32`: the rectangle read |
| 48 | `target_width, target_height: u32` |
| 56 | `stride: u32`: of the caller's buffer; `canonical: u32`: 0, BGRA bytes |
| 64 | `context: u64`: the device context that last wrote the target (the compositor's for a composite) |
| 72 | `target: u64`: the caller's handle; `resource: u64` at 80: the device's resource or surface id |
| 88 | `generation: u64`: the target's published generation (section 1.4.1) |
| 96 | `render_serial: u64`: the submission that produced that generation; `render_fence: u64` at 104 |
| 112 | `render_status: u32`, `render_kind: u32`: the answer and the command answered (`SUBMIT_3D` on VirGL; the command buffer's status and `DX_DRAW` on SVGA3D) |
| 120 | `readback_fence: u64`; `readback_status: u32`, `readback_kind: u32` at 128 (`TRANSFER_FROM_HOST_3D`; `READBACK_GB_SURFACE`) |
| 136 | `readback_surface: u64`, `readback_mob: u64` at 144: VMware's dedicated readback surface and its MOB; 0 on VirGL |
| 152 | `copy_kind: u32`, `copy_status: u32`: VMware's host copy into the readback surface; 0 on VirGL |
| 160 | `poison: u32`; `poisoned_remaining: u32` at 164, which must be 0 |
| 168 | `commit: u64`, `composite_serial: u64` at 176: for a window or composite observation, else 0 |
| 184 | `present_fence: u64`; `present_status: u32`, `window_count: u32` at 192 |
| 200 | `windows: [(window: u64, commit: u64, render_fence: u64, generation: u64); 16]`: every window in the composite, the commit sampled, the render fence that commit carried and the generation sampled |
| 712 | `renderer: [u8; 64]`: the device's renderer string from its capability set, NUL-padded, empty if none |
| 776 | reserved |

A readback refuses with EIO and a named reason when a composed window's render fence had not completed OK
before the composite was sent, when any recorded fence belongs to another submission, when the device's
binding differs from the observed resource, or when the target is invalid (section 1.4.1). More than 16
windows in a composite returns EIO without a partial record, as v1 does.

| Target | Sequence the guest runs |
|---|---|
| VirGL (three targets) | poison the guest backing; fenced `SUBMIT_3D` of the frame; fenced `TRANSFER_FROM_HOST_3D` of the read rectangle into the attached backing; invalidate the CPU cache over it; refuse any pixel still poisoned where the contract allows a distinguishable poison |
| SVGA3D DX | complete the draw; GPU `SURFACE_COPY` from the judged target into a dedicated readback surface on its own MOB, in the same command buffer; clear the MOB; `READBACK_GB_SURFACE`; device fence; invalidate and read the MOB; refuse unwritten pixels |

The VMware row extends the sequence BreenGL recorded for host-modified contents (§20.2): `READBACK_GB_SURFACE`
writes nothing for a surface whose contents came only from guest uploads, and a host copy first makes it write
the whole surface. Readback of a shader-rendered target is one of the VMware G0 spike's findings.

**Planted faults** (test controls, §21, new points 19 to 31), each with a `gpu` case that requires the exact
reason and then the clean result once disarmed: 19 `skip-transfer` (a skipped transfer-back), 20
`stale-fence` (a fence answered for another submission), 21 `wrong-resource` (readback of another resource),
22 `corrupt-pixel` (a pixel outside the draw corrupted on the device), 23 `mask-capability`, 24
`profile-violation` (one draw with filtered sampling), 25 `skip-present-flush`, 26 `wrong-scanout`, 27
`device-error` (a forced device error, escalating to a reset when armed twice), 28 `suppress-draw` (the
translator drops one draw from the device stream while the fence still completes), 29 `wrong-texture` (one
draw binds another texture the same process owns), 30 `skip-upload-visibility` (one upload's transfer to the
host is omitted), 31 `withhold-fence` (one fence's completion is ignored, so the watchdog fires; a real host
hang cannot be induced from inside the profile, whose shader loops are all bounded, so this point simulates
one).

There is no planted missing-cache-invalidation fault: on every target the "device" is host software
writing guest memory through the host's coherent memory system, so omitting the invalidation changes no
bytes. The invalidation stays as required code, and a structure test checks that every readback path calls
it between the transfer fence and the first read.

### 1.9 Clean room and provenance

The rule is the libc rule applied to graphics. **Allowed:** the VirtIO 1.3 specification; the VirGL protocol
definition header as interface facts (command numbers, field layouts); the TGSI documentation; VMware's
published device headers (`svga3d_cmd.h`, `svga3d_dx.h`, `svga3d_devcaps.h` and their siblings in
`device_include`); Microsoft's published shader token format; the OpenGL and Direct3D specifications for
rasterization, conversion and precision rules; IEEE 754 for binary32; Breenix's own code; black-box
experiments (a Linux VM run as a device-behaviour probe, recorded as results). **Forbidden, including while
debugging:** reading, porting or adapting Mesa (its VirGL and SVGA drivers, Gallium, the state trackers),
virglrenderer's renderer sources, Linux's virtio-gpu or vmwgfx driver C files, and ANGLE's implementation;
and using byte sequences captured from another guest's driver as templates. Host software (QEMU,
virglrenderer, ANGLE, Mesa as the host renderer) runs unmodified or with Breenix's own host patches and is
never linked into the guest. Host-stack work reads only build files and QEMU's and UTM's display and
frontend integration code, and its lanes do not author guest GPU code.

`docs/planning/gpu-rendering-attack-plan.md` proposes capturing and porting exact byte sequences; that
approach is void.

**The provenance audit comes first and gates reuse.** It is the first G0 task, and it covers every file the
effort or the harvest reuses: `virgl.rs`, `gpu_pci.rs`, `svga3.rs` and the rest of `drivers/vmware/`, every
shader program text or token array already in the tree, the libgfx algorithms that move to `libgfx-ref`, and
libfont's rasterizer. Each encoding, structure layout, shader and algorithm gets its source (a section of an
allowed document, or Breenix's own design); anything whose only source is a captured stream or a forbidden
implementation is rewritten from an allowed source before reuse. The audit's record per file lives in
`docs/gui/provenance/`. A harvest PR merges only after its files' audit record has merged, and every PR in the
effort lists the sources its new code was written from, which review checks against the forbidden list.

### 1.10 Suite `gpu`: outline

`/sbin/suite-gpu`, source `userspace/programs/src/suite_gpu/`, manifest `docs/suites/gpu.json`, contract this
document, zero skips, gate timeout 900 s on every target until measured (plan §19's rule). Every case runs on
every target; a case whose evidence differs per backend names the backend's row, as `breengel`'s
`scanout/device-path` does. Expectations are literal tables or formulas computed in the suite crate, or
`libgfx-ref` output where section 2.4 allows it; none comes from BGL or the kernel.

**No CPU canvas in the suite.** The suite does not use `Suite::gui()`, `suite_stage` or
`suite::judged(&FrameBuf)`, which rasterize and compare CPU framebuffers. It judges with
`suite::judged_readback(&Readback, &Expected)` over read-back bytes and their record (comparison, not
rendering). From G2 each rendering case binds its target to the scanout while it owns the display, and the
runner draws a status strip of solid tiles (one per case, pass or fail colour) with BGL's `solid` pipeline;
case names stay on serial. Text on the stage arrives with BreenGL's GPU stage.

| Category (milestone) | Cases | Case ids |
|---|---|---|
| `device`: 3D device & capability profile | 12 | `version`, `feature-negotiated`, `caps-parsed`, `profile-formats`, `profile-integer-ops`, `profile-limits`, `profile-record`, `missing-capability-fails`, `no-fallback`, `display-info`, `backend-named`, `device-error-reported` |
| `submit`: command submission & fences | 22 | `context-create`, `context-destroy`, `context-limit`, `stream-validate`, `validation-atomic`, `stream-overflow`, `fence-order`, `fence-wait`, `wait-timeout`, `wait-eintr`, `wait-killable`, `fence-foreign`, `multi-context`, `ring-backpressure`, `device-error-eio`, `recovery-after-error`, `frame-one-syscall`, `freed-resource-refused`, `fence-echo-checked`, `counters`, `watchdog-quarantine`, `device-reset-recovers` |
| `readback`: device readback | 19 | `full-target`, `poison-unwritten`, `scenes-aba`, `partial-rect`, `padded-stride`, `format-canonical`, `record-fields`, `device-path`, `fence-identity`, `ordered-after-draws`, `fault-skip-transfer`, `fault-stale-fence`, `fault-wrong-resource`, `fault-corrupt-pixel`, `fault-suppress-draw`, `error-invalidates-targets`, `owner-only`, `efault`, `large-target` |
| `textures`: textures & uploads | 18 | `create-formats`, `create-limits`, `upload-full`, `upload-subrect`, `upload-clip`, `upload-stride`, `upload-3byte`, `upload-visibility`, `copy-region`, `copy-overlap`, `immutable-while-pinned`, `destroy-deferred`, `quota`, `r8-atlas`, `readback-uploaded`, `efault-upload`, `fault-wrong-texture`, `fault-skip-upload` |
| `profile`: deterministic rendering profile | 22 | `clear-exact`, `unorm-roundtrip`, `texel-fetch-exact`, `integer-ops`, `wide-arithmetic`, `binary32-emulation`, `int-float-exact`, `fragcoord-integer`, `rect-coverage`, `rect-edges`, `scissor-exact`, `viewport-exact`, `alpha-write`, `swizzle-bgra-rgba`, `no-dither`, `no-srgb`, `determinism`, `cross-context`, `large-coordinates`, `filtered-sampler-refused`, `fixed-blend-off`, `profile-violation-detected` |
| `pipelines`: drawing pipelines | 26 | `solid-rects`, `rect-batch`, `copy`, `copy-clip`, `copy-scaled`, `over-premultiplied`, `over-straight`, `over-overlap`, `coverage-blend`, `coverage-lcd`, `coverage-spans-square`, `coverage-spans-winding`, `coverage-spans-corpus`, `bitmap-text`, `line-bresenham`, `line-clip`, `line-extreme`, `circle-fill`, `circle-outline`, `rounded-rect`, `gradient`, `triangle-integer`, `box-filter`, `nearest-scale`, `clip-nested`, `order` |
| `isolation`: per-process isolation | 25 | `handle-private`, `handle-guess`, `fork-no-handles`, `exec-reclaims`, `exit-reclaims`, `kill-mid-submit`, `killed-work-drains`, `quota-memory`, `quota-handles`, `quota-contexts`, `work-estimate-refused`, `foreign-target-draw`, `foreign-readback`, `display-context-unreachable`, `no-addresses`, `fuzz-streams`, `bounded-validation`, `efault-submit`, `efault-readonly`, `quiet-errors`, `unknown-op`, `test-hooks-absent`, `killable-wait`, `client-cannot-scanout`, `irq-masked-bound` |
| `present`: modes, HiDPI & present | 16 | `modes-listed`, `mode-1280x800`, `mode-2560x1600`, `scale-reported`, `scanout-bind`, `flip`, `present-completion`, `present-owner-only`, `readback-presented`, `takeover-giveback`, `ownership-stack`, `console-restored`, `firmware-handoff`, `mode-change-content`, `fault-skip-flush`, `fault-wrong-scanout` |
| `performance`: frame rate & latency | 8 | `clear-rate-2560`, `rect-throughput`, `submit-latency`, `readback-time-2560`, `upload-bandwidth`, `coverage-throughput`, `wait-no-spin`, `present-rate-2560` |
| **Total** | **168** | |

Notes on the cases that carry the most weight:

- `profile/unorm-roundtrip` writes every 8-bit value through each channel and reads it back; `integer-ops`,
  `wide-arithmetic` and `binary32-emulation` run each routine through `probe_int` against literal results at
  the ends of its range; `determinism` and `cross-context` render the same stream twice and require identical
  bytes.
- `pipelines/over-premultiplied` renders all 65,536 (colour, alpha) pairs over four backgrounds and compares
  with §2's formula; `over-overlap` draws overlapping blended primitives in one BGL draw call and requires
  the sequential result. `line-bresenham`, `circle-outline` and `coverage-spans-square` use literal tables;
  `coverage-spans-winding` and `coverage-spans-corpus` compare with `libgfx-ref`'s rasterizer bit for bit,
  the latter over every printable glyph of the pinned fonts at the five reviewed reference sizes.
- `readback/fault-suppress-draw` and `textures/fault-wrong-texture` show a completed fence is not proof the
  intended command ran: each must fail the exact comparison.
- `isolation/fuzz-streams` submits 10,000 seeded malformed streams; each must be refused with EINVAL and its
  failing index, the device must stay alive, and the next valid frame must read back exactly.
  `work-estimate-refused` submits a stream one step over the cap. `irq-masked-bound` requires the longest
  IRQ-masked interval in the kernel's GPU paths (validation, translation, submission, upload and readback
  copies, fence completion, the watchdog) under a full-screen workload to stay within 500 µs, after the
  existing `irq-hold` point has shown the gauge reads a 2 ms hold, as `breengel`'s
  `composition/flush-irq-bound` does.

**Performance bounds,** in the contract from G0 and held on every target. G8 may tighten a bound from
measurement and never loosens one; a target that misses a bound has a slow path to fix.

| Case | Bound |
|---|---|
| `clear-rate-2560` | at least 60 full-screen 2560×1600 clears, each presented, per second |
| `rect-throughput` | 10,000 solid 16×16 rectangles in one submission complete within 50 ms |
| `submit-latency` | an empty submission's fence: median at most 2 ms, maximum at most 10 ms over 1,000 |
| `readback-time-2560` | a full 2560×1600 readback within 100 ms |
| `upload-bandwidth` | a 16 MiB texture upload visible to the next submission within 100 ms |
| `coverage-throughput` | a full-screen 2560×1600 `coverage` blend from an R8 mask within 16.7 ms |
| `wait-no-spin` | CPU time while waiting on a fence at most 5% of the wait |
| `present-rate-2560` | at least 60 presents per second alternating two full-screen targets |

### 1.11 Milestones and build order

**G0 (host, audit, spikes and contract; measured by its acceptance checks, not by the suite).** In this
order where one depends on another:

1. the provenance audit (section 1.9), then the harvest PRs to main as their files' audits merge (section 3.3);
2. the macOS GL host stack and its acceptance (section 1.6.1), with the attestation patch (section 1.6.5);
3. the entry spikes, one throwaway boot sequence per non-QEMU target on a `scratch/*` branch cut from main,
   so no effort-lane commit is involved and the target guard passes (ruling R2): Parallels (the profile's
   capability set on a client render target, read back exactly) and VMware (the whole profile, section
   1.6.3);
4. this document's contract, completed and frozen after the spikes report: BGL, syscall 416, the profile with
   its width derivations and closed-form proofs, the catalogue with each program's specification citations,
   record v2, the test-control points and the performance bounds;
5. `docs/suites/gpu.json` and the suite with all 168 cases, each failing with a specific "missing …" reason.

The `gpu` entry in `docs/efforts/path.json` (section 3.2) lands with this document. Until `docs/suites/gpu.json`
exists, Vigil shows the effort's milestones as not measured yet.

**Lanes on ARM64 QEMU (in order; each milestone is its category passing on ARM64 QEMU, and each step supplies
everything its category's cases use):**

| Step | Milestone | Work |
|---|---|---|
| G1 | `device`, `submit` | `virtio-gpu-gl-pci` on the PCI transport; the existing PCI virtio-gpu display path (the one Parallels uses) running on QEMU so main's programs keep working; the kernel GPU core (`kernel/src/gpu/`): contexts, handles, validation, the work estimate, fences, waits, the watchdog and reset; syscall 416 on ARM64 |
| G2 | `readback` | `READBACK` with record v2 and fault points; `solid`; target invalidation; the minimal scanout bind and status strip |
| G3 | `textures` | uploads with purposes, region copies, `copy` |
| G4 | `profile` | `probe_int` with the integer, multi-word, binary32 and shape-predicate routines; the profile checks |
| G5 | `pipelines` | the rest of the catalogue in TGSI, the destination copy and hazard rule, BGL's draw API |
| G6 | `isolation` | quotas, reclamation at exit/exec/kill, the fuzzer, quiet errors, no addresses, the IRQ gauge over the GPU paths |
| G7 | `present` | modes, scale, two-target present, the ownership stack on the GPU, the kernel console on the GPU, the firmware handoff |
| G8 | `performance` | measure against the bounds, fix the slow paths |

**Catch-up (each target until all 168 pass there, with its attestation):** Parallels (the same backend;
Parallels capset and display configuration), then VMware (the SVGA3D DX backend and its programs, designed
against the G0 spike). x86-64 is not in the catch-up (section 1.6.4). The effort is done when one run per
target has all 168 passing with an accepted attestation; `gpu`'s landing PR also switches every ARM64 gate to
the GL binary (section 1.6.1, ruling R5).

**Sizing (planning guesses, not measurements and not budgets).** The `breengel` effort went from its plan
(2026-10-15) to 393/393 on three targets (2026-10-16), with much of its library code already in the tree;
most of this work's code does not exist yet, and the items below are what makes it larger.

| Item | Guess |
|---|---|
| G0 host stack: UTM's build turned into a CLI binary, the attestation patch, signing, both frontends | 1 to 2 weeks (2 more if the SDL fallback is needed) |
| G0 audit, spikes (VMware's covering the whole profile), contract and suite skeleton | 1 to 2 weeks, partly in parallel with the host stack |
| G1–G2: kernel core, validation, work estimate, watchdog and reset, readback | 1.5 to 3 weeks |
| G3–G5: uploads, the catalogue in TGSI including multi-word arithmetic, binary32 emulation, destination copies and the hazard rule | 2 to 4 weeks |
| G6–G8: isolation, present and HiDPI, performance at 2560×1600 (four times the pixels of 1280×800) | 1.5 to 3 weeks |
| Catch-up: Parallels; VMware's DX backend and SM4 programs | 0.5 to 1; 2 to 4 weeks |
| BreenGL rebuild: drawing layer, text on `coverage_spans`, windows and GPU composition, `libgfx-ref` and its tests, porting 8 programs, catch-up on three targets | 4 to 8 weeks |
| Host stack upkeep (pins, UTM and macOS updates) | ongoing, after landing |

Together, roughly 3.5 to 7 months of calendar time to #1625 landing. These are guesses until G0 and the spikes
report, and are revised then.

## 2. BreenGL rebuilt on the GPU

### 2.1 What changes in the architecture

- **Drawing.** Every BreenGL primitive becomes BGL draws: rectangles to `solid`, lines/circles/rounded
  rectangles/gradients to their predicate pipelines, blits to `copy`/`copy_scaled`/`over`/`over_straight`,
  clips to scissors, bitmap-font text to `bitmap_text`, TrueType text to `coverage_spans` into a GPU atlas
  and `coverage` quads. BreenGL computes geometry on the CPU (clipping in `i64`, the line's start state at its
  first in-target step, glyph outlines flattened to segments, and their sub-scanline crossings) and no
  per-pixel values.
- **Surfaces become targets.** `FrameBuf` over memory is replaced by `Surface` over a BGL target. Pixels reach
  the CPU only through readback, for checks, captures and encoding.
- **Windows.** A window's 2 or 3 buffers become GPU targets. `ACQUIRE`'s kernel replay becomes a GPU region
  copy of the replay set; committed-buffer immutability becomes the validator refusing any submission that
  writes a front or retired buffer, with the render-completion handoff of section 1.4.1 covering submissions
  accepted before `COMMIT`; `CAPTURE` becomes a device readback of the committed target; remnants hold the
  target. The commit protocol, damage log, authority table, events and metadata are otherwise unchanged.
- **Composition.** The kernel compositor composes every visible window by GPU draws sampling the clients'
  front targets into the scanout target (the §18 formula: topmost window's pixel or black, damage only), with
  the cursor drawn as a top layer so readback sees it. The software compositor
  (`kernel/src/graphics/display.rs`'s backends for virtio-gpu 2D, GOP and the SVGA3 MOB) is deleted, as are
  `fb_mmap` (412), `fb_draw` (411, including `FLUSH` and `FLUSH_BATCH`), `fb_info` (410), `MAP_COMMITTED`
  (op 14) and `MAP_COMPOSITOR_TEXTURE` (op 21), which become ENOSYS, in the same PR as the last program
  migration (section 2.6).
- **Text keeps its contract.** `libfont` keeps parsing, hardening, flattening and §8.3's coverage definition,
  which is binary32 arithmetic in `rasterizer.rs`. The split: the CPU computes, for each row's 8
  sub-scanlines, the crossings and their winding directions exactly as `accumulate` does (the same code, in
  f32 on the CPU, including the counted merge sort; this crossing half stays in libfont, while the whole of
  `rasterizer.rs` moves to `libgfx-ref` unchanged), and writes the sorted crossing lists into a `Buffer`;
  `coverage_spans` computes each pixel's coverage from them on the GPU by walking each sub-scanline's
  crossings in order with the same winding and `fill_start` rule, adding that pixel's share of each span
  exactly as `span` does, in the same order, and converting with `to_byte`, all in emulated binary32 (section
  1.5, item 6). §8.3 gains a normative statement of that operation order, written from `rasterizer.rs`. The
  reviewed text references therefore stand unchanged. The 5×7 and 16 px bitmap fonts stay in the API and
  their cases stand, drawn on the GPU by `bitmap_text` from their tables. The stage and widgets move to
  DejaVu Sans (ruling R3).

### 2.2 The 393 cases

| Category | Cases | Keep | Change (GPU-only) | Go (GPU-only) | Change (R3, R4) | Go (R3) |
|---|---|---|---|---|---|---|
| `raster` | 40 | 33 | 7 | 0 | 0 | 0 |
| `text` | 37 | 36 | 1 | 0 | 0 | 0 |
| `font-parser` | 22 | 20 | 2 | 0 | 0 | 0 |
| `inflate` | 21 | 21 | 0 | 0 | 0 | 0 |
| `bmp` | 9 | 9 | 0 | 0 | 0 | 0 |
| `png-decode` | 31 | 31 | 0 | 0 | 0 | 0 |
| `png-encode` | 7 | 7 | 0 | 0 | 0 | 0 |
| `widgets` | 23 | 16 | 0 | 0 | 7 | 0 |
| `text-entry` | 19 | 18 | 0 | 0 | 1 | 0 |
| `icons` | 12 | 12 | 0 | 0 | 0 | 0 |
| `windows` | 39 | 32 | 7 | 0 | 0 | 0 |
| `isolation` | 38 | 35 | 3 | 0 | 0 | 0 |
| `composition` | 27 | 23 | 2 | 2 | 0 | 0 |
| `events` | 32 | 32 | 0 | 0 | 0 | 0 |
| `scanout` | 15 | 12 | 3 | 0 | 0 | 0 |
| `panel` | 21 | 11 | 0 | 0 | 7 | 3 |
| **Total** | **393** | **348** | **25** | **2** | **15** | **3** |

**Keep** means the check, its contract section and its expected values stand; only the implementation under
it changes. Every kept drawing case (all of `raster`'s shape, alpha, blit, clip and damage cases, the `text`
cases including `raster-identical`, the bitmap-font cases and the four reviewed-reference cases,
`png-decode/blit`, `icons/assets`, the `compose-*` cases) keeps its formula, table or reviewed reference and
now judges a device readback of GPU output. A pixel the GPU draws differently is a defect in a pipeline,
never a new expectation.

**Change, forced by GPU-only** (25), with how each stays exact and what it no longer covers:

- `raster` (7):
  - `guard-pages` splits into its two remaining risks: guard pages around the user buffers of `UPLOAD` and
    `READBACK` called with extreme rectangles (the kernel's copies are where a CPU write can now escape), and
    a canary target beside the drawn one, read back, for device-side writes outside the target or clip. What
    goes: CPU drawing into guard-paged surface memory, which no longer exists.
  - `extreme-work-bound` bounds the CPU geometry-setup steps by a constant per primitive and the emitted
    fragment area and work estimate (from `COUNTERS`) by the clipped bounding rectangle; a wholly clipped
    primitive emits no draw.
  - `stride-padding` moves to padded-stride upload and readback buffers.
  - `surface-validate` and `surface-owned` move to target creation (`Null` and `Misaligned` go, a
    device-maximum `TooLarge` and an unsupported `Format` stay) and to the GPU memory counter returning to
    baseline.
  - `bpp3-bpp4`: 3-byte render targets go, because no 3-byte format is renderable in the profile. 3-byte
    upload and 3-byte readback conversion stay as two checks in the case.
  - `copy-row-clip`: `copy_row_from` is a CPU memory primitive and leaves the API, including its
    byte-granular partial-pixel semantics, which have no GPU meaning. The case becomes pixel-granular upload
    rectangle clipping, never past the row's end.
- `text` (1): `size-range` keeps its sizes and refusals; its rasterization bound becomes the split bound below.
- `font-parser` (2): `work-bound` and `coords-extreme`. The load, outline and flattening bounds are unchanged.
  The rasterization bound splits in two: the CPU's crossing and sort steps (§7's formula without the span-fill
  term `10C`) by libfont's counter, and the GPU's fragment-steps by the work estimate, which for the
  independent 1,024 × 1,024 square must equal its formula exactly, so a counter that skips the span fill is
  still caught. The contract recomputes both numbers; `GlyphTooExpensive` is still refused before any work.
- `windows` (7): `create` returns target handles, not mapped addresses; `destroy` frees targets;
  `fork-no-mapping` becomes no inherited GPU handles; `commit-immutable` becomes a refused submission to a
  front buffer with the front unchanged on readback, including a submission accepted before `COMMIT`;
  `resize-rlimit` and `rlimit-accounting` count guest backing against `RLIMIT_AS` and device memory against
  the GPU quota; `every-target` names backend 4 or 5.
- `isolation` (3): `other-map` and `texture-map` become refused foreign handles and a refused sample of the
  compositor's scanout target; `retired-ops` adds 410, 411, 412, `MAP_COMMITTED` and `MAP_COMPOSITOR_TEXTURE`.
- `composition` (2): `fbinfo-geometry` becomes the mode and scale from `MODES`. `flush-irq-bound` becomes
  `irq-bound`: the same 500 µs bound and `irq-hold` gauge, over the GPU composition path (validation,
  translation, submission, fence completion and readback copies) during full-screen damage.
- `scanout` (3): `mode-set` checks the mode reported by `MODES` instead of `fb_info` (2560×1600 at scale 2,
  ruling R4); `device-path` and `boundary-stated` use record version 2's rows and boundary code 4.

**Go, forced by GPU-only** (2): `composition/fbmmap-pane` and `flush-clip` (`fb_mmap` and `FLUSH` no longer
exist).

**Change and go with rulings R3 and R4** (15 and 3), which answer the request for real text and display scale
rather than the GPU-only rule; both rulings were granted:

- R3, UI and stage text in DejaVu Sans: `widgets` (7) `button-draw`, `button-disabled`, `checkbox-toggle`,
  `label-clip`, `focus-ring`, `menubar-open` and `menu-disabled` take label positions and widths from DejaVu
  Sans' 26.6 advances (§8.3, already certified by `text/ttf-advance` and `ttf-width`), with expected label
  pixels from the certified text path as a lower layer (plan §2.5 rule 1); `text-entry/scroll-to-cursor`
  takes its widths from the TrueType metrics; `panel/counts`, `fail-rows` and `layout` go, because §23.1's 5×7
  diagnostics panel is replaced by the stage.
- R3 and R4 together: `panel` (7) `stage-layout`, `tile-stamps`, `tile-thumbnail`, `spotlight-picture`,
  `spotlight-timing`, `milestone-ribbon` and `finale` keep §23's rules at the display scale with DejaVu Sans
  text; the thumbnail and spotlight formulas are unchanged and run as `box_filter` and `nearest_scaled`.

**New, forced by GPU-only** (6): `raster/cpu-pixels-zero` (section 2.4's runtime accounting across the
category); `raster/cpu-render-planted` (section 2.4's planted violations are each refused);
`windows/commit-waits-render` (a commit whose render fence is still pending is not composed until it
completes, and one that completes with an error is refused); `composition/gpu-composite` (client targets
composed with no CPU pixel copy, by counter); `composition/target-lifetime` (a target held by the compositor,
the scanout, a capture or a replay copy survives `RESOURCE_DESTROY` until its last holder releases it);
`scanout/present-flip` (the record names the flipped target).

**New with R4** (5): `text/hidpi-sizes` (scale-2 text is rasterized at twice the size, equals its reference
and differs from 2× replication), `widgets/scale-2`, `composition/scale-2`, `scanout/hidpi-mode` (full
2560×1600 readback), `panel/stage-hidpi`.

The GPU-only rule alone gives 393 − 2 + 6 = 397 cases; with R3 and R4 the rebuilt suite has
397 − 3 + 5 = **399** cases. Its 16 categories and 14 milestones are unchanged.

### 2.3 Contract sections

| Section | Becomes |
|---|---|
| §1, §6, §9–§13, §15, §16, §19, §22, §25 | unchanged |
| §2 Pixels and alpha | formulas unchanged; formats become target formats; 3-byte only as upload and readback formats |
| §3 Surfaces | rewritten: GPU targets, their limits and errors |
| §4 Clipping, §5 Primitives | pixel sets unchanged; each names its pipeline; §5.5 gains the closed form and its proof; §5.9's `copy_row_from` removed |
| §7 Work counters | `libgfx::work` becomes geometry-setup steps plus emitted fragment area and work estimate; the glyph rasterization bound split between CPU crossing steps and GPU fragment-steps; libfont's other bounds and libimg's unchanged |
| §8.1, §8.2 bitmap fonts | unchanged, drawn by `bitmap_text` |
| §8.3 TrueType text | layout and coverage definition unchanged; a normative binary32 operation order added; computed on the GPU |
| §8.4 Glyph cache | a GPU atlas, same capacity rules |
| §8.6 Reviewed references | unchanged |
| §14 Widgets | text in DejaVu Sans at the theme's size (R3); geometry in logical units × scale (R4) |
| §17 Window interface | buffers are GPU targets; ops 14 and 21 retired; immutability by refused submission plus the render-completion handoff; accounting split between `RLIMIT_AS` and the GPU quota |
| §18 Compositor and display | GPU composition and hold sets; modes 1280×800 and 2560×1600; `fb_info`, `fb_mmap`, `FLUSH` and the CPU display copy gone; the IRQ bound kept over the GPU path |
| §20 Scanout observation | record version 2 (section 1.8); boundary code 4 |
| §21 Test controls | points 19–31 added (the GPU contract's); point 2 `irq-hold` kept |
| §23 Panel and stage | §23.1 removed and DejaVu Sans text (R3); §23.2–§23.8 at display scale (R4) |
| §24 Helpers | unchanged; `kiosk_test` composes on the GPU |

### 2.4 Does any CPU raster remain, and how is that enforced?

Only as test oracle, and only where an independent expectation has to be computed:

1. **The suites' formula expectations**, as today: the suite crates compute expected pixels from the
   contract's formulas and literal tables on the CPU, the oracle plan §2.5 requires. It shares no code with
   BGL, the kernel's programs or BreenGL.
2. **`libs/libgfx-ref`**: libgfx's CPU primitives, blits and alpha, and libfont's coverage rasterizer, moved
   without change. The move is checked as a pure rename (no content change in the diff), so the evidence of
   393/393 on ARM64 QEMU, Parallels and VMware at #1655's merge (`4e855bb6`) applies to exactly this code. It
   also gains host tests against the contract's literal tables (§5's lines and circles, §8.3's square). Its
   consumers are `tools/gfx-refgen` (authoring references on the host, which plan §2.3 allows), host tests,
   and the suites' expectation code (`coverage-spans-winding`, `coverage-spans-corpus`). The reviewed text
   references, not `libgfx-ref`, remain the oracle for transformed and curved glyphs, as they are today.

The pre-driver boot console and panic screen are CPU-written on the firmware framebuffer before any GPU
driver runs, and nowhere else (ruling R7); after the driver starts, the console is GPU-drawn (section 1.7).

**Enforcement, in three layers, each with a planted violation it must catch:**

- **Static.** A structure test in `tests/` refuses any dependency on `libgfx-ref` from the kernel or from
  `libs/breengel`, `libbui`, `libicon`, `libfont`'s non-test code, `bgl` or any installed program; suite
  binaries may depend on it only from their expectation modules. It refuses direct use of syscall 416 outside
  `bgl`, and any call of BGL's `upload` outside its two sanctioned sites: `Texture::from_decoded`, which takes
  a `libimg::Decoded` that only libimg constructs, and the fixture path, which takes only the built-in font
  tables or a suite's declared fixtures.
- **Runtime.** `COUNTERS` reports upload bytes by purpose and instances per draw. In every drawing case, upload
  bytes must equal the bytes of the decoded assets and fixtures the case declares (zero for shape, alpha and
  TrueType text cases), and each primitive may emit at most a constant number of instances set by its pipeline
  (so a CPU loop drawing one-pixel rectangles is caught). Suite binaries are held to the same counters: in a
  passing run they upload no expected image; on a failure they may upload expected and diff images, tagged
  `Fixture` and reported as failure artifacts.
- **Planted.** A fixture crate and a test-control flag supply four violations, each of which the checks must
  refuse with its reason: a renamed CPU fill loop whose result is uploaded as `Image`; a CPU loop emitting a
  `solid` rectangle per pixel; a library crate that takes a raw `&mut [u8]` pixel slice and uploads it; and a
  suite binary presenting a CPU-rendered expected image in a passing case. `raster/cpu-render-planted` runs the
  runtime ones and the structure test's own fixtures cover the static one.

### 2.5 What of `breengel/suite` is kept

| Kept as is | Kept, re-based | Removed |
|---|---|---|
| `libimg` (inflate, deflate, CRC-32, Adler-32, PNG decode and encode, BMP) and its fixtures | `libfont` (parsing, hardening, flattening and crossings kept; span accumulation to the GPU) | the software compositor in `kernel/src/graphics/display.rs` and its backends |
| `libfont` parsing, the 64-file corpus, work and allocation bounds | `libgfx` drawing → BGL pipelines; its CPU code → `libs/libgfx-ref` | `libgfx::diagnostics` (R3) |
| `libs/breengel-abi`, `kernel/src/window/` registry, commit protocol, damage log, events, authority, claims, remnants | `kernel/src/window/mem.rs` buffer storage → GPU targets; `present.rs` → GPU composition | `fb_info`, `fb_mmap`, `fb_draw` and the version 1 window ops |
| syscall 415 test controls, the PID-1 latch, kernel output accounting | `svga3.rs` and `gpu_pci.rs`/`virgl.rs`: their transport, fencing and readback code becomes the backends' base, after the audit | the DISPI 1280×800 mode set and GOP compositor work in #1654 |
| `libbui` widget logic, `FocusChain`, the file picker, `libicon`, the UI clock, `FontWatcher` | widget and stage drawing → BreenGL on BGL; `libgfx::font` and `bitmap_font` → `bitmap_text` | |
| `libpixcheck`'s comparison core, `suite::spawn`, `kiosk_test`, `raw-window_test`, the suite harness and its split | `suite::judged(&FrameBuf)` → `judged_readback`; the run stage (`libgfx/stage.rs`, `suite_stage`) → BreenGL at display scale | |
| the showcase contract (§23.2–§23.8), Vigil's showcase pacing and evidence (#1658) | | |
| general kernel fixes found on the branch (for example `a9fd021d`, fw_cfg read serialization) | | |

### 2.6 The graphical programs

Fourteen installed programs use `FrameBuf`, `fb_mmap` or `libgfx` today, on main and on `breengel/suite`
(found by search on 2026-10-16). Between `gpu`'s landing and BreenGL's they keep running on the existing
display path, which G1 keeps working on QEMU's PCI device. In the BreenGL rebuild each is either ported to
BreenGL on BGL, in a new `breengel/gpu-apps` lane, or retired from the tree and the image:

| Program | What it is | Disposition |
|---|---|---|
| `bwm` | the window manager | ported (decorations, frames and its own drawing through BreenGL; its composition model is the `window-manager` effort's) |
| `bterm` | the terminal | ported |
| `blauncher` | the quick launcher (Super+Super) | ported |
| `bfontpicker` | the font picker | ported |
| `bcheck` | the windowed post-boot self-check, spawned at session start | ported |
| `blog` | the graphical log viewer | ported |
| `probe` | the production boot probe | ported (its graphical checks become GPU checks) |
| `bounce` | the bouncing-spheres demo, spawned at session start | ported |
| `particles`, `confetti`, `rectangles`, `demo`, `biconkit`, `guskit` | demos and showcase apps | retired, with an issue each to rebuild in `gui-apps` on BreenGL (ruling R8) |

The PR that removes `fb_info`, `fb_mmap`, `fb_draw`, ops 14 and 21 and the software compositor is the
`gpu-apps` lane's last: it lands the last port and the retirements in the same change, and the image build
and a boot smoke that starts every installed graphical program are its gate. No program ever builds against
a removed API or runs into ENOSYS. The session scripts in `scripts/create_ext2_disk.sh` that spawn `bounce`
and `bcheck` and bind Super+Super to `blauncher` keep working throughout.

### 2.7 Carrying the branch forward

1. **First:** freeze `breengel/suite` at `0585917d` and tag it `breengel-cpu-393`, recording its newest
   evidence with the tag (393/393 on ARM64 QEMU, Parallels and VMware, at #1655's merge, `4e855bb6`), so the
   CPU-raster result stays reproducible. Stop BreenGL lane work.
2. **Harvest to main** (section 3.3), each PR after its files' provenance audit. The harvest PRs re-apply
   files on branches cut from main, so they carry no effort-lane commit and pass the target guard.
3. **The `gpu` effort** runs and lands on main.
4. **Merge main into `breengel/suite`** with a merge commit (force pushes are not used; merge commits are the
   plan of record). Harvested files arrive identical, so those conflicts resolve to either side.
5. **BreenGL GPU lanes** cut from `breengel/suite` (`breengel/gpu-raster`, `gpu-text`, `gpu-window`,
   `gpu-display`, `gpu-apps`; IMAGES and WIDGETS logic need no lane), each writing its changed checks first,
   in their own commits, failing for a specific reason, before the implementation (BreenGL's existing lane
   rules stand). The contract and manifest change in one DISPLAY-owned PR, carrying the GPU-only changes and
   R3's and R4's.
6. **ARM64 QEMU green (399), then the catch-up** on Parallels and VMware.
7. **#1625 lands** as the GPU-based BreenGL, retitled, with the three-target evidence and attestations.

## 3. The path, the entries and the harvest

### 3.1 New order

`kernel-bring-up`, `files-io`, `directories`, `processes`, `signals`, `time`, `memory`, `threads`, `ipc`,
`terminals`, `networking`, `libc`, `rust-std`, `utilities`, `sdk`, **`gpu`**, `breengel`, `window-manager`,
`ui-testing`, `gui-apps`.

### 3.2 Entries in `docs/efforts/path.json`

Inserted after `sdk`:

```json
{
  "id": "gpu",
  "title": "GPU rendering",
  "outcome": "Breenix renders on the GPU on every target: a clean-room, GL-style Breenix API whose command streams the kernel validates and bounds per process and translates for VirGL (ARM64 QEMU and Parallels) or SVGA3D DX (VMware), executed by a hardware GPU on each host, presented at 1280×800 and at 2560×1600 with HiDPI scaling, and read back from the device pixel-exact under a declared deterministic rendering profile.",
  "targets": ["arm64-qemu", "parallels", "vmware"],
  "targetsNote": "x86-64 joins when the dedicated Breenix hardware box replaces the current x86 gate host.",
  "suite": "gpu",
  "subsystems": [17, 22, 25],
  "milestones": [
    { "id": "device", "title": "3D device & capability profile", "measure": { "suite": "gpu", "categories": ["device"] } },
    { "id": "submit", "title": "command submission & fences", "measure": { "suite": "gpu", "categories": ["submit"] } },
    { "id": "readback", "title": "device readback", "measure": { "suite": "gpu", "categories": ["readback"] } },
    { "id": "textures", "title": "textures & uploads", "measure": { "suite": "gpu", "categories": ["textures"] } },
    { "id": "profile", "title": "deterministic rendering profile", "measure": { "suite": "gpu", "categories": ["profile"] } },
    { "id": "pipelines", "title": "drawing pipelines", "measure": { "suite": "gpu", "categories": ["pipelines"] } },
    { "id": "isolation", "title": "per-process GPU isolation", "measure": { "suite": "gpu", "categories": ["isolation"] } },
    { "id": "present", "title": "display modes, HiDPI & present", "measure": { "suite": "gpu", "categories": ["present"] } },
    { "id": "performance", "title": "frame rate & latency", "measure": { "suite": "gpu", "categories": ["performance"] } }
  ]
}
```

The milestones use the ordinary suite measure. Until `docs/suites/gpu.json` has those categories, Vigil shows
each milestone as not measured yet, as it does for `breengel`, `window-manager`, `ui-testing` and `gui-apps`,
whose suites are not on main either (`docs/efforts/README.md`).

`breengel`'s outcome is replaced (its milestones are unchanged):

> "Programs draw with BreenGL entirely on the GPU (clipped and alpha-blended shapes and blits, bitmap and
> TrueType text, PNG and BMP images, widgets and animated icons) exactly to its written contract, every
> installed graphical program draws through it, and the windows they commit through an authority-checked
> kernel interface are composed by the GPU and observed by device readback, pixel-exact on every target."

The path's top-level outcome ends "…an SDK for building our own applications, and a GPU-rendered graphical
desktop (GPU rendering, BreenGL, the window manager and its applications) tested to the same bar."

`breengel`, `window-manager`, `ui-testing` and `gui-apps` carry the same `targets` and `targetsNote`.
`window-manager`, `ui-testing` and `gui-apps` keep their entries otherwise. Their plans gain one rule when each
is authored: bwm composes through BGL, and every app draws through BreenGL on the GPU.

`docs/gui/plan.md` points here and states the GPU-only direction in its decisions; its CPU-rendering sections
(§1 item 3, §4, §14, §20.1) and `docs/gui/summary.md` are rewritten with BreenGL's contract change (section
2.7, step 5). `docs/gui/provenance/` and `docs/suites/gpu.json` arrive in G0.

### 3.3 The harvest to main (small PRs, each after its files' audit and with its evidence)

1. Test controls: syscall 415, the PID-1 latch, `init`'s absence check, on ARM64 and x86-64, both dispatched
   through `dispatch_numbered` (no Tier-1 file changes).
2. Kernel output accounting (§22) and its structure test.
3. `libs/libpixcheck`'s comparison core over byte slices, and `libbreenix::suite::judged_readback`. Not
   `suite::judged(&FrameBuf)`, `Suite::gui()` or `suite_stage`, which stay on `breengel/suite` until BreenGL
   converts them.
4. The VMware driver corrections (`kernel_va_to_ipa` addresses for command buffers and DMA pages, SVGA3D
   commands on context 0 after `START_STOP_CONTEXT`, `SET_OTABLE_BASE64`, the version 1
   `DEFINE_GB_SURFACE`, restart after `COMMAND_ERROR`) and the host-copy readback sequence.
5. The Parallels driver's fenced `TRANSFER_FROM_HOST_3D` readback and texture-slot allocation.
6. The display ownership stack (§18: `GIVE_DISPLAY` semantics, exact restore, the four-owner bound).
7. General kernel fixes found on the branch that are not BreenGL-specific, `a9fd021d` first.

### 3.4 #1625 and #1654 meanwhile

- **#1625** becomes a draft, with a comment giving the ruling, the freeze tag and a pointer to this document,
  and stays the BreenGL integration branch. It does not merge until the GPU-based BreenGL passes its 399 cases
  on its three targets. Its title changes then.
- **#1654** stops and stays a draft with a comment. Its GOP compositor and DISPI mode set are obsolete. Its
  arch-neutral window-interface work for x86-64 (the 414/415 dispatch, the x86 window ops) is taken when
  x86-64 joins BreenGL (section 1.6.4), by merging its branch into `breengel/suite` and deleting the GOP and
  DISPI display code in the same PR; #1654 then closes as merged.

## 4. Risks

| Risk | Why it is real | Mitigation, and what to check first |
|---|---|---|
| **The macOS host stack** | UTM builds QEMU as a library for its own app; turning it into a CLI binary with a headless gate mode is unproven; rendering with no SPICE client attached is unverified; it moves every ARM64 gate from QEMU 11.0.3 to 10.0.12-utm. | G0 first, pinned and scripted, with both frontends in its acceptance. The null-client fallback for headless; the sized SDL fallback. Every ARM64 suite must match main on the new binary before the gate switch (R5). |
| **x86-64 has no GPU path yet** | The current x86 gate host has no rendering GPU, the gate's virtual GPU negotiates no VirGL, and its QEMU's GL display module failed to load. | x86-64 is out of the GPU-era efforts' targets until the dedicated Breenix hardware box replaces that host (R1); its spike's acceptance mirrors G0's then. |
| **The hardware claim** | No protocol lets the guest tell a GPU from software. | Host attestation from the executing context on QEMU targets, refused when software or missing, with a planted software run; configuration plus host GPU time on Parallels and VMware, with a negative control, and the configuration check as the labelled basis where that cannot separate them (R6). |
| **VMware DX complexity** | No DX draw has run; the shader token programs and signatures are hand-written; errors arrive only as command-buffer status; readback of a rendered target is unproven. | The G0 spike covers the whole profile before the contract freezes; one program at a time, each with a readback case; a Linux VM in Fusion as a black-box behaviour probe. |
| **Exactness of GPU output** | Drivers may round UNORM conversion, blend or interpolate differently; ANGLE translates to Metal; native float precision varies. | The profile removes every place a driver chooses: integer predicates with multi-word arithmetic, integer fetches, shader blending with the hazard rule, integer-cornered rectangles, binary32 emulated in integers. `profile/*` checks each property per target. A target that cannot meet the profile fails; there is no tolerance. |
| **Shader cost of exactness** | Multi-word arithmetic, binary32 emulation and destination copies cost device time, at four times the pixels. | The work estimate makes the cost visible per draw; glyphs are rasterized once per size into an atlas; the performance bounds are set now and a miss is a slow path to fix. If emulated coverage cannot meet them after optimization, a change to §8.3's definition goes to the operator with measurements, as its own ruling. |
| **Kernel complexity and isolation** | Validating and translating command streams in the kernel is new attack surface; the existing encoder stops silently at capacity. | Whole-stream validation, the work estimate and caps, the watchdog and reset, per-process contexts with only owned resources attached, hold sets, the fuzz case, no device addresses exposed. |
| **Program migration** | Fourteen programs use the CPU APIs; retiring them early breaks the image. | Port or retire in the `gpu-apps` lane; the API removal lands with the last port, gated by the image build and a boot smoke of every graphical program. |
| **Clean-room drift** | The existing VirGL code has a history that includes a byte-capture plan. | The audit is first and gates every harvest and reuse; sources listed per PR; host-stack lanes kept apart from guest GPU code. |
| **Time** | BreenGL's visible result waits for a whole new effort plus a rebuild: roughly 3.5 to 7 months by section 1.11's guesses. | The `gpu` suite is watchable from G2; the BreenGL rebuild keeps 348 cases' meaning and every codec, parser and window-protocol piece. The guesses are revised after G0. |
| **Readback cost at 2560×1600** | Each full readback moves 16 MB; slow uncached copies were seen on VMware before. | Readback MOBs and backings are cached guest RAM; `performance/readback-time-2560` holds it; cases read back the rectangle they judge when the contract allows, and the full destination where the contract requires it. |

## 5. First steps and rulings

1. Convert #1625 to a draft, tag `breengel-cpu-393`, stop #1654 (section 3.4).
2. Write `gpu` into `docs/efforts/path.json` with the `breengel` outcome change (section 3.2): done with this
   document.
3. G0 in section 1.11's order: the provenance audit, then the macOS host stack and the harvest PRs, the
   Parallels and VMware spikes, the contract and the `gpu` suite with 168 failing cases. **The first lane is
   the provenance audit of `virgl.rs`, `gpu_pci.rs` and `drivers/vmware/`,** because every harvest and every
   reuse waits on it; the macOS host stack starts beside it, since it reuses no guest code.
4. G1 onward on ARM64 QEMU.

**Rulings, 2026-10-16, made under the operator's GPU-only ruling.** R1 to R8 are decided; R1 is the
operator's own.

- **R1, x86-64's GPU: decided (operator).** The current x86 gate host stays VGA only and x86-64 is measured
  up to the shell. x86-64 leaves the GPU-era efforts (`gpu`, `breengel`, `window-manager`, `ui-testing`,
  `gui-apps`) until a dedicated Breenix hardware box replaces that host, and joins them then (section 1.6.4).
  There is no software-renderer option.
- **R2, early spikes: decided, yes.** The Parallels and VMware spikes run in G0, before the contract freezes,
  as scratch boots on `scratch/*` branches cut from main. The x86-64 spike follows when the dedicated
  Breenix hardware box is in place.
- **R3, UI text: decided, yes.** UI and stage text moves to DejaVu Sans (15 changed cases with R4, 3 removed;
  section 2.2). The bitmap fonts stay in the API.
- **R4, the GUI mode: decided, yes.** Every GUI suite boot runs at 2560×1600 at scale 2, and 1280×800 stays a
  tested mode.
- **R5, the ARM64 gate binary: decided, yes.** The ARM64 gates move to the matched UTM-stack QEMU
  (10.0.12-utm) at `gpu`'s landing, only after every ARM64 suite matches main on it.
- **R6, Parallels' and VMware's hardware claim: decided, yes.** Where no host measurement separates GPU from
  software rendering on Parallels or VMware, the configuration check is the claim's basis, labelled so in the
  record.
- **R7, the pre-driver console: decided, yes.** The boot console and panic screen stay CPU-written on the
  firmware framebuffer before the GPU driver starts, and nowhere else.
- **R8, the six demos: decided, yes.** `particles`, `confetti`, `rectangles`, `demo`, `biconkit` and `guskit`
  are retired until `gui-apps`.
