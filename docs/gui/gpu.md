# GPU rendering: the `gpu` effort's design and contract

Written 2026-10-16 against `origin/main` `45ff7aa6`; the contract was frozen the same day, after the G0
spikes reported (section 1.12). This document is the design and contract of the `gpu` effort ("GPU
rendering"), which comes after `sdk` and before `breengel` in `docs/efforts/path.json`, and the plan for
rebuilding BreenGL on it. Section 5 records the rulings it needed; R1 to R8 are decided.

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

**Status.** A plan and a frozen contract. G0's host stack, provenance audit and Parallels and VMware spikes
have run; the effort's lanes land into `gpu/integration` (section 1.11). Sections 1.3 to 1.10 are the
contract, frozen after the spikes reported. Section 1.12 records what the spikes changed and why, section
1.13 the amendment G1's review required, section 1.14 the amendment G2's review required, and section 1.15
the interface G3 needs, and section 1.16 the interfaces G6 needs. A later change to a frozen section is a contract change: it is made
in its own PR, with its reason, before the lane that needs it, separately from the code,
checks or thresholds that conform to it. The suite,
`docs/suites/gpu.json` and `/sbin/suite-gpu`, measures this contract; its 170 cases fail, each naming the
layer it needs, until that layer exists.

**References.** `§N` is a section of BreenGL's contract, `docs/gui/breengel.md`, whose suite is
`docs/suites/breengel.json`; both are on the `breengel/suite` branch at `0585917d` (the showcase merge,
#1658). `plan §N` is a section of `docs/gui/plan.md`. "Section N" is a section of this document. #1625
(`breengel/suite` into main) and #1654 (`breengel/catchup-x86` into `breengel/suite`) are the BreenGL PRs open
when this was written.

## 0. Decisions at a glance

1. **A new effort, `gpu` ("GPU rendering"), goes into the path after `sdk` and before `breengel`.** It
   delivers the kernel GPU core, two independently written device backends (VirGL and SVGA3D DX), a
   Breenix-authored GL-style API (`libs/bgl`, "BGL"), display modes up to 2560×1600 with a scale factor,
   present, and exact device readback on ARM64 QEMU, Parallels and VMware. Its suite is `gpu` (170 cases,
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
    feasibility spikes on `scratch/*` branches cut from main in G0, before the contract froze (ruling R2).
    The spikes exercised a subset of the profile (section 1.12). The catch-up then goes Parallels, then VMware.

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
                      ├─ kernel/src/gpu/virgl/: VirGL + TGSI encoder, virtio-gpu commands and queue → the kernel's virtio PCI transport
                      │      ARM64 QEMU (virtio-gpu-gl-pci), Parallels (its virtio-gpu PCI)
                      └─ kernel/src/gpu/svga/: SVGA3D DX + shader-token encoder and SVGA3 command path (VMware, catch-up)
```

The effort renders full-screen under one display owner (the suite runner, or a case it gives the display
to). Windows, composition of several clients and the window interface stay in `breengel`; nothing here
waits on bwm.

**The core is clean-room only.** Everything under `kernel/src/gpu/` is written from section 1.9's allowed
sources. It never calls, wraps or copies `drivers/virtio/gpu_pci.rs`, `drivers/virtio/virgl.rs` or
`drivers/vmware/svga3.rs`; those drivers serve the existing display path until BreenGL deletes the software
compositor (section 2.6). The in-place rewrite of those drivers (#1673) is closed. On ARM64 QEMU the existing
display (the boot console and main's programs through `fb_mmap` and `fb_draw`) stays on the MMIO
`virtio-gpu-device` until G7; the core drives `virtio-gpu-gl-pci` beside it.

### 1.3 BGL, the GL-style API

BGL borrows OpenGL's concepts and names none of its entry points; it claims no OpenGL profile or
conformance. It is a userspace crate (`no_std` + `alloc`, integer-only, so it needs no floating point in any
userspace) over syscall 416.

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

**Backing and row pitch.** Every guest backing the kernel owns for a GPU resource (targets, textures, buffers
and readback bounce buffers) is whole frames from the frame allocator, attached to the device as a page
list, never kernel heap. VirGL transfers use `align_up(width × bytes per pixel, 256)` row pitch as a
conservative policy following the Parallels round-trip probes (section 1.12); those probes did not isolate
which transfer direction failed. VMware's version 1 `DEFINE_GB_SURFACE` (1097) has no pitch field: its
single-mip, single-sample, non-array surfaces use tightly packed rows of `width × bytes per pixel`, with
only the MOB allocation rounded to whole pages. In particular BGRA width 48 uses 192-byte rows and R8
width 64 uses 64-byte rows on VMware. The kernel copies rows between the backend's layout and the caller's
`stride`, which the device never sees. The validator refuses nothing on this account; the translator
supplies the backend's layout.

**Copies.** A `copy` between two rectangles of one resource that overlap gives the result of copying the
source as it was before the copy; the kernel stages it through a scratch texture. A region copy moves bytes
unchanged, so its source and destination have the same format; converting between formats is the `copy`
pipeline's work.

**Sampling.** A pipeline that samples a texture reads each texel by integer fetch at the coordinates its
rule gives and converts it channel by channel into the target's format: B8G8R8A8 and R8G8B8A8 texels keep
their four channels, and an R8 texel `r` reads as R `r`, G 0, B 0 and A 255, the value OpenGL ES 3.0 and
Direct3D 10 both give a one-channel texture's missing channels. A rectangle drawn from a texture is clipped
to the texels the texture has, so no pixel reads outside it.

### 1.4 Syscall 416

`gpu(op: u32, arg: *mut u8, len: usize) -> isize`, numbered the same on both architectures, with the window
interface's conventions (§17.1): `len` must equal the argument size, reserved fields zero, negative errno
returns, `copy_from_user`/`copy_to_user` everywhere, and no kernel output on any path (§22). EFAULT is atomic
only as far as the kernel can guarantee it: a fault reading the argument returns EFAULT before the op does
anything, but results are written back after the op acts, and the kernel cannot hold the caller's pages
against the caller's own threads, so what an EFAULT while writing them leaves behind is stated per op in
`docs/gui/gpu-abi.md`. An op number the table does not list returns EINVAL. The GPU-era targets dispatch it on
ARM64; x86-64's dispatch arrives when x86-64 joins (section 1.6.4).

| Op | Name | Notes |
|---|---|---|
| 0 | `VERSION` | returns 1 |
| 1 | `CAPS` | the profile record: backend, feature bits, formats, maximum sizes, catalogue version, the device's host renderer string where its capability set carries one |
| 2, 3 | `CONTEXT_CREATE`, `CONTEXT_DESTROY` | per-process; EAGAIN past the quota; creation queues asynchronous warm-up of the fixed rendering catalogue on the GPU worker and returns; `probe_int` routines warm on their first PROBE (section 1.4.1) |
| 4, 5 | `RESOURCE_CREATE`, `RESOURCE_DESTROY` | targets, textures, buffers; a scanout flag declares scanout targets at creation (section 1.7); ENOMEM against the GPU memory quota and `RLIMIT_AS` for guest backing; destroy drops the handle, and storage is freed when the resource's hold set is empty (section 1.4.1) |
| 6 | `UPLOAD` | user bytes to a texture or buffer rectangle, clipped, with its `purpose`, sent in a named context; visible to later submissions only; EINTR on an interrupted wait, ENOMEM if staging cannot be allocated or charged to the process's guest-backing quota or `RLIMIT_AS` |
| 7 | `SUBMIT` | a BGL command stream, validated whole before anything is sent (EINVAL with the failing command's index), with its work estimate checked (EDQUOT past the cap); returns a fence |
| 8 | `WAIT` | fence wait with timeout; ETIMEDOUT, EINTR, EIO for an errored or reset fence; killable within 250 ms |
| 9 | `READBACK` | a target or texture rectangle into user memory, with a 1,024-byte `ObservationRecord` version 2; ENOMEM if staging cannot be charged to the process's guest-backing quota or `RLIMIT_AS` (section 1.8) |
| 10, 11 | `MODES`, `SET_MODE` | display owner only for `SET_MODE` |
| 12 | `PRESENT` | display owner only; EINVAL for a never-written target or one created without the scanout flag; returns a fence of the context it names, never a raw device fence number (section 1.7) |
| 13 | `COUNTERS` | per-context submissions, draws, instances, fragment area, work estimate, uploads by purpose, readbacks, catalogue warm-up state |

**Dispatch without a Tier-1 edit.** 416 is not added to `SyscallNumber`; it is dispatched in
`dispatch_numbered` (`kernel/src/syscall/mod.rs`, about line 889), which both architectures' entry paths
already fall through to for unlisted numbers (`kernel/src/syscall/handler.rs` line 563 on x86-64,
`kernel/src/arch_impl/aarch64/syscall_entry.rs` line 187 on ARM64). No Tier-1 file changes in this effort.
When x86-64 joins, its dispatch of 415 and 416 takes the same route; the earlier `handler.rs` change for 414
and 415 (`652a4494`) was reverted (`0f53872e`) and is not reapplied.

#### 1.4.1 Submission, completion and lifetime

- **Validation atomicity.** A stream that fails validation sends nothing to the device. This is a property of
  validation only; it says nothing about execution.
- **Foreign handles are the validator's to refuse.** A stream naming a resource or context the process does
  not own is refused with EINVAL before anything is sent. The device gives no such error: a VirGL command on
  a resource not attached to the submitting context is answered `OK_NODATA` and does nothing (section 1.12),
  and disables its context as the next item states. No case expects a device error code for a foreign handle.
- **Two commands answer OK and silently disable their context.** These are device facts, observed on ARM64
  QEMU's VirGL device (`docs/gui/provenance/gpu-core.md`, experiments B1 to B4), not rules Breenix chose:
  - a `SUBMIT_3D` naming a resource not attached to the submitting context;
  - a transfer whose box extends past the resource (observed with a `TRANSFER_FROM_HOST_3D` box one pixel
    past the resource's width).

  Each is answered `OK_NODATA`, and from then on every command in that device context, transfers included,
  is answered `OK_NODATA` and does nothing. The device reports no error, so no fence status,
  `COUNTERS` state or quarantine can show it; only an observation can. A readback in such a context leaves the
  target's poisoned backing unwritten (`READBACK` reason 1, section 1.8). The kernel therefore never sends
  either command: the validator refuses foreign handles, every resource is attached to every context of its
  process, and every transfer box the kernel builds (`READBACK`'s rectangle, its bands, test-control point
  21's transfer, `UPLOAD` from G3) is checked against the resource it names before it is sent.
<!-- claim-lint:ok: Normative asynchronous warm-up requirements from coordinator ruling 33 (#1725), not measured results. -->
- **Program warm-up (ruling 33, #1725).** `CONTEXT_CREATE` queues the fixed rendering catalogue's
  compilation and warm-up on the GPU worker and returns without waiting for it. The worker draws each
  rendering program once into a 1×1 scratch target and waits for its fence. A draw whose pipeline is
  still warming waits only for that pipeline, never for the remaining catalogue; a ready pipeline can
  draw while other pipelines warm. `WAIT` and fence ordering and completion retain their existing rules.
  The context reports progress through `COUNTERS`: state 3 is live with catalogue warm-up in progress;
  state 0 is live with catalogue warm-up complete. Quarantined and reset states keep their meanings
  (`docs/gui/gpu-abi.md`). Performance bounds, including `performance/first-draw-latency`, measure draws
  issued after that context reports warm-up complete, excluding application startup and warm-up.
  The profile-only `probe_int` measuring programs are excluded from catalogue warm-up; each compiles and
  warms on its routine's first PROBE in that context. That first-use cost is excluded from performance bounds;
  later PROBEs use the warmed program for that routine. `probe_int` is a family with one
  program per routine (ruling 48), compiled on that routine's first use. Each compilation remains
  subject to the unchanged 2 s watchdog on every target.
  `submit/context-create` retains the suite runner's existing 10 s case deadline. Except for the
  timed-window start of `textures/upload-visibility` (ruling 52, below), case deadlines and work-estimate
  weights do not change, including for cases waiting for warm-up completion.
  Each warm-up fence is subject to the same 2 s watchdog as any submission; compile time is not exempt
  from hang recovery. A kill during creation or a wait for a warming pipeline must end the caller within
  250 ms, without waiting for remaining programs to compile (`isolation/killable-wait`); the first PROBE
  has the same watchdog and kill bounds. Queued work retains its resources until completion or reset.
  The spike measured compilation of only its own programs, not the full catalogue (section 1.12).
- **Execution and publication.** Each target a submission writes is *pending* until the submission's fence
  completes. Completed OK, the target's generation advances and its contents are published. Completed with a
  device error, or lost to a watchdog reset, each target the submission wrote is marked *invalid*: readback
  returns EIO naming the submission, sampling it in a later submission is refused, and it becomes valid again
  only when a later submission that writes it whole (a full clear, a full copy or a full upload) completes
  OK. Drawing into an invalid target is allowed: the validator refuses only a command that samples one. A
  draw that does not write the whole target leaves it invalid. A partial result of a failed submission is
  invalid. An `UPLOAD` the device answers with an error, or that the watchdog or a reset
  ends, leaves its texture invalid the same way, until an upload of the whole texture completes OK.
  An interrupted upload wait returns EINTR; if its request has been sent, it leaves its texture invalid
  with the same recovery rule, even if the device later completes that upload OK. Interruption before
  sending leaves the resource unchanged. Temporary upload staging is charged like `READBACK` staging
  (section 1.8), including `RLIMIT_AS` and `COUNTERS.resource_bytes`; inability to allocate or charge it
  returns ENOMEM before device work is sent or the resource is changed.
- **Uploads are ordered, not blocking.** An `UPLOAD` is sent behind everything its context sent before it, and
  the device executes one context's commands in the order they were sent (which
  `textures/immutable-while-pinned` checks), so a submission of that context sent before the upload samples
  the texture as it was before, and the submissions sent after the upload returns see it. It waits for a
  readback or upload of the same resource in progress and for submissions of the process's other contexts that
  name the resource, whose order against it the device does not state.
- **The work estimate.** For each draw the validator computes fragments × per-fragment steps, where the
  per-fragment steps are a constant of the pipeline, except `coverage_spans`, whose steps for a row are the
  number of crossings on that row's 8 sub-scanlines (read from the buffer's per-sub-scanline offsets in
  `O(rows × 8)`). Limits per submission: 65,536 draws, 1,048,576 instances, 64 MiB of buffers, 2³¹ estimated
  fragment-steps; per context, 4 submissions in flight. Every shader loop has a trip count fixed by the
  pipeline or clamped by a validated parameter, so the estimate bounds the device work.
- **The watchdog.** A submission whose fence has not completed 2 s after it was sent is treated as hung: its
  context is quarantined (its pending fences complete with EIO `device-hung`, its targets become invalid,
  further submissions to it are refused) and the kernel destroys the device context. If the device's queue
  makes no progress for another 2 s after that quarantine, the kernel resets the device (virtio-gpu reset and
  reinitialization; SVGA3's restart in the VMware backend), every context gets EIO `device-reset`, and the
  display owner and compositor recreate their resources; the console is redrawn. virtio-gpu defines no
  per-context cancellation, so destroying a context is not assumed to stop host work. Completing a fence
  with EIO reports failure to callers; it does not prove that device access has ended. DMA backing remains
  held until actual device completion or a successfully confirmed reset proves that the device can no
  longer access it. A reset request or an unsuccessful reset supplies no such proof; if reset fails, the
  backing stays held, even after callers have received EIO. The watchdog examines the outstanding
  submissions at least once every 250 ms.
- **Device errors quarantine one context; resets are for the device.** A submission the device answers with
  an error completes its fence with EIO `device-error`, and its targets become invalid. Two consecutive
  submissions of one context completing with device errors (no OK completion between them, in fence order)
  quarantine that context and no other: further submissions to it return EIO, its submissions already sent
  complete with the device's own answers, and the kernel then destroys its device context. Other contexts,
  of the same process or another, stay live. The device is reset only for a device fault or the watchdog's
  no-progress condition above, never because one process's submissions fail. A device fault is a failure of
  the device rather than of one context's stream: on virtio-gpu, the device setting `DEVICE_NEEDS_RESET`
  (VirtIO 1.3 section 2.1), a used-ring entry naming a descriptor the kernel did not post or a length past
  its buffer, or the failure of a command the kernel issues on its own behalf outside any process's context
  (a capability-set read, a display-information query). The VMware backend states SVGA3's device faults in
  the same terms when the catch-up writes it. Test-control point 27 follows the same rule (section 1.8).
<!-- claim-lint:ok: Normative watchdog and backing-lifetime requirements of section 1.4.1, extended to asynchronous warm-up by ruling 33 (#1725), not measured results. -->
- **Synchronous device work.** Every device wait the kernel makes on a caller's behalf, not only `WAIT`, is
  enrolled in the watchdog and ends within 250 ms of a kill: a draw waiting for its warming pipeline,
  `READBACK`'s transfers from G2, and every later one. Asynchronous catalogue warm-up on the GPU worker
  is also enrolled in the watchdog. Its DMA backing stays held until actual device
  completion or a successfully confirmed reset proves that device access has ended. Reporting EIO, a
  timeout or a kill to the caller does not release that backing; a failed reset leaves it held.
- **Buffers are snapshotted at submission.** `SUBMIT` copies each buffer range the stream names once, before
  validation; validation, the work estimate and translation read only that copy, so neither a later
  `UPLOAD` (from G3) nor a concurrent write can change what was validated. Charging that copy against a
  per-submission quota is G6 work (section 1.11).
- **Timing bounds.** Let W = 2,000 ms (the watchdog period above), K = 250 ms (the kill bound below, which is
  also the longest a blocked GPU path may take to observe any event that ends it: its fence completing, its
  timeout, a signal, a kill, its context's quarantine or a device reset) and R = 100 ms (the reset budget:
  from the reset decision to the device reinitialized, its control queue published again and every fence
  outstanding at the reset completed EIO `device-reset`). Intervals are measured on the suite's monotonic
  clock, from the start of the first call or action a row names to the return of the call that observes the
  result. In the two stalled-`SUBMIT` rows the upper bound starts at that `SUBMIT`'s return instead, because
  the submission is sent somewhere between the call's start and its return:

  | Case | Interval | Bound | Derivation |
  |---|---|---|---|
  | `submit/wait-timeout` | `WAIT` with timeout T on a fence that cannot complete in time, to its ETIMEDOUT | [T, T + K]; [50, 300] ms for T = 50 ms | never early; the expiry is observed within K |
  | `submit/wait-eintr` | arming a timer that signals the waiter after s, to `WAIT`'s EINTR | [s, s + K]; [30, 280] ms for s = 30 ms | no EINTR before the signal; the signal is observed within K |
  | `submit/watchdog-quarantine` | the stalled `SUBMIT`, to a `WAIT` blocked on its fence returning EIO `device-hung` | [W, W + 2K] = [2,000, 2,500] ms | hung at W after sending; the next watchdog scan within K; the waiter within K |
  | `submit/device-reset-recovers` | the stalled `SUBMIT`, to a `WAIT` blocked across the reset returning EIO `device-reset` | [2W, 2W + 3K + R] = [4,000, 4,850] ms | quarantine by W + K; no queue progress for W more, seen by a scan within K; reset in R; the waiter within K |

  The suite checks these bounds as written; changing one is a contract change. They bound recovery and
  wakeups and do not extend the 250 ms kill requirement. `submit/ring-backpressure` proves four submissions
  in flight by their fences, never by host GPU speed: with point 31's fence-depth mode (section 1.8) it
  finds all four fences incomplete (`WAIT` with timeout 0 returns ETIMEDOUT for each) before it submits the
  fifth, the fifth `SUBMIT` waits for room, and then all five complete OK in order.
- **Kills.** `WAIT` returns within 250 ms of a kill, but the killed process's queued GPU work is not cancelled:
  its contexts are quarantined, and its resources stay held until actual device completion or a
  successfully confirmed reset proves that device access has ended (`isolation/killed-work-drains`).
  Caller-visible fence errors do not drain this work or release its holds; a failed reset leaves them held.
  Resources are freed only when the handle is dropped and all holds have ended.
- **Hold sets.** A resource is freed only when its handle is dropped and its hold set is empty. Holders: each
  submission that reads or writes it (until its fence), a committed window buffer and a remnant (until
  retired), the compositor's sampling submission (until its fence), the scanout binding (until another
  target is presented and that present's fence completes), a capture, readback or texture upload (until
  its transfer fence), and an `ACQUIRE` replay copy (until its fence). For each device-access hold above,
  "until its fence" requires both publication of the fence and the end of device access, by actual device
  completion or a successfully confirmed reset. Device completion with the fence still unpublished keeps
  the hold. A caller-visible error synthesized at quarantine or reset does not establish the end of
  device access; merely requesting reset or failing to reset cannot retire the hold. Reporting EIO or
  EINTR does not by itself empty a hold set. After the final hold ends, device unref is
  asynchronous: backing remains allocated and charged until the device answers that unref
  (ruling 49), or a confirmed reset ends device access.
- **The render-completion handoff** (used by BreenGL's windows, section 2.1): each target records the fence of
  the last accepted submission that writes it. `COMMIT` records that fence with the commit and from then on
  refuses submissions that write the committed buffer. The compositor samples a commit only after that fence
  has completed OK; the kernel waits for it on the CPU before sending the compositor's submission, because
  fences on different device contexts are not ordered against each other. A commit whose fence completes with
  an error is refused (the window keeps its previous front and the client gets the commit's failure).

<!-- claim-lint:ok: Normative catch-up contract amendment, rulings 47-52 (#1814), not measured results. -->
**Catch-up check windows (rulings 49, 50 and 52).** `textures/destroy-deferred` waits at
most one second after its sampling fence completes for the asynchronous device unref to release the
backing charge, then requires exactly the original expected `resource_bytes`, not a tolerance.
`present/firmware-handoff` requires zero kernel writes to the firmware framebuffer after handoff on
every target, using the MODES counter defined in `gpu-abi.md`; firmware memory hashes remain diagnostic.
A device can rewrite its own firmware framebuffer while driving the display, so hash equality cannot
prove kernel non-interference. `textures/upload-visibility` starts its 10 s timed window only once
all contexts it needs report catalogue warm-up complete. Warm-up retains the per-compilation watchdog
and kill bounds above; the runner and host still bound unfinished setup. Other case deadlines are unchanged.

### 1.5 The deterministic rendering profile

Device readback makes the pixels observable; the profile is what makes them the same on the three targets'
host stacks. This section is the profile's contract. The closed forms' proofs are added to it before the
`pipelines` cases that draw them are written (item 5), and the `profile` category checks it on each target.
The G0 spikes exercised a subset; binary32 emulation remains unproved (section 1.12):

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
   division by restoring long division with a fixed iteration count (96 steps for a three-word dividend,
   as required by `profile/wide-arithmetic`), so each pipeline's per-fragment cost is
   constant. The width each pipeline needs (section 1.3's table):
   - `line`: `dx`, `dy` < 2³², the error terms < 2³⁴, the closed form's products < 2⁶⁵: three words.
   - `circle_fill`, `circle_outline`: a primitive whose clipped bounding rectangle is empty emits no draw, and
     inside it `|x − cx|, |y − cy| ≤ r < 2³¹`, so the sums are below 2⁶³: two words.
   - `rounded_rect`: `2r < 2³²`, so the squares are below 2⁶⁴ and their sum below 2⁶⁵: three words.
   - `gradient_v`: the numerator `top × (h − 1 − i) + bottom × i + (h − 1)/2` is below 2⁴⁰, divided by
     `h − 1 < 2³¹`: two words. (`255 × h` alone overflows 32 bits.)
   - `triangle`: edge functions of i32 vertices are below 2⁶⁵ in magnitude: three words.
   - `box_filter`: a bin sum is at most `2²⁸ × 255`: two words.
   <!-- claim-lint:ok: Normative arithmetic and conversion requirements in docs/gui/gpu.md section 1.5, proposed by #1714. -->
   Native floats are confined to fragment/vertex positions below 2²⁴, the UNORM fetch boundary and the
   final conversion of an 8-bit value `k` as `k / 255`, which both APIs' UNORM conversion rules round back
   to `k`. A fetched UNORM channel `v` is converted to its integer byte by `floor(v * 255 + 0.5)`;
   subsequent blending and coverage accumulation use integer words. This boundary conversion does not
   change the required texel byte or permit a tolerance. `coverage_spans` uses native float conversion
   only for its integer pixel coordinates and parameter-buffer texel fetches, not coverage arithmetic.
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

#### 1.5.1 Closed forms for G5

<!-- claim-lint:ok: Algebraic derivation of docs/gui/gpu.md section 1.5 item 5; input domain and formula stated here. -->
For a line, let M be the larger absolute endpoint displacement, m the smaller, and k
its major-axis step from the first endpoint. For M > 0 the minor displacement is
`floor((2*k*m + M)/(2*M))`, for `0 <= k <= M`, with each axis following its endpoint's
sign. M = 0 writes the one endpoint. This is the unclipped walk: clipping intersects
its pixel set and never restarts its error accumulator.

<!-- claim-lint:ok: Inductive derivation for docs/gui/gpu.md section 1.5 item 5, with base and recurrence stated here. -->
To prove the formula, write `k*m = q*M + r`, with `0 <= r < M`. The midpoint decision
chooses q when `2*r < M`, otherwise q+1. The initial step has q=r=0. Advancing k adds m
to r, subtracts M at a carry and advances q; its midpoint comparison is exactly the
walk's error comparison, including its equality choice. Induction establishes the
formula for every step and either choice of major axis. The doubled product fits
three words even when both displacements approach 2^32; clipping does not narrow
those operands before the predicate is evaluated.

For the circle outline, reflect the candidate into the first octant, `X >= Y >= 0`.
For radius r >= 1 its membership is
`X*X - X + Y*Y <= r*r < (X+1)*(X+1) - (X+1) + Y*Y`.
At Y=0 the largest satisfying X is r. The expression is strictly increasing in X
for X >= 1, so there is a unique largest satisfying X. Incrementing Y changes it
by `2*Y+1`; decrementing X changes it by `-2*X+2`. Those are the outline walk's
midpoint-error increments, with its equality retained on the lower side. Induction
therefore gives the same recorded X at each Y until X < Y. Reflection gives exactly
the walk's eight symmetric pixels. Radius 0 writes the centre; negative radii are
empty. Both inequalities use two words after bounding |dx| and |dy| by r.

### 1.6 Per-target backends

| Target | Device | Guest backend | Host renderer | Readback (section 1.8) |
|---|---|---|---|---|
| ARM64 QEMU (macOS host) | `virtio-gpu-gl-pci` | VirGL over the PCI transport; the kernel has an ECAM PCI enumeration path on ARM64 (`kernel/src/drivers/mod.rs`, about line 169), which G1 confirms on QEMU's `virt` machine | virglrenderer on ANGLE's Metal backend in UTM's stack (section 1.6.1) | fenced `TRANSFER_FROM_HOST_3D` |
| Parallels | its virtio-gpu PCI device with VirGL | the same clean-room VirGL backend and virtio-gpu command path as ARM64 QEMU (`kernel/src/gpu/virgl/`), in the catch-up (section 1.9) | Parallels' own, on the host's GPU | the same |
| VMware Fusion | SVGA3 | SVGA3D DX contexts, objects and SM4 programs on SVGA3 command buffers, MOBs and GB surfaces (section 1.6.3) | Fusion's own, on the host's GPU | GPU copy of the target into a dedicated readback surface, then `READBACK_GB_SURFACE` |

**VirGL object handles (both VirGL targets).** virglrenderer keeps one object-handle space per context,
shared by every object type a command stream creates: blend, rasterizer and depth-stencil states, shaders,
vertex elements, sampler views, sampler states, surfaces, queries and stream-output targets. The contract
forbids reusing a handle across types: a handle the VirGL encoder has used for one object type is never used
for an object of another type in the same context, even after the first object is destroyed, and the encoder
allocates every object handle from one per-context allocator. The G0 spike found why: when a handle was
reused across types, the host flagged a context error and dropped the creation, the draws that depended on
it did nothing, and the readback showed the target as it was before them, with no error reaching the guest.

**VirGL device facts (both VirGL targets).** The backend depends only on the `VIRGL` and `VERSION_1`
features: Parallels offers no blob resources, context-init or EDID, so nothing in the design needs them.
Per-process device contexts are plain `CTX_CREATE`s beside the display owner's, each with only its own
resources attached (`CTX_ATTACH_RESOURCE`), and a context-owned `BIND_SCANOUT` resource can be the scanout.
A device may list the same capability set at several indices (Parallels lists its VirGL 2 set at four), so
enumeration keeps each capability set id once. Where the capability set carries a renderer string (it does
on Parallels), CAPS and record v2's `renderer` field carry it. Every transfer uses the 256-byte pitch rule
of section 1.3. Silence is the device's failure mode: a dropped transfer, a cross-type handle and a foreign
resource are all answered without an error, which is why readback is judged by complementary poison
(section 1.8) and uploads are proved by reading them back (`textures/upload-visibility`).

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

The smallest guest change. Parallels runs the clean-room VirGL backend the ARM64 QEMU lanes build, with its
per-process contexts, owned resources on frame-allocated backing with the 256-byte pitch (section 1.3), the
catalogue's TGSI programs and atomic validation; `gpu_pci.rs` and `virgl.rs` are not used (section 1.2). The
catch-up's work: the profile checks against Parallels' capset, and the 2560×1600 display setting in
`scripts/parallels/` (the spike ran 2560×1600 targets off-screen; the VM's display mode stayed 1280×960). Parallels is treated as hardware:
behaviour that looks wrong is first reproduced on a Linux guest used as a black-box probe, and a Breenix
defect is assumed until the probe shows otherwise. The 256-byte pitch rule is still to be reproduced on that
probe; the kernel aligns every pitch either way, so the result changes no rule here.

#### 1.6.3 VMware Fusion

The largest guest change: the catch-up writes the SVGA3D DX backend and its SVGA3 command path clean-room in
`kernel/src/gpu/svga/`. `svga3.rs`, which has command buffers, object tables, MOBs, GB surfaces and screen
targets for the existing display and no DX context, shader, view or draw, is not used (section 1.2). The G0
spike exercised the path below, with the present-readback limitation stated in section 1.12. The backend,
in order:

1. **Admission by the device-capability entries.** Read the DX-context entry (index 95) and the entry for
   every profile format: `B8G8R8A8_UNORM` (surface format 141, entry 240) and `R8G8B8A8_UNORM` (68, entry
   167) need supported, sampleable and colour render target (bits `0x7`); `R8_UNORM` (93, entry 192) needs
   supported and sampleable (`0x3`). Initialization fails naming the first that is missing. The register
   capability word's DX bit is not evidence: with 3D acceleration off it stayed set while the DX-context
   entry read 0.
2. **The command path.** Command queue 0 started; byte-addressed command-buffer headers carrying device
   addresses (the kernel's IPA translation); the DX-context header flag on DX commands; global object tables
   set with `SET_OTABLE_BASE64` on explicit 64-bit page-table backing (a range-backed table was refused).
   Command submission and recovery, and the tables' capacities, are written from VMware's published
   definitions and Breenix's own object quotas (the provenance audit's issues #1663 and #1664).
3. **A DX context** defined and bound (`DX_DEFINE_CONTEXT` 1143, `DX_BIND_CONTEXT` 1145) with an 8,192-byte
   context MOB and seven context object tables (`DX_SET_COTABLE` 1207): render-target views,
   shader-resource views, element layouts, blend, depth-stencil and rasterizer states, and shaders. MOBs
   are backed by explicit 64-bit page-number trees.
4. **Surfaces** defined with the version 1 `DEFINE_GB_SURFACE`, single-mip, single-sample and non-array,
   with explicit render-target and shader-resource bind flags; render-target and shader-resource views;
   vertex and parameter buffers. The profile needs nothing the version 2 descriptor adds.
5. **Programs** as bare SM4 token streams followed by VMware's signature header and input and output
   signature entries; the device needs no DXBC container. They are written from VMware's published headers
   and Microsoft's published token format, with no shader compiler. A resource is declared with an opaque
   resource-register operand: a data-register writemask there makes the device refuse `DX_SET_SHADER`.
6. **Draw** (`DX_DRAW` 1152); destination copies with `DX_PRED_COPY_REGION`. **Present** binds the drawn
   target's surface to the screen target and updates it; the device accepts a rendered `B8G8R8A8_UNORM`
   target there. The spike checked command completion and source-surface readback, not screen-target
   readback; the presented-scanout cases still require the latter (section 1.10).
7. **Readback** through the host-copy sequence of section 1.8, fenced with `DX_MOB_FENCE_64`.

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
item for item. The guest side it will need is known now: the gpu core's virtio-gpu command path, queues, DMA
allocation and cache handling, ARM64-only today, generalized to x86-64, with the kernel kept free of FPU use
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
<!-- claim-lint:ok: Normative renderer attestation threshold amended by ruling 51 (#1814); no measured result claimed. -->
- **Parallels.** There is no host hook, but macOS's accelerator statistics attribute GPU time to each
  client process, and every GPU client the VM opens belongs to its VM process. The gate checks the VM's
  configuration has 3D acceleration on, reads that process's accumulated GPU time over an idle window just
  before the `performance` category and over the category itself, and requires the category's reading to
  have a host GPU time rate at least ten times the idle window's rate (ruling 51). Compare
  GPU-time deltas divided by their respective wall-time windows, not absolute deltas or a
  percentage of wall time. A zero idle rate requires a positive category GPU-time delta.
  This is a required acceptance threshold;
  the spike serial does not establish host GPU-time deltas (section 1.12). The negative control,
  the same boot with 3D acceleration off, runs in the Parallels catch-up; the device is expected then not to
  offer VirGL, and initialization refuses naming the missing feature.
- **VMware.** The accelerator statistics give no per-process figure for the VM, so under ruling R6 the
  claim is configuration-based, and the record says so: the gate checks the VM's configuration has 3D
  enabled, and the backend admits the device only by its capability entries (section 1.6.3). The negative
  control ran in G0: with 3D disabled the same kernel refused initialization at the DX-context entry, before
  creating a context or drawing.
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
<!-- claim-lint:ok: Normative presentation requirements amended by ruling 53, #1956; no measured result claimed. -->
- **Present.** Two scanout targets per display owner. Scanout targets are declared at creation:
  `RESOURCE_CREATE` gains a scanout flag (`docs/gui/gpu-abi.md`); only targets created with it get the
  device's `SCANOUT` bind. `PRESENT` refuses a target created without it with EINVAL. `PRESENT` of a
  never-written target is refused with EINVAL: after waiting for its accepted writers, the target must
  have a completed write. `PRESENT` binds the drawn one (`SET_SCANOUT` and
  `RESOURCE_FLUSH` on virtio-gpu; the screen-target update on SVGA3) and returns a fence that completes when
  the device has answered. That fence is a context fence: `PRESENT` names one of the caller's live contexts,
  and its fence is `ns << 32 | n` in that context's fence sequence, shared with its `SUBMIT`s, so `WAIT` on
  that context accepts it and it orders after the context's earlier fences. `PRESENT` never returns a raw
  device-wide fence number, and neither does the observation record (offset 184, section 1.8). There is no vblank on these devices; the present rate is measured, not assumed.
  The display keeps the selected mode when a suite case renders a differently sized target (ruling 53,
  #1956). A smaller target is presented by GPU-copying its whole contents, without scaling, to origin
  (0,0) of a separate display-sized scanout target, clearing the remainder first. Only that
  display-sized target is bound. A target wider or taller than the display is observed by full device
  readback and is never bound as a scanout. These viewing operations do not change the case's source
  dimensions, pixel expectations, work bounds or performance intervals.
  The source observation names the case target and its device resource; the scanout observation names
  the actual display-sized resource and checks the same source pixels at (0,0). They are distinct
  identities when a copy is needed, never an offscreen source reported as the bound resource.
  A minimal scanout bind arrives in G2 (section 1.11), so runs are watchable from then; modes,
  flipping and the ownership stack arrive in G7.
<!-- claim-lint:ok: Normative display-ownership requirements, ruling R7 and ruling F25; not measured results. -->
- **Ownership.** The display ownership stack (`take_over_display` 431, `give_back_display` 432, §18) keeps
  each owner's last presented target; giving the display back presents the previous owner's target again,
  exactly. Whenever no process owns the display, the screen is the kernel's: the console effort's screen
  console (`docs/console/contract.md`), its six virtual terminals with the grid sized for the kernel mode
  (index 1) by the contract's size rule, drawn by the CPU into the backing of one display-sized scanout
  target the core uploads, binds and flushes (`kernel/src/gpu/screen.rs`, ruling F25). A process that owns
  the display through the framebuffer interface has its pane presented through that target too, so no
  kernel writer touches the firmware framebuffer once the core has shown a frame. The core's own console,
  drawn from a glyph atlas uploaded from its built-in font tables (`purpose` `Fixture`) through
  `bitmap_text`, is the panic and diagnostics path only (section 1.7.1). Before the core shows a frame, the
  boot console and panic screen are CPU-written on the firmware framebuffer, and nowhere else (ruling R7).

G7's argument layouts, ownership transitions, console observation, firmware write count and
present fault effects are defined in `gpu-abi.md`, "G7 display interface". These definitions
supply the observability for the existing present cases; their case counts and expectations stay fixed.

Under `--gpu-gl`, G7 shows the kernel's screen (the screen console) until E17 migrates bwm from the legacy
compositor interface. Boots without `--gpu-gl` retain bwm's existing desktop behavior.
G7 does not restore legacy takeover bookkeeping (ruling 39). A take-over on a GPU-core boot still makes the
caller's terminal the display owner's (`KD_GRAPHICS`, switching refused unless `VT_PROCESS`), as the
console contract's section 4.2 requires; while that terminal is released, the kernel's screen shows the
foreground terminal.

#### 1.7.1 Terminal panic display after GPU handoff

<!-- claim-lint:ok: Normative post-handoff panic requirements for #1749, separated under coordinator ruling 9; not measured results. -->
Serial remains the authoritative panic record (ruling 40). Before GPU handoff, the
firmware diagnostics screen follows R7. After handoff, a kernel panic or fatal EL1
fault attempts a GPU-drawn red diagnostics screen containing the headline, location
or fault details, boot stage and recent kernel log. Recoverable EL0 faults do not
enter terminal display mode. The kernel must not write the firmware framebuffer
after handoff, including when the GPU attempt fails.

<!-- claim-lint:ok: Normative terminal ownership and deadlines for #1749; not measured results. -->
The first fatal caller latches terminal mode for the rest of the boot. Normal GPU
producers stop without releasing backing the device may still access; later fatal
callers cannot become another queue writer. The attempt uses prepared, pinned
resources and requests, without allocating or waiting for the ordinary core,
console or framebuffer locks. Other CPUs' active queue accesses have at most
100 ms to drain. If they do not, the GPU attempt ends. After that drain, queue
drain, draw, bind, flush and any capture-profile device observation share one
2 s deadline measured by the architectural counter, independent of interrupts.
Timeout, unavailable preparation, reset-invalidated state or a device error leaves
serial as the record; it does not permit a CPU-rendered fallback or a retry on the
normal queue. Requests and DMA backing stay pinned after a timeout.

<!-- claim-lint:ok: Normative terminal-stop and acceptance requirements for #1749; not measured results. -->
Terminal mode does not resume scheduling or display ownership. CPUs parked by this
fatal stop mask interrupts; the soft-lockup detector must recognize that intentional
terminal state and must not append a LOCKUP capture to the panic record. This does
not exempt a lockup before terminal mode or change ordinary lockup detection. The
panic capture profiles must read the bound panic target through both complementary
poisons, require exact equality to the expected diagnostics pixels, and end with
no subsequent soft-lockup capture. Screen failure never suppresses the serial
panic record. This terminal path adds no syscall, userspace recovery operation or
change to the ordinary watchdog, fence, readback or IRQ-masking bounds.

### 1.8 Observation by device readback

`READBACK` and BreenGL's `SCANOUT_READBACK` share one record, `ObservationRecord` version 2: 1,024 bytes,
little-endian, reserved bytes zero, produced atomically with the pixels it describes.

`READBACK` allocates temporary whole-rectangle staging in guest-backing frames. The staging is charged
to the process's 192 MiB guest-backing quota and `RLIMIT_AS`, and `COUNTERS` reports the staging under
`resource_bytes` while it is held. `READBACK` may fail with ENOMEM when staging cannot be charged to
the process's guest-backing quota or `RLIMIT_AS`. The charge is released when the staging is freed.

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
| 56 | `stride: u32`: of the caller's buffer; `canonical: u32`: 0, BGRA bytes; 1, one byte per pixel (an R8 texture) |
| 64 | `context: u64`: the device context that last wrote the target (the compositor's for a composite) |
| 72 | `target: u64`: the caller's handle; `resource: u64` at 80: the device's resource or surface id |
| 88 | `generation: u64`: the target's published generation (section 1.4.1) |
| 96 | `render_serial: u64`: the submission that produced that generation; `render_fence: u64` at 104 |
| 112 | `render_status: u32`, `render_kind: u32`: the answer and the command answered (`SUBMIT_3D` on VirGL; the command buffer's status and `DX_DRAW` on SVGA3D) |
| 120 | `readback_fence: u64`; `readback_status: u32`, `readback_kind: u32` at 128 (`TRANSFER_FROM_HOST_3D`; `READBACK_GB_SURFACE`) |
| 136 | `readback_surface: u64`, `readback_mob: u64` at 144: VMware's dedicated readback surface and its MOB; 0 on VirGL |
| 152 | `copy_kind: u32`, `copy_status: u32`: VMware's host copy into the readback surface; 0 on VirGL |
| 160 | `poison: u32`: the first observation's poison pattern (the second's is its complement); `unwritten: u32` at 164: bytes whose two observations differ, which must be 0 |
| 168 | `commit: u64`, `composite_serial: u64` at 176: for a window or composite observation, else 0 |
| 184 | `present_fence: u64`: when bit 0 of `flags` is set, the context fence `PRESENT` returned for the completed binding that made the observed target the scanout (section 1.7), never a raw device fence number; else 0. `present_status: u32`, `window_count: u32` at 192 |
| 200 | `windows: [(window: u64, commit: u64, render_fence: u64, generation: u64); 16]`: every window in the composite, the commit sampled, the render fence that commit carried and the generation sampled |
| 712 | `renderer: [u8; 64]`: the device's renderer string from its capability set, NUL-padded, empty if none |
| 776 | reserved |

<!-- claim-lint:ok: Normative catch-up contract amendment, rulings 47-52 (#1814), not measured results. -->
**Backend-specific observations (ruling 47).** A record's backend must match `CAPS.backend`;
checks select the following native values by that backend, never accept a value from the other one.
These are interface identifiers from VirtIO section 5.7.6.7 and VMware's published
`svga3d_cmd.h` and `svga_reg.h` (section 1.9), not rendering-policy sources.

| Field | Backend 4 (VirGL) | Backend 5 (SVGA3D DX) |
|---|---|---|
| Successful render, readback, copy and present status | `OK_NODATA` 0x1100 (copy fields remain 0) | `SVGA_CB_STATUS_COMPLETED` 1 |
| Render kind after drawing | `SUBMIT_3D` 0x207 | `DX_DRAW` 1152 |
| Render kind after texture upload | `TRANSFER_TO_HOST_3D` 0x205 | `UPDATE_GB_SURFACE` 1102 |
| Readback kind | `TRANSFER_FROM_HOST_3D` 0x206 | `READBACK_GB_SURFACE` 1104 |
| Readback surface and MOB | both 0 | both nonzero; dedicated readback surface differs from the judged resource |
| Copy kind | 0 | `SURFACE_COPY` 1042 |
| Forced device-error answer | VirtIO error response 0x1200 through 0x12ff | `SVGA_CB_STATUS_COMMAND_ERROR` 3 |

<!-- claim-lint:ok: Normative catch-up contract amendment, rulings 47-52 (#1814), not measured results. -->
Pixel equality, both complementary poisons, fence and resource identities, all performance bounds,
and the same 170 cases with zero skips remain required on both backends.

A readback refuses with EIO and a named reason (the codes are in `docs/gui/gpu-abi.md`) when the two
complementary-poison observations differ (unwritten), when any recorded fence belongs to another submission
(fence mismatch), when a composed window's render fence had not completed OK before the composite was sent
(its code is assigned with composite observations, which come after G2), when
an observation claiming the scanout (record `flags` bit 0 set) names a resource that differs from the
device's confirmed scanout binding (resource mismatch), when the target is invalid (section 1.4.1; the
reason names the submission that invalidated it), when the device
answers a readback transfer with an error, when the watchdog ends a transfer the device has not answered
(device hung), when a device reset is decided while a transfer is outstanding, whether the reset then
completes or fails (device reset), when the target was lost to an earlier device reset (target lost), when
the target's handle was destroyed while the readback waited for its writers (target dropped), and when the
target may be the scanout but the scanout's binding is not known because a `PRESENT` that binds it or
replaces it was sent and has not completed, or failed (binding unknown), and when a composite has more than
16 windows (composite-overflow; its code is assigned with composite observations, which come after G2).
Every EIO a readback returns names one of these reasons; none is reported under another's name.
Composite overflow returns no partial record, as v1 does.

**A fault is detected from what can be observed, never from the fault variable.** A readback's reason comes
from what the kernel observes: the two observations, the answer and fence echo the device returns, and the
kernel's own record of the target and the binding. It never comes from knowing that a test-control point
fired. The device's transfer answer carries no resource identity, so the kernel cannot know which resource
the device copied from: a transfer that names the wrong resource writes that resource's backing, the
observed target's poisoned backing stays unwritten, and the readback refuses with reason unwritten (1).
Resource mismatch (3) is not observable for that fault.

| Target | Sequence the guest runs |
|---|---|
| VirGL (ARM64 QEMU, Parallels) | fenced `SUBMIT_3D` of the frame; poison the attached backing with P; fenced `TRANSFER_FROM_HOST_3D` of the read rectangle into it; invalidate the CPU cache over it and keep the bytes; poison with the complement of P, transfer and invalidate again; refuse when any byte differs between the two |
| SVGA3D DX (VMware) | complete the draw; GPU `SURFACE_COPY` from the judged target into a dedicated readback surface on its own MOB, in the same command buffer; poison the MOB with P; `READBACK_GB_SURFACE`; `DX_MOB_FENCE_64` and wait for its value; invalidate and read the MOB; repeat the copy and readback with the complement of P; refuse when any byte differs between the two |

**Two observations under complementary poisons.** A byte the device wrote is the same in both observations;
a byte it did not write keeps each poison and so differs. One poison cannot tell an unwritten byte from a
pixel whose expected value equals the poison, and any RGBA value can occur, so every readback on every target
makes both observations and counts the differing bytes into `unwritten`. Silence is how these devices fail
(section 1.6): the Parallels spike caught a dropped transfer only through its poison, and the VMware spike
judged every target this way.

The VMware row extends the sequence BreenGL recorded for host-modified contents (§20.2): `READBACK_GB_SURFACE`
writes nothing for a surface whose contents came only from guest uploads, and a host copy first makes it write
the whole surface. The VMware spike read shader-rendered targets back exactly through this sequence.

<!-- claim-lint:ok: Normative one-shot fault requirements from docs/gui/breengl.md section 21 and the GPU contract #1709, not measured results. -->
**Planted faults** (test controls, §21, new points 19 to 31), each with a `gpu` case that requires the exact
reason and then the clean result once disarmed. Every point is one-shot, as every point of §21 is: arming it
affects the next occurrence of its event, the point then counts one firing and disarms itself, and a check
that needs the fault on several operations arms it before each. 19 `skip-transfer` (a skipped transfer-back),
20 `stale-fence` (a fence answered for another submission), 21 `wrong-resource` (the next readback transfer
names another target of the same process that contains the transfer's box, so the box never extends past the
resource it names, section 1.4.1; the case creates that target, and with none the point does not fire and
stays armed. The device writes that target's backing, the observed target's poisoned backing stays
unwritten, and the readback refuses with reason unwritten (1), the reason `readback/fault-wrong-resource`
requires; resource mismatch (3) is not observable for this fault, as stated above), 22 `corrupt-pixel` (a pixel outside the draw corrupted on the device), 23 `mask-capability` (the
next admission decision, made by a `CAPS` query or by any other op that admission gates, counts the named
capability as absent, so that op fails naming it; the op after it sees the device's real capabilities, with no
`DISARM_ALL`), 24 `profile-violation` (the next DRAW, DRAW_BUFFER or DRAW_SAMPLED is refused by validation
for filtered sampling; arm with target 0 and parameter 0, with no BEGIN-target filter, as defined in
`docs/gui/gpu-abi.md`), 25 `skip-present-flush`, 26
`wrong-scanout`, 27 `device-error` (the next submission is answered with a device error; armed before each of
two consecutive submissions of one context, it quarantines that context only and never resets the device,
section 1.4.1), 28 `suppress-draw` (the translator drops one draw from the device stream while the fence still
completes), 29 `wrong-texture` (one draw binds another texture the same process owns), 30
`skip-upload-visibility` (one upload's transfer to the host is omitted), 31 `withhold-fence` (one fence's
completion is ignored, so the watchdog fires; a real host hang cannot be induced from inside the profile,
whose shader loops are all bounded, so this point simulates one). Point 31 also has a fence-depth mode: the
device's answers to the next context's first four submissions are taken and kept but not published, so their
fences stay incomplete, until a fifth submission on that context waits at the four-in-flight limit; then the
four kept answers are published in fence order and the fifth is admitted. It invents no completion and changes
no watchdog deadline (`submit/ring-backpressure`, section 1.4.1).

There is no planted missing-cache-invalidation fault: on every target the "device" is host software
writing guest memory through the host's coherent memory system, so omitting the invalidation changes no
bytes. The invalidation stays as required code, and a structure test checks that every readback path calls
it between the transfer fence and the first read.

### 1.9 Clean room and provenance

The rule is the libc rule applied to graphics. **Allowed:** the VirtIO 1.3 specification; the VirGL
protocol's public interface definitions, `virgl_protocol.h` (command numbers, field layouts), `virgl_hw.h`
(the capability-set layout and format numbers), and `p_defines.h` and `p_shader_tokens.h` (the
enumeration values and token layouts the VirGL wire carries), used for numeric values, enumerations and
structure layouts only, never for code, control flow or policy; for four virtio-gpu requests only, the
BSD-licensed `virtio_gpu.h` interface header (below); Gallium's published
[Tungsten Graphics Shader Infrastructure documentation](https://docs.mesa3d.org/gallium/tgsi.html), solely
its language and instruction definitions, not Gallium implementation code; VMware's published interface
headers (`svga3d_cmd.h`, `svga3d_dx.h`, `svga3d_devcaps.h` and siblings) in the
[Linux source tree's `drivers/gpu/drm/vmwgfx/device_include` directory](https://github.com/torvalds/linux/tree/master/drivers/gpu/drm/vmwgfx/device_include)
(the VMware-authored interface headers only, not the surrounding vmwgfx driver C code); Microsoft's
[`DirectX-Headers/include/directx/D3D12TokenizedProgramFormat.hpp`](https://github.com/microsoft/DirectX-Headers/blob/main/include/directx/D3D12TokenizedProgramFormat.hpp)
for SM4 instruction and operand encodings; the OpenGL and Direct3D specifications for
rasterization, conversion and precision rules; IEEE 754 for binary32; Breenix's own code; black-box
experiments (a Linux VM run as a device-behaviour probe, recorded as results). **Forbidden, including while
debugging:** reading, porting or adapting Mesa (its VirGL and SVGA drivers, Gallium, the state trackers),
virglrenderer's renderer sources, Linux's virtio-gpu or vmwgfx driver C files, and ANGLE's implementation;
and using byte sequences captured from another guest's driver as templates. Host software (QEMU,
virglrenderer, ANGLE, Mesa as the host renderer) runs unmodified or with Breenix's own host patches and is
never linked into the guest. Host-stack work reads only build files and QEMU's and UTM's display and
frontend integration code, and its lanes do not author guest GPU code.

**virtio-gpu commands cite the specification.** Every virtio-gpu command, structure and flag the core uses
cites its VirtIO 1.3 section, with one exception. VirtIO 1.3 section 5.7.6.9 names `CTX_ATTACH_RESOURCE`,
`RESOURCE_CREATE_3D`, `TRANSFER_TO_HOST_3D`, `TRANSFER_FROM_HOST_3D` and `SUBMIT_3D` but gives no request
layout for them, as it does for `CTX_CREATE`. For those four request layouts only (attach, 3D resource
creation, the 3D transfer layout both transfer directions use, and 3D submission), the field names, order and
widths come from the Linux uapi `virtio_gpu.h`, the BSD-licensed interface definition of the device, treated
exactly like `virgl_protocol.h`: interface facts only, never code, control flow or policy. Each field is
recorded with its line, and each layout is confirmed black-box on the device (it accepts the request and
returns the expected data). `virtio_gpu.h` is not a source for any other command.

**Shader text.** The kernel's programs are written from the TGSI documentation's instruction
descriptions. The text form virglrenderer accepts is learned black-box: a candidate is submitted and
accepted or refused, and each grammar fact (the label format among them) is recorded with the experiment
that showed it. A minimal program that comes out identical to an older one in the tree, such as a
pass-through, is acceptable when its derivation is recorded that way; no older program is a template.

**The provenance record.** Every value the core uses (a number, an enumeration, a structure layout, a
grammar fact) names its source in `docs/gui/provenance/gpu-core.md`: the specification section, or the
interface header and its line range at a pinned revision, or the black-box experiment, or Breenix's own
design. A value with no recorded source is not landed.

Every black-box experiment records the commit, the Vigil run id or harness invocation, and the serial
or output excerpt that showed the fact. Reproduce commands match the runs they cite: the provenance
record's commands use exactly the flags the cited runs used, run folders keep `host-renderer.json`, and
`scripts/vigil-record.sh` records whether the boot had a display window.

`docs/planning/gpu-rendering-attack-plan.md` proposes capturing and porting exact byte sequences; that
approach is void.

**The provenance audit comes first and gates reuse.** It is the first G0 task, and it covers every file the
harvest or BreenGL's rebuild reuses: `virgl.rs`, `gpu_pci.rs`, `svga3.rs` and the rest of `drivers/vmware/`,
every shader program text or token array already in the tree, the libgfx algorithms that move to `libgfx-ref`,
and libfont's rasterizer. The gpu core reuses none of the driver files (section 1.2). Each encoding, structure
layout, shader and algorithm gets its source (a section of an allowed document, or Breenix's own design);
anything whose only source is a captured stream or a forbidden implementation is rewritten from an allowed
source before reuse. The audit's record per file lives in `docs/gui/provenance/`. A harvest PR merges only
after its files' audit record has merged, and every PR in the effort lists the sources its new code was
written from, which review checks against the forbidden list.

The audit of the four driver files (`virgl.rs`, `gpu_pci.rs`, `svga3.rs`, `drivers/vmware/mod.rs`) ran in
G0. It keeps their published protocol facts (command numbers, wire layouts, enumerations) and Breenix's own
code, and names six parts to re-author from allowed sources before any reuse: VirGL's fixed pipeline state
(#1659), the embedded TGSI programs and their token counts (#1660), the VirGL batch and quirk policy
(#1661), the VirtIO owned-command DMA layer (#1662), SVGA command submission and recovery (#1663), and the
SVGA object tables' sizes and capacities (#1664). These concern the drivers' own uses (the existing display
path and the harvest); the gpu core writes its equivalents from the allowed sources. libgfx, libfont's rasterizer and any shader outside those
files still get their own audit before they move. The spikes' new code was written from the allowed list
only: the VirGL protocol's interface facts and Gallium's TGSI language documentation for Parallels, and
the VMware interface headers and Microsoft token-format header named above for VMware. These are
interface facts; implementation code is not copied or adapted.

### 1.10 Suite `gpu`: outline

`/sbin/suite-gpu`, source `userspace/programs/src/suite_gpu.rs` (the case table; case modules go under
`userspace/programs/src/suite_gpu/` as they are written), manifest `docs/suites/gpu.json`, contract this
document, zero skips, gate timeout 900 s on every target until measured (plan §19's rule). Every case runs on
every target; a case whose evidence differs per backend names the backend's row, as `breengel`'s
`scanout/device-path` does. Expectations are literal tables or formulas computed in the suite crate, or
`libgfx-ref` output where section 2.4 allows it; none comes from BGL or the kernel.

**Before its layer exists** each case fails naming the layer it needs and the build step that supplies it,
with what the kernel answered syscall 416's VERSION op, for example `missing the line pipeline (G5):
syscall 416 VERSION returned ENOSYS`. Once VERSION answers, a case whose check has not been written yet fails
saying so. A lane writes each case's check in its own commit, from this contract, before the implementation
it measures; `device/version` is already its real check. ARM64 QEMU boots of the suite use the GL QEMU
(`scripts/boot-interactive.sh --mode suite --suite gpu --gpu-gl`).

<!-- claim-lint:ok: Normative suite viewing requirements amended by ruling 53, #1956; no measured result claimed. -->
**No CPU canvas in the suite.** The suite does not use `Suite::gui()`, `suite_stage` or
`suite::judged(&FrameBuf)`, which rasterize and compare CPU framebuffers. It judges with
`suite::judged_readback(&Readback, &Expected)` over read-back bytes and their record (comparison, not
rendering). From G2 each rendering case shows its result while it owns the display, using section 1.7's
viewing path (ruling 53, #1956). A smaller case target is GPU-copied at origin (0,0) into a
separate display-sized scanout; an oversized case retains its full offscreen readback and never binds
that target. Source and scanout observations retain their actual resource identities; scanout checks
judge the source pixels at that origin and the completed present fence of the display-sized resource.
The 17x9 record-fields source remains 17x9 with scanout bit clear, and its separate scanout record has
the display dimensions, scanout bit set and the present fence. A viewing copy consumes its own SUBMIT
fence before PRESENT; fence-sequence checks include that operation. Direct scanout fault and ownership
checks use display-sized targets. The runner draws a status strip of solid tiles (one per case, pass or fail colour) with BGL's `solid` pipeline;
case names stay on serial. Text on the stage arrives with BreenGL's GPU stage. Until G2 the shared runner
draws its usual scored panel through the existing display path, as every program on main does (section 1.1),
and judges nothing with it.

| Category (milestone) | Cases | Case ids |
|---|---|---|
| `device`: 3D device & capability profile | 12 | `version`, `feature-negotiated`, `caps-parsed`, `profile-formats`, `profile-integer-ops`, `profile-limits`, `profile-record`, `missing-capability-fails`, `no-fallback`, `display-info`, `backend-named`, `device-error-reported` |
| `submit`: command submission & fences | 22 | `context-create`, `context-destroy`, `context-limit`, `stream-validate`, `validation-atomic`, `stream-overflow`, `fence-order`, `fence-wait`, `wait-timeout`, `wait-eintr`, `wait-killable`, `fence-foreign`, `multi-context`, `ring-backpressure`, `device-error-eio`, `recovery-after-error`, `frame-one-syscall`, `freed-resource-refused`, `fence-echo-checked`, `counters`, `watchdog-quarantine`, `device-reset-recovers` |
| `readback`: device readback | 19 | `full-target`, `poison-unwritten`, `scenes-aba`, `partial-rect`, `padded-stride`, `format-canonical`, `record-fields`, `device-path`, `fence-identity`, `ordered-after-draws`, `fault-skip-transfer`, `fault-stale-fence`, `fault-wrong-resource`, `fault-corrupt-pixel`, `fault-suppress-draw`, `error-invalidates-targets`, `owner-only`, `efault`, `large-target` |
| `textures`: textures & uploads | 19 | `create-formats`, `create-limits`, `upload-full`, `upload-subrect`, `upload-clip`, `upload-stride`, `upload-3byte`, `upload-visibility`, `unaligned-pitch`, `copy-region`, `copy-overlap`, `immutable-while-pinned`, `destroy-deferred`, `quota`, `r8-atlas`, `readback-uploaded`, `efault-upload`, `fault-wrong-texture`, `fault-skip-upload` |
| `profile`: deterministic rendering profile | 22 | `clear-exact`, `unorm-roundtrip`, `texel-fetch-exact`, `integer-ops`, `wide-arithmetic`, `binary32-emulation`, `int-float-exact`, `fragcoord-integer`, `rect-coverage`, `rect-edges`, `scissor-exact`, `viewport-exact`, `alpha-write`, `swizzle-bgra-rgba`, `no-dither`, `no-srgb`, `determinism`, `cross-context`, `large-coordinates`, `filtered-sampler-refused`, `fixed-blend-off`, `profile-violation-detected` |
| `pipelines`: drawing pipelines | 26 | `solid-rects`, `rect-batch`, `copy`, `copy-clip`, `copy-scaled`, `over-premultiplied`, `over-straight`, `over-overlap`, `coverage-blend`, `coverage-lcd`, `coverage-spans-square`, `coverage-spans-winding`, `coverage-spans-corpus`, `bitmap-text`, `line-bresenham`, `line-clip`, `line-extreme`, `circle-fill`, `circle-outline`, `rounded-rect`, `gradient`, `triangle-integer`, `box-filter`, `nearest-scale`, `clip-nested`, `order` |
| `isolation`: per-process isolation | 25 | `handle-private`, `handle-guess`, `fork-no-handles`, `exec-reclaims`, `exit-reclaims`, `kill-mid-submit`, `killed-work-drains`, `quota-memory`, `quota-handles`, `quota-contexts`, `work-estimate-refused`, `foreign-target-draw`, `foreign-readback`, `display-context-unreachable`, `no-addresses`, `fuzz-streams`, `bounded-validation`, `efault-submit`, `efault-readonly`, `quiet-errors`, `unknown-op`, `test-hooks-absent`, `killable-wait`, `client-cannot-scanout`, `irq-masked-bound` |
| `present`: modes, HiDPI & present | 16 | `modes-listed`, `mode-1280x800`, `mode-2560x1600`, `scale-reported`, `scanout-bind`, `flip`, `present-completion`, `present-owner-only`, `readback-presented`, `takeover-giveback`, `ownership-stack`, `console-restored`, `firmware-handoff`, `mode-change-content`, `fault-skip-flush`, `fault-wrong-scanout` |
| `performance`: frame rate & latency | 9 | `clear-rate-2560`, `rect-throughput`, `submit-latency`, `first-draw-latency`, `readback-time-2560`, `upload-bandwidth`, `coverage-throughput`, `wait-no-spin`, `present-rate-2560` |
| **Total** | **170** | |

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
- `readback/owner-only` checks that a live non-owner gets EPERM for READBACK target 0
  while another process owns the display. After the child owner exits, the restored parent
  can read back target 0; the restored parent is not the non-owner probe (ruling 37).
- `isolation/client-cannot-scanout` expects EPERM from a non-owner SET_MODE with valid
  arguments. Ownership is checked before argument validation, so malformed arguments also
  return EPERM for a non-owner. The current owner's SET_MODE(NULL, 0) returns EINVAL
  (ruling 38); ENOSYS is no longer the G7 expectation.
- `readback/poison-unwritten` checks the two complementary observations of section 1.8, and
  `fault-skip-transfer` shows a skipped transfer is refused through them. `textures/upload-visibility` reads
  an upload back as well as sampling it, because a dropped upload samples as a texture of zeros.
  `textures/unaligned-pitch`, `upload-stride`, `r8-atlas`, `readback/partial-rect` and `padded-stride` each
  include a width whose natural row is not a multiple of 256 bytes (R8 64 wide, BGRA 48 wide).
- `isolation/foreign-target-draw` and its siblings expect EINVAL from the validator, never a device error
  (section 1.4.1).
- `isolation/fuzz-streams` submits 10,000 seeded malformed streams; each must be refused with EINVAL and its
  failing index, the device must stay alive, and the next valid frame must read back exactly.
  `work-estimate-refused` submits a stream one step over the cap. `irq-masked-bound` requires the longest
  IRQ-masked interval in the kernel's GPU paths (validation, translation, submission, upload and readback
  copies, fence completion, the watchdog) under a full-screen workload to stay within 500 µs, after the
  existing `irq-hold` point has shown the gauge reads a 2 ms hold, as `breengel`'s
  `composition/flush-irq-bound` does.

<!-- claim-lint:ok: Normative performance measurement requirements in docs/gui/gpu.md sections 1.4.1 and 1.10, amended by contract #1709, not measured results. -->
**Performance bounds,** in the contract from G0 and held on every target. G8 may tighten a bound from
measurement and never loosens one; a target that misses a bound has a slow path to fix. A device's answer to
a submission is not proof the GPU did the work: opaque overdraw may be culled, and a response need not
prove pixel completion. Every measured interval ends with a dependent observation (a readback, or a draw
that reads the result), and throughput cases use blended or discarding draws wherever
opaque overdraw could be culled. Every bound is measured after the context reports its asynchronous fixed
rendering-catalogue warm-up complete through `COUNTERS` (ruling 33, #1725). Each profile-only routine's program warms
on that routine's first PROBE in its context; that first-use cost is excluded from performance
bounds (section 1.4.1, ruling 48).

| Case | Bound |
|---|---|
| `clear-rate-2560` | at least 60 full-screen 2560×1600 clears, each presented, per second |
| `rect-throughput` | 10,000 solid 16×16 rectangles in one submission complete within 50 ms |
| `submit-latency` | an empty submission's fence: median at most 2 ms, maximum at most 10 ms over 1,000 |
| `first-draw-latency` | a new context's first draw with each fixed rendering-catalogue pipeline (excluding `probe_int`), issued after that context reports warm-up complete, each completing within 10 ms |
| `readback-time-2560` | a full 2560×1600 readback within 100 ms |
| `upload-bandwidth` | a 16 MiB texture upload visible to the next submission within 100 ms |
| `coverage-throughput` | a full-screen 2560×1600 `coverage` blend from an R8 mask within 16.7 ms |
| `wait-no-spin` | CPU time while waiting on a fence at most 5% of the wait |
| `present-rate-2560` | at least 60 presents per second alternating two full-screen targets |

### 1.11 Milestones and build order

**G0 (host, audit, spikes and contract; measured by its acceptance checks, not by the suite).** In this
order where one depends on another:

1. the provenance audit (section 1.9): done for the four driver files, with six parts to re-author
   (#1659 to #1664); the harvest PRs to main follow as their files' records merge (section 3.3);
2. the macOS GL host stack and its acceptance (section 1.6.1), with the attestation patch (section 1.6.5):
   done (#1666);
3. the entry spikes, one throwaway boot sequence per non-QEMU target on a `scratch/*` branch cut from main,
   so no effort-lane commit is involved and the target guard passes (ruling R2): Parallels (the profile's
   capability set on a client render target, read back exactly) and VMware (the whole profile, section
   1.6.3): the device primitives were exercised, but full-profile acceptance remains unproved; section
   1.12 lists the untested items. The acceptance requirement remains the whole profile;
4. this document's contract, frozen after the spikes reported: BGL, syscall 416, the profile with its width
   derivations, the catalogue, record v2, the test-control points and the performance bounds. Two additions
   are due before the cases that need them, and change no frozen rule: the closed-form proofs of section
   1.5, item 5 (before `pipelines/line-*` and `circle-outline`), and each program's specification citations
   (with the program, in G4 and G5);
5. `docs/suites/gpu.json` and the suite with all 170 cases, each failing with a specific "missing …" reason.

The `gpu` entry in `docs/efforts/path.json` (section 3.2) landed with this document.

**Lanes on ARM64 QEMU (in order; each milestone is its category passing on ARM64 QEMU, and each step supplies
everything its category's cases use).** Lanes land into the integration branch `gpu/integration`, cut from
main after the `gpu` suite merged, never into main; main receives the effort once it passes on ARM64 QEMU,
Parallels and VMware. A contract amendment a lane needs lands in its own PR into `gpu/integration` before
that lane, and the lane then conforms to it.

| Step | Milestone | Work |
|---|---|---|
| G1 | `device`, `submit` | `virtio-gpu-gl-pci` on the PCI transport, beside the MMIO `virtio-gpu-device` that keeps the existing display until G7; the clean-room kernel GPU core (`kernel/src/gpu/`, section 1.2): contexts, handles, validation, buffer snapshots, the work estimate, fences, waits, the watchdog over every synchronous device wait, per-context quarantine and device reset (section 1.4.1); syscall 416 on ARM64; test points 20, 23, 27, 31 (with its fence-depth mode) and the PID 1 guard for every reserved point 19–31; the provenance record `docs/gui/provenance/gpu-core.md` for every value the core uses |
| G2 | `readback` | `READBACK` with record v2 and points 19, 21, 22, 28 (reusing point 20 from G1); `solid`; target invalidation; single-target `PRESENT`, its fence and killable blocking path, scanout bind and status strip |
| G3 | `textures` | uploads with purposes, region copies, `copy`; points 29, 30 |
| G4 | `profile` | `probe_int` with the integer, multi-word, binary32 and shape-predicate routines; the profile checks; point 24 |
| G5 | `pipelines` | the rest of the catalogue in TGSI, the destination copy and hazard rule, BGL's draw API |
| G6 | `isolation` | quotas, including the per-submission copy quotas (the private copy of the command stream, the buffer snapshots, the translated device chunks and device vertex storage), reclamation at exit/exec/kill, the fuzzer, quiet errors, no addresses, the IRQ gauge over the GPU paths |
| G7 | `present` | modes, scale, two-target present, the ownership stack on the GPU, the kernel console on the GPU, the firmware handoff; points 25, 26 |
| G8 | `performance` | measure against the bounds, fix the slow paths |

Deferred by name, not dropped: per-submission copy quota charging is G6's (above). G1 supplies the
buffer snapshots and the watchdog over warm-up waits now, and G2 enrolls `READBACK`'s transfers when it adds
them, because those bound the work and the kill path from the first lane that has them.

<!-- claim-lint:ok: Normative test-control guard required by ruling 34 for contract PR #1729, not a measured result. -->
G1 reserves points 19–31 and checks the boot latch plus a root caller before decoding any argument,
including for points whose implementation is supplied later. `isolation/test-hooks-absent` requires these
points to be refused to every non-root process: STATE, ARM, FIRED and DISARM_ALL return EPERM to a
non-root caller, even with malformed arguments. The boot latch is set when the first PID 1 is a
`/sbin/suite-*` binary; the caller need not be PID 1. G6 checks this guard without requiring G7's present
fault implementations. `isolation/killable-wait` uses G2's single-target PRESENT;
G7 adds two-target flipping and mode changes, not the first PRESENT operation.

**Catch-up (each target until all 170 pass there, with its attestation):** Parallels (the same clean-room
backend; Parallels capset and display configuration), then VMware (the SVGA3D DX backend and its programs, designed
against the G0 spike). x86-64 is not in the catch-up (section 1.6.4). The effort is done when one run per
target has all 170 passing with an accepted attestation; `gpu`'s landing PR also switches every ARM64 gate to
the GL binary (section 1.6.1, ruling R5).

**Sizing (planning guesses, not measurements and not budgets).** The `breengel` effort went from its plan
(2026-10-15) to 393/393 on three targets (2026-10-16), with much of its library code already in the tree;
most of this work's code does not exist yet, and the items below are what makes it larger.

| Item | Guess |
|---|---|
| G0 host stack: UTM's build turned into a CLI binary, the attestation patch, signing, both frontends | 1 to 2 weeks (2 more if the SDL fallback is needed) |
| G0 audit, full-profile spike acceptance, contract and suite skeleton | 1 to 2 weeks, partly in parallel with the host stack |
| G1–G2: kernel core, validation, work estimate, watchdog and reset, readback | 1.5 to 3 weeks |
| G3–G5: uploads, the catalogue in TGSI including multi-word arithmetic, binary32 emulation, destination copies and the hazard rule | 2 to 4 weeks |
| G6–G8: isolation, present and HiDPI, performance at 2560×1600 (four times the pixels of 1280×800) | 1.5 to 3 weeks |
| Catch-up: Parallels; VMware's DX backend and SM4 programs | 0.5 to 1; 2 to 4 weeks |
| BreenGL rebuild: drawing layer, text on `coverage_spans`, windows and GPU composition, `libgfx-ref` and its tests, porting 8 programs, catch-up on three targets | 4 to 8 weeks |
| Host stack upkeep (pins, UTM and macOS updates) | ongoing, after landing |

Together, roughly 3.5 to 7 months of calendar time to #1625 landing. These are guesses until G0 and the spikes
report, and are revised then.

### 1.12 What the G0 spikes changed

The spikes are device-primitive experiments, not certification of the whole profile or catalogue.
The Parallels source is `kernel/src/drivers/virtio/gpu_pci/spike.rs` on
`scratch/gpu-spike-parallels`, commit `7be7d956b9a7436e7dbbf5cf5a480899d6da93dc`;
Vigil run `20261016T205350Z-parallels` holds its serial. The VMware source is
`kernel/src/drivers/vmware/svga3/spike.rs` on `scratch/gpu-spike-vmware`, rendering commit
`f3d6f9c0f89ad0b6c7405a7149614076ebc4e739`; Vigil run `20261016T201245Z-vmware`
holds its serial. These scratch branches are not merged into the effort.

Parallels' `GPU_SPIKE DONE passed=12 failed=0` covers its judged scenes, including a 2560×1600
upload and render, and catches corrupted-pixel, suppressed-draw and skipped-transfer faults.
VMware's `GPU_SPIKE DONE passed=34 failed=0` counter comprises 32 normal pixel judgements,
one suppressed-draw catch and one completed present submission counted without a pixel judgement.
Only 33 entries are judgements. Its `presented-target` judgement reads the bound source surface,
not the screen target, so it does not prove scanout contents. Normal VMware judgements use two
complementary-poison observations of 256×256 targets.

Neither spike implements profile item 6, integer emulation of binary32. Parallels exercises a
two-word multiply and native `UDIV`, not the required multi-word restoring division. VMware
exercises three-word restoring division, but neither spike proves every shape predicate, the
future catalogue, isolation, recovery or performance bounds. Those remain required suite checks;
no profile requirement is relaxed by these experiments.

| Decision | Where | Observation and limit |
|---|---|---|
| VirGL transfer pitch is aligned to 256 bytes; VMware surface rows stay packed | section 1.3; `textures/unaligned-pitch` and stride cases | Parallels' `pitch-probe` round trips fail at 128, 192 and 320 bytes and succeed at 256 and 512. Upload and readback use the same pitch, so these probes do not locate the failing direction. VMware's `surface` sizes its MOB from tightly packed pixels; descriptor 1097 has no pitch field. |
| GPU backing is frame-allocated | section 1.3 | A full-size resource needs storage independent of kernel heap capacity. This is an allocation requirement, not a profile result. |
| Every readback has complementary-poison observations; offset 164 is `unwritten` | section 1.8; `readback/poison-unwritten` | VMware's `judge` compares both observations; a single poison cannot distinguish an unwritten byte equal to that poison. The record change strengthens detection. |
| Uploads are proved by reading them back | `textures/upload-visibility` | Parallels' `r8-64-pitch64-transfer` both samples zeros and leaves poisoned readback bytes. |
| Foreign handles are refused by the validator | section 1.4.1; `isolation/foreign-target-draw` | Parallels' foreign-context probe receives `OK_NODATA` without the intended drawing. |
| Context creation queues asynchronous rendering-catalogue warm-up (ruling 33, #1725); `probe_int` warms on its first PROBE | sections 1.4, 1.4.1; `performance/first-draw-latency` | Parallels' `sceneA` first submission takes 28,365 µs, and `sceneA-again` takes 349 µs. These are the spike's programs, not the whole catalogue. Creation has its own deadline and kill requirement; the first PROBE's cost is excluded from performance bounds. |
| Timed work ends with a dependent observation | section 1.10 | A command response alone cannot establish completed pixel work. Opaque overdraw may be culled; blended or discarding workloads must measure the required work. |
| VirGL requires `VIRGL` and `VERSION_1`, deduplicating capability set IDs | section 1.6 | Parallels' capability enumeration lacks blob, context-init and EDID and repeats its VirGL 2 set at multiple indices. |
| VMware admits by capability entries | section 1.6.3 | The 3D-off control refuses at `DXCONTEXT=0`; the register's DX bit alone is insufficient. |
| VMware uses queue 0, page-table-backed tables, version 1 surfaces and bare SM4 tokens with signatures | section 1.6.3 | The cited rendering commit submits and reads these primitives back; present command completion is not scanout verification. |
| Attestation remains mandatory for implemented GPU layers | section 1.6.5 | The Parallels spike includes idle/load phases, but the cited serial alone does not establish host GPU-time deltas. The rate-ratio threshold of section 1.6.5 (amended by ruling 51) is a required acceptance criterion, not a measurement from this spike. VMware's attestation remains configuration-based under R6. |

The suite has 170 cases rather than the outline's 168. `textures/unaligned-pitch` requires exact
round trips at widths that exercise the backend layouts. `performance/first-draw-latency` adds a
10 ms first-draw bound after warm-up. The titles for poison detection, upload visibility and
strides state the checks now required; main's ID-only outline cannot establish title equivalence.
The offset-164 meaning and warm-up measurement condition are explicit contract changes, not
wording-only edits. The Parallels pitch behaviour still needs a black-box Linux probe; its 3D-off
control belongs to catch-up. Neither pending experiment changes the frozen pixel requirements.

### 1.13 What G1's review changed

G1's review (#1677, #1678) found places where the code and the contract disagreed. The coordinator ruled on
2026-10-16 that the contract's semantics win and that the contract is amended first, in its own PR, with G1
conforming to it. The amendment:

| Rule | Where |
|---|---|
| The VirGL interface headers (`virgl_protocol.h`, `virgl_hw.h`, `p_defines.h`, `p_shader_tokens.h`) are sources for values, enumerations and layouts only; virtio-gpu cites the VirtIO specification, except the four request layouts the specification omits, which come from the `virtio_gpu.h` interface header and are confirmed black-box; every value is recorded in `docs/gui/provenance/gpu-core.md` | section 1.9 |
| Programs are written from the TGSI documentation; the accepted text form is learned black-box and each grammar fact is recorded with its experiment | section 1.9 |
| Drawing into an invalid target is allowed; only sampling one is refused | section 1.4.1 |
| Every test-control point, 23 included, is one-shot | section 1.8 |
| Repeated device errors quarantine one context; the device is reset only for a device fault or the watchdog | sections 1.4.1, 1.8 |
| On ARM64 QEMU the existing display stays on the MMIO virtio-gpu until G7; the core is clean-room and never uses `gpu_pci.rs`, `virgl.rs` or `svga3.rs` | sections 1.2, 1.6, 1.11, 2.5, 2.6 |
| The wait, quarantine and reset tolerances, derived from the watchdog period, the kill bound and the reset budget; ring backpressure proved by fences | sections 1.4.1, 1.8 |
| Per-submission copy quotas are G6's; buffer snapshots and the watchdog over synchronous device waits are G1's | sections 1.4.1, 1.11 |

`docs/gui/gpu-abi.md`, the layouts and codes of syscall 416, follows the same rules.

### 1.14 What G2's review changed

G2's review (#1687, #1688, #1694) and G1's black-box experiments (#1684) found device behaviour and readback
semantics the contract did not state. The coordinator ruled on 2026-10-17 (rulings 13 to 15 and 17 to 20)
that they enter the contract in contract PRs, before G2's code conforms to it, and never together with
code, checks or thresholds. The amendment:

| Rule | Where |
|---|---|
| A `SUBMIT_3D` naming an unattached resource, or a transfer box past the resource, is answered OK and silently disables its device context; the kernel never sends either | section 1.4.1 |
| A fault is detected from what can be observed, never from the fault variable; fault-wrong-resource is detected as unwritten backing, reason 1, and `readback/fault-wrong-resource` requires reason 1 | section 1.8; `docs/gui/gpu-abi.md` |
| Readback names every EIO it returns, including a lost or dropped target, a transfer ended by a device reset (completed or failed), a transfer the watchdog ended and an unknown scanout binding | section 1.8; `docs/gui/gpu-abi.md` |
| EFAULT is atomic only as far as the kernel can guarantee it; what an EFAULT while writing results leaves behind is stated per op | section 1.4; `docs/gui/gpu-abi.md` |
| `PRESENT` and record offset 184 carry a context fence, never a raw device-wide fence number | sections 1.4, 1.7, 1.8; `docs/gui/gpu-abi.md` |
| `PRESENT` of a never-written target is refused with EINVAL | sections 1.4, 1.7; `docs/gui/gpu-abi.md` |
| `READBACK` may fail with ENOMEM when staging cannot be charged to the process's guest-backing quota or `RLIMIT_AS`; `COUNTERS` reports the staging under `resource_bytes` | sections 1.4, 1.8; `docs/gui/gpu-abi.md` |
| `RESOURCE_CREATE` declares scanout intent with a flag; only targets created with it get the device's `SCANOUT` bind, and `PRESENT` refuses other targets with EINVAL | sections 1.4, 1.7; `docs/gui/gpu-abi.md` |
| Reproduce commands use exactly the cited runs' flags, run folders keep `host-renderer.json`, and `scripts/vigil-record.sh` records whether the boot had a display window | section 1.9; `docs/gui/gpu-abi.md` |

### 1.15 What G3 needs

G3 (`textures`) is the first lane to upload, copy and sample. Sections 1.3, 1.4 and 1.8 named those
operations and left their interface to `docs/gui/gpu-abi.md`, which did not define it yet. This amendment
defines it, in its own PR before the lane's code (ruling 9):

| Rule | Where |
|---|---|
| `UPLOAD` names a live context, a texture or buffer, a clipped rectangle, the caller's stride, one of five source layouts (two 4-byte, two 3-byte with alpha 255, and R8) and a purpose; `COUNTERS` counts the source bytes read per purpose | sections 1.3, 1.4; `docs/gui/gpu-abi.md` |
| Uploads are ordered behind their own context's earlier commands rather than blocking on them, and wait for other contexts' submissions naming the resource; a failed upload leaves its texture invalid until a whole upload completes OK | section 1.4.1; `docs/gui/gpu-abi.md` |
| An interrupted upload wait returns EINTR; after sending, it leaves the texture invalid like EIO. Upload staging is charged like readback staging and may return ENOMEM. Device-access holds require fence publication as well as device completion or a successfully confirmed reset (ruling 24) | sections 1.4, 1.4.1; `docs/gui/gpu-abi.md` |
| `READBACK` reads textures as well as targets; an R8 texture reads back one byte per pixel (`canonical` 1); a texture's record describes its last completed upload | sections 1.4, 1.8; `docs/gui/gpu-abi.md` |
| The stream gains `COPY`, a region copy between resources of one format into a target, staged when the rectangles overlap in one resource, and `DRAW_SAMPLED`, whose instances name a destination rectangle and a source origin; refusal codes 15 to 17 | section 1.3; `docs/gui/gpu-abi.md` |
| Sampling converts by channel, with an R8 texel `r` read as (`r`, 0, 0, 255), and clips each rectangle to the texels the texture has | section 1.3; `docs/gui/gpu-abi.md` |
| Points 29 (`wrong-texture`) and 30 (`skip-upload-visibility`) take a texture handle and are defined in full | section 1.8; `docs/gui/gpu-abi.md` |

### 1.16 What G6 needs

G6 (`isolation`) measures what one process can do to another and to the kernel through syscall 416.
Sections 1.4, 1.4.1, 1.10 and 1.11 name its cases and leave how several of them are observed to the
implementation. This amendment states those interfaces, in its own PR before the lane's code (ruling 9):

<!-- claim-lint:ok: Normative G6 interface requirement stated by this amendment, docs/gui/gpu.md section 1.16, not a measured result. -->
- **Reclamation at exit, exec and kill.** When the last thread of a process exits, normally or by a signal,
  the GPU core drops every handle the process holds, as `CONTEXT_DESTROY` and `RESOURCE_DESTROY` of each
  would. A successful exec does the same before the new image runs. Dropping a
  handle does not cancel work: a context's device context is destroyed once its submitted work has completed,
  and a resource is freed when its hold set is empty (section 1.4.1, kills and hold sets). The new image of an
  exec holds no GPU handles: every handle from before the exec is EINVAL, and `COUNTERS` for the process
  reports no contexts, no resources and no resource bytes until the new image creates some.
<!-- claim-lint:ok: Normative G6 interface requirement stated by this amendment, docs/gui/gpu.md section 1.16, not a measured result. -->
- **Global GPU counters.** `/proc/trace/counters` reports three gauges over every process and the kernel:
  `gpu_contexts`, the contexts the core holds, including destroyed handles whose work is outstanding;
  `gpu_resources`, the resources it holds, including the kernel's own per-context and per-submission
  resources and dropped resources still held; and `gpu_resource_bytes`, the bytes charged to processes' GPU
  memory quotas (below) that have not been released. After a process with no work outstanding exits and its
  parent has reaped it, the three return to their values from before its first GPU call within 250 ms (K,
  section 1.4.1). After a process with work outstanding is killed, they stay above those values until that
  work's fences complete, by the device or by the watchdog, and return to them within 250 ms after.
<!-- claim-lint:ok: Normative G6 interface requirement stated by this amendment, docs/gui/gpu.md section 1.16, not a measured result. -->
- **Per-submission copy quotas.** `SUBMIT`'s private copies (the copy of the stream, the snapshot of the
  buffer ranges it names and the translated device command chunks) and the device vertex storage it creates
  are charged to the process's 192 MiB GPU memory quota and to `RLIMIT_AS` while they are held, like `UPLOAD`
  and `READBACK` staging. `COUNTERS` reports them in their own field, `copy_bytes` at offset 136, so
  `resource_bytes` keeps its meaning (guest backing and staging), and `gpu_resource_bytes` includes both. The
  quota limits the sum of `resource_bytes` and `copy_bytes`. The stream copy
  and the snapshot are released when `SUBMIT` returns; the chunks and the vertex storage when the device has
  returned the submission's requests or a confirmed reset has ended them. A charge that cannot be made returns
  ENOMEM, with error code 22 at index 0, before anything is sent.
<!-- claim-lint:ok: Normative G6 interface requirement stated by this amendment, docs/gui/gpu.md section 1.16, not a measured result. -->
- **No addresses.** A handle is a 64-bit value whose top byte is neither 0x00 nor 0xFF, so no handle has the
  form of a user, kernel or physical address on either architecture. Each value an op writes back is a handle,
  a context fence (`ns << 32 | n`), a device id below 2³², a count, a size, a serial, a generation, a status,
  an answer or a reason, a capability value, a string, or bytes of the caller's own pixels.
  G7 also exposes the current scanout's pixels to its display owner and the FNV memory fingerprints
  specified in `gpu-abi.md`, G7 display interface.
<!-- claim-lint:ok: Normative G6 interface requirement stated by this amendment, docs/gui/gpu.md section 1.16, not a measured result. -->
- **`SET_MODE` before G7.** Until G7 builds it, `SET_MODE` refuses a caller that is not the display owner
  with EPERM before its argument is decoded and returns ENOSYS to the owner; `MODES` returns ENOSYS.
<!-- claim-lint:ok: Normative G6 interface requirement stated by this amendment, docs/gui/gpu.md section 1.16, not a measured result. -->
- **Bounded validation.** Validation does work proportional to the stream's length, with the buffer
  ranges it reads bounded by the snapshot limits of section 1.4.1. The `coverage_spans` crossing tables one
  submission names in a buffer, each distinct table counted once, total at most that buffer's `data_bytes`
  (EINVAL, code 8, otherwise), so overlapping tables cannot make validation read the snapshot again for
  each instance. `isolation/bounded-validation` builds
  streams of 1 MiB and 16 MiB from the same mix of commands, each refused at its last command, and requires
  the 16 MiB stream's time per word to be at most twice the 1 MiB stream's (each the least of three runs),
  and a stream of the 80 MiB maximum to be refused within 1 s.
<!-- claim-lint:ok: Normative G6 interface requirement stated by this amendment, docs/gui/gpu.md section 1.16, not a measured result. -->
- **Quiet errors.** Kernel output is accounted per thread as BreenGL's contract, section 22, defines:
  `/proc/<pid>/task/<tid>/kernel_output` is the decimal count of bytes the kernel committed for output on
  that thread's behalf. `isolation/quiet-errors` makes 1,000 failing GPU calls, covering each op's refusals,
  and requires the calling thread's count to stay unchanged; test-control point 3, `emit-diagnostic`, shows
  first that a GPU call's output does reach the count.
<!-- claim-lint:ok: Normative G6 interface requirement stated by this amendment, docs/gui/gpu.md section 1.16, not a measured result. -->
- **The IRQ gauge over the GPU paths.** The trace counter `gpu_irq_masked_max_us` is the longest interval,
  in microseconds, that a CPU held IRQs masked while it ran a GPU path, since boot or the last `DISARM_ALL`,
  which restarts it at 0. A GPU path is a thread inside syscall 416, the GPU worker `kgpu`, the exit and exec
  reclamation above, or the GPU's interrupt handler. Every interval opened by the kernel's IRQ-masking
  primitives (`without_interrupts` and the IRQ-safe locks) on a thread in a GPU path counts, from the mask to
  the restore, as does the GPU interrupt handler from its entry to its return. The context switch itself is
  the scheduler's and is not a GPU path. Test-control point 2, `irq-hold` (BreenGL's contract, section 21),
  makes the next `SUBMIT` hold IRQs masked for 2 ms during its validation, through the same primitive, so
  `isolation/irq-masked-bound` first shows the gauge reads that hold.

| Rule | Where |
|---|---|
| Exit, kill and exec drop the process's GPU handles, without cancelling its work; the exec'd image starts with no handles | section 1.4.1; `docs/gui/gpu-abi.md` |
| `gpu_contexts`, `gpu_resources` and `gpu_resource_bytes` in `/proc/trace/counters`, with the 250 ms return after exit and after killed work drains | section 1.4.1; `docs/gui/gpu-abi.md` |
| `SUBMIT`'s copies and vertex storage are charged to the GPU memory quota and `RLIMIT_AS`, reported as `COUNTERS.copy_bytes`; ENOMEM, code 22 | sections 1.4, 1.4.1; `docs/gui/gpu-abi.md` |
| Handles have no address's form; returned values are of stated kinds | section 1.4; `docs/gui/gpu-abi.md` |
| `SET_MODE` refuses non-owners with EPERM before G7 | section 1.4; `docs/gui/gpu-abi.md` |
| The bounded-validation measurement; one submission's distinct coverage tables in a buffer total at most its `data_bytes` (code 8) | section 1.10; `docs/gui/gpu-abi.md` |
| Per-thread kernel output, and point 3 `emit-diagnostic` for the GPU core | sections 1.4, 1.8; `docs/gui/gpu-abi.md` |
| `gpu_irq_masked_max_us`, the GPU paths it covers, and point 2 `irq-hold` | sections 1.8, 1.10; `docs/gui/gpu-abi.md` |

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
2. **`libs/libgfx-ref`**: the full move of libgfx's CPU primitives, blits and alpha, and libfont's coverage
   rasterizer belongs to the BreenGL rebuild, not G5 (ruling 32, #1722b). G5 keeps only the frozen coverage
   reference from `breengel-cpu-393`, used solely by suite expectation modules, with no programs-wide
   dependency. Under ruling 30 (#1719), the four byte-for-byte reference copies stay byte-identical to
   their source tag. A frozen-copy manifest lists each path, source tag and SHA-256. Prose lint and
   whitespace checks skip exactly those listed files; a check fails if a listed file's bytes differ
   from its recorded SHA-256. No other file is exempted.

   During the BreenGL rebuild the full CPU reference moves without change. The move is checked as a pure
   rename (no content change in the diff), so the evidence of 393/393 on ARM64 QEMU, Parallels and VMware at #1655's merge (`4e855bb6`) applies to exactly this code. It
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
| syscall 415 test controls, the PID-1 latch, kernel output accounting | | the DISPI 1280×800 mode set and GOP compositor work in #1654 |
| `libbui` widget logic, `FocusChain`, the file picker, `libicon`, the UI clock, `FontWatcher` | widget and stage drawing → BreenGL on BGL; `libgfx::font` and `bitmap_font` → `bitmap_text` | |
| `libpixcheck`'s comparison core, `suite::spawn`, `kiosk_test`, `raw-window_test`, the suite harness and its split | `suite::judged(&FrameBuf)` → `judged_readback`; the run stage (`libgfx/stage.rs`, `suite_stage`) → BreenGL at display scale | |
| the showcase contract (§23.2–§23.8), Vigil's showcase pacing and evidence (#1658) | | |
| general kernel fixes found on the branch (for example `a9fd021d`, fw_cfg read serialization) | | `svga3.rs`, `gpu_pci.rs` and `virgl.rs` as a base for the GPU backends: the gpu core is clean-room (section 1.2), and those drivers serve only the existing display path |

### 2.6 The graphical programs

Fourteen installed programs use `FrameBuf`, `fb_mmap` or `libgfx` today, on main and on `breengel/suite`
(found by search on 2026-10-16). Between `gpu`'s landing and BreenGL's they keep running on the existing
display path, which on ARM64 QEMU stays on the MMIO `virtio-gpu-device` until G7. In the BreenGL rebuild each is either ported to
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

1. Test controls: syscall 415, the PID-1 latch, `init`'s absence check, on ARM64, dispatched through
   `dispatch_numbered` (no Tier-1 file changes); x86-64's dispatch arrives when x86-64 joins (section 1.6.4).
2. Kernel output accounting (§22) and its structure test.
3. `libs/libpixcheck`'s comparison core over byte slices, and `libbreenix::suite::judged_readback`. Not
   `suite::judged(&FrameBuf)`, `Suite::gui()` or `suite_stage`, which stay on `breengel/suite` until BreenGL
   converts them.
4. The VMware driver corrections (`kernel_va_to_ipa` addresses for command buffers and DMA pages, SVGA3D
   commands on context 0 after `START_STOP_CONTEXT`, `SET_OTABLE_BASE64`, the version 1
   `DEFINE_GB_SURFACE`, restart after `COMMAND_ERROR`) and the host-copy readback sequence.
5. The Parallels driver's fenced `TRANSFER_FROM_HOST_3D` readback and texture-slot allocation.

   Items 4 and 5 correct the existing display drivers; the gpu core does not use them (section 1.2).
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
| **The macOS host stack** | It is a second QEMU (10.0.12-utm with UTM's patches, ANGLE and virglrenderer forks) that Breenix builds and keeps pinned, and it moves every ARM64 gate from QEMU 11.0.3 at `gpu`'s landing. | G0 built it as a self-contained, pinned command-line bundle with both frontends and the attestation (#1666); rendering needs no SPICE client. Every ARM64 suite must match main on it before the gate switch (R5). |
| **x86-64 has no GPU path yet** | The current x86 gate host has no rendering GPU, the gate's virtual GPU negotiates no VirGL, and its QEMU's GL display module failed to load. | x86-64 is out of the GPU-era efforts' targets until the dedicated Breenix hardware box replaces that host (R1); its spike's acceptance mirrors G0's then. |
| **The hardware claim** | No protocol lets the guest tell a GPU from software. | Host attestation from the executing context on ARM64 QEMU, refused when software or missing, with a planted software run; host GPU time per VM process on Parallels; on VMware the configuration check with its 3D-off control, labelled configuration-based (R6). |
| **VMware DX complexity** | The shader token programs and signatures are hand-written, and errors arrive only as command-buffer status; the G0 spike ran 13 programs, not the whole catalogue. | The spike exercised the device primitives listed in section 1.12 and fixed the wire path; the backend adds one program at a time, each with a readback case, with a Linux VM in Fusion as a black-box behaviour probe. |
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
3. G0 in section 1.11's order: the provenance audit, the macOS host stack, the Parallels and VMware
   spikes, this contract and the `gpu` suite with 170 failing cases are done; the harvest PRs follow their
   files' audit records.
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
