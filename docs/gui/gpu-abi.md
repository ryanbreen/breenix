# Syscall 416: argument layouts, the BGL stream and its codes

`docs/gui/gpu.md` section 1.4 fixes syscall 416's ops, their errno rules and what each op does. This
document fixes what that section leaves to the implementation: each op's argument layout, the BGL
command stream `SUBMIT` carries, and the codes the kernel reports. It specifies the layouts alongside
the contract; the asynchronous catalogue warm-up amendment (ruling 33, #1725) is stated in both documents.
The single definition in code is the `gpu-abi` crate (`libs/gpu-abi`, from G1), which the kernel and
libbreenix share; the suite checks the layouts at the offsets given here.

Every structure is little-endian with the offsets shown. `len` must equal the structure's size;
reserved fields and output fields are zero on the way in (EINVAL otherwise) and the kernel writes
reserved fields zero. `VERSION` takes no argument (`len` 0).

**EFAULT, as far as the kernel can guarantee it.** A fault reading the argument, or anything else the op
reads from the caller, returns EFAULT before the op has done anything. Results are written back after the
op acts, and the kernel cannot hold the caller's pages against the caller's own threads, so an EFAULT while
writing them reports that the results were not delivered, not that nothing happened: `CONTEXT_CREATE` and
`RESOURCE_CREATE` destroy the handle they made; a `SUBMIT` whose results cannot be written back has been sent
and completes like any other, and its fence is lost to the caller; `READBACK` is below.

## Admission

The device is admitted when it has the capabilities required by the profile (`docs/gui/gpu.md` section 1.5,
item 8). Until it is, ops other than `VERSION` and `CAPS` return ENODEV before their argument is
decoded, and `CAPS` reports why. Ops the effort has not built yet (`MODES` and `SET_MODE`, until
G7) return ENOSYS, except that `SET_MODE` refuses a caller that is not the display owner with EPERM
before its argument is decoded. An op number above 13 is EINVAL.

Capability ids, in the order admission checks them (the first missing one is reported):

| Id | Name | VirGL evidence |
|---|---|---|
| 1 | `3d-feature` | `VIRTIO_GPU_F_VIRGL` negotiated |
| 2 | `capability-set` | a VIRGL or VIRGL2 capability set of at least `virgl_caps_v1`'s 308 bytes |
| 3, 4 | `b8g8r8a8-render`, `b8g8r8a8-sample` | the format's bit in the set's render and sampler masks |
| 5, 6 | `r8g8b8a8-render`, `r8g8b8a8-sample` | as above |
| 7 | `r8-sample` | the sampler mask |
| 8, 9 | `integer-ops`, `texel-fetch` | the set's shading-language level at least 130 (GLSL 1.30). The level is what the device offers its guest; a GLES-backed host reports it on the same scale |
| 10 | `region-copy` | a VirGL capability set: `RESOURCE_COPY_REGION` is a base command of the protocol the set describes, gated by no capability bit |
| 11 | `target-size` | a maximum 2D texture size of at least 2560 |

Test-control point 23 (`mask-capability`) is one-shot: the next admission decision, made by a `CAPS` query
or by any other op that admission gates, counts the capability its parameter names as absent, so that op
fails naming it (ENODEV, or `CAPS` status 2 with `missing` set), the point counts one firing and disarms.
The op after it sees the device's real capabilities without `DISARM_ALL`. A check that needs several ops
refused arms the point before each.

## Op arguments

**`CAPS` (512 bytes).**

| Offset | Field |
|---|---|
| 0 | `version: u32` (1), `size: u32` (512) |
| 8 | `status: u32`: 0 admitted, 1 no device, 2 missing capability, 3 device failed; `missing: u32` at 12, the capability id |
| 16 | `backend: u32` (4 VirGL, 5 SVGA3D DX); `features: u32` at 20: bit 0 VirGL negotiated, bit 1 VERSION_1 negotiated, bit 8 SVGA3D DX context |
| 24 | `capset_id: u32`, `capset_version: u32`: the capability set the profile was read from |
| 32 | `capset_count: u32`; `capset_ids: [u32; 8]` at 36: the distinct ids the device lists, ascending, zero after the count |
| 68 | `formats: [(format: u32, flags: u32); 3]`: B8G8R8A8 (0), R8G8B8A8 (1), R8 (2); flags bit 0 sampleable, bit 1 renderable |
| 92 | `profile: u32`: bit 0 integer operations, bit 1 integer texel fetch, bit 2 region copies; `shading_level: u32` at 96 |
| 100 | `max_width: u32`, `max_height: u32`: the device's 2D maximum, capped at 16,384 |
| 108 | `catalogue_version: u32` (1); `pipelines: u32` at 112: bit `n` set when catalogue pipeline `n` is built |
| 116 | `scanout_count: u32`; reserved to 128 |
| 128 | `renderer: [u8; 64]`: the capability set's renderer string, NUL-padded |
| 192 | `reason: [u8; 64]`: empty when admitted, else `missing <name>` or the failure, NUL-padded |
| 256 | `scanouts: [(enabled: u32, width: u32, height: u32, reserved: u32); 16]`, zero after the count |

Pipelines are numbered in the catalogue table's order (`docs/gui/gpu.md` section 1.3): `solid` 0,
`copy` 1, `copy_scaled` 2, `nearest_scaled` 3, `box_filter` 4, `over` 5, `over_straight` 6, `coverage` 7,
`coverage_spans` 8, `bitmap_text` 9, `line` 10, `circle_fill` 11, `circle_outline` 12, `rounded_rect` 13,
`gradient_v` 14, `triangle` 15, `probe_int` 16. Formats: 0 B8G8R8A8, 1 R8G8B8A8, 2 R8 (record v2's
`format` uses the same numbers).

**`CONTEXT_CREATE` (32 bytes):** `handle: u64` out, `flags: u32` (0), `warmed: u32` out (the
number of fixed rendering-catalogue programs warmed at the time of the result, excluding `probe_int`),
16 reserved. EAGAIN past 8 contexts per process. Creation queues catalogue compilation and warm-up on the
GPU worker and returns without waiting for it; `warmed` is a progress snapshot, not the completion signal.
Query `COUNTERS` with this context's handle to observe warm-up completion (state 0); state 3 means the
context is live and still warming. The profile-only `probe_int` program is compiled and warmed on the
first PROBE in that context; that first PROBE's cost is excluded from performance bounds. The watchdog
and kill bounds still apply (`docs/gui/gpu.md` section 1.4.1).

<!-- claim-lint:ok: Normative G6 interface requirement from docs/gui/gpu.md section 1.16, not a measured result. -->
**Handles.** `CONTEXT_CREATE` and `RESOURCE_CREATE` return handles private to the process: a value
another process holds names nothing here. A handle's top byte is neither 0x00 nor 0xFF, so no handle has
the form of a user, kernel or physical address. No op writes back a kernel or device address: each output
is a handle, a context fence, a device id below 2³² (record v2's `context` and `resource`), a count, a
size, a serial, a generation, a status, an answer or a reason, a capability value, a string, or bytes of
the caller's own pixels.

<!-- claim-lint:ok: Normative G6 interface requirement from docs/gui/gpu.md section 1.16, not a measured result. -->
**Reclamation.** The exit of a process's last thread, normal or by a signal, drops every handle the
process holds, as `CONTEXT_DESTROY` and `RESOURCE_DESTROY` of each would; a successful exec does the same
before the new image runs. The global counters below return to their earlier values within 250 ms of the
parent reaping a process that had no work outstanding (`docs/gui/gpu.md` section 1.16). Dropping a handle cancels nothing: the device context goes when its work
has completed, and a resource when its hold set is empty (`docs/gui/gpu.md` section 1.4.1).

**`CONTEXT_DESTROY`, `RESOURCE_DESTROY` (16 bytes):** `handle: u64`, 8 reserved. EINVAL for a handle the
process does not hold.

**`RESOURCE_CREATE` (48 bytes):** `kind: u32` (1 target, 2 texture, 3 buffer), `format: u32`, `width:
u32`, `height: u32`, `size: u64` (bytes, buffers only), `handle: u64` out, `flags: u32` at 32,
12 reserved. `flags` bit 0 is scanout; all other bits must be zero (EINVAL otherwise). The scanout flag
is valid only for targets (EINVAL for textures or buffers). Scanout targets are declared at creation:
only targets created with this flag get the device's `SCANOUT` bind, and `PRESENT` refuses a target
created without it with EINVAL. Targets are B8G8R8A8 or R8G8B8A8; textures add R8; sides are 1 to
`max_width`. Buffers are 1 byte to 128 MiB.
ENOMEM past 192 MiB of guest backing per process; EAGAIN past 256 resource handles. A new resource is
attached to every context of the process, and a new context to every resource.

**`UPLOAD` (64 bytes):** `context: u64`, `resource: u64`, `x, y, width, height: u32` at 16, `pixels: u64`
(address) at 32, `pixels_len: u64` at 40, `stride: u32` at 48, `source: u32` at 52, `purpose: u32` at 56,
4 reserved. Copies a rectangle of the caller's pixels into a texture, or a range of bytes into a buffer,
and makes it visible to the submissions sent after the call returns.

- `context` is one of the caller's contexts (EINVAL otherwise) and must be live (EIO otherwise); the
  upload's transfer is sent in it and its bytes are counted to it and to the process. `resource` is a
  texture or buffer the caller holds; a target, or any other handle, is EINVAL.
- `source` is the layout of the caller's pixels: 0 B8G8R8A8 (bytes B, G, R, A), 1 R8G8B8A8 (R, G, B, A),
  2 R8 (one byte), 3 B8G8R8 (B, G, R) and 4 R8G8B8 (R, G, B). A B8G8R8A8 or R8G8B8A8 texture takes
  sources 0, 1, 3 and 4, converted channel by channel into the texture's format, with alpha 255 for a
  3-byte source; an R8 texture and a buffer take source 2 only. Any other pairing is EINVAL.
- For a buffer, `x` is the first byte, `width` the number of bytes, `y` 0 and `height` 1.
- `width` and `height` are at least 1. The rectangle `[x, x + width) × [y, y + height)` is clipped to the
  resource: pixel `(i, j)` of the source, at `pixels + j × stride + i × bpp` (`bpp` the source's bytes per
  pixel), lands at `(x + i, y + j)`, and pixels that land outside the resource are neither read nor
  written, so no row is written past its end. A rectangle whose clipped part is empty is EINVAL.
  With the clipped size `cw × ch`, `stride` must be at least `cw × bpp` and `pixels_len` at least
  `(ch − 1) × stride + cw × bpp` (EINVAL otherwise).
- `purpose` is 1 `Image` or 2 `Fixture` (`docs/gui/gpu.md` section 2.4; EINVAL otherwise). `COUNTERS`
  adds the source bytes read, `cw × ch × bpp`, to the purpose's upload bytes.
- The source rows are read before anything is sent: an unmapped or unreadable source returns EFAULT before
  anything is sent, written or counted.
- Temporary upload staging uses guest-backing frames, charged like `READBACK` staging to the process's
  192 MiB guest-backing quota and `RLIMIT_AS`. `UPLOAD` returns ENOMEM if staging cannot be allocated or
  charged, before device work is sent or the resource is changed. `COUNTERS.resource_bytes` includes
  this staging while held; the charge is released when the staging is freed.
- An upload waits for a readback or upload of the same resource in progress, and for the submissions of the
  caller's other contexts that name the resource to complete. It does not wait for submissions of its own
  context: the device executes one context's commands in the order they were sent (which
  `textures/immutable-while-pinned` checks), so a submission sent before the upload samples the texture as it
  was before, and one sent after it returns sees it.
- On VirGL a texture upload writes the clipped rows into the texture's backing at its 256-byte-aligned
  pitch (`docs/gui/gpu.md` section 1.3) and sends one fenced `TRANSFER_TO_HOST_3D` of the clipped box in
  `context`, a synchronous device wait under the watchdog. A buffer upload is a copy into the buffer's
  backing, which `SUBMIT` snapshots (`docs/gui/gpu.md` section 1.4.1).
- A completed texture upload advances the texture's generation. The device answering the transfer with an
  error, the watchdog ending it or a reset decided while it was outstanding returns EIO and leaves the
  texture invalid, as a failed submission leaves its targets: sampling it is refused and reading it back
  returns EIO target-invalid, naming the upload, until an upload of the whole texture completes OK.
- A signal interrupting an upload wait returns EINTR, and a kill ends the caller within 250 ms. If the
  upload request has been sent, EINTR leaves its texture invalid with the same sampling refusal,
  readback error and whole-upload recovery as EIO above. Interruption before the request is sent leaves
  the resource unchanged. Returning EINTR does not cancel device work or release its holds: device-access
  holds last until the fence is published and actual device completion or a successfully confirmed reset
  has ended device access (`docs/gui/gpu.md` section 1.4.1).

**`SUBMIT` (64 bytes):** `context: u64`, `stream: u64` (address), `length: u64` (bytes, a multiple of
4, 1 to 80 MiB), `fence: u64` out, `error_index: u32` out, `error: u32` out, `estimate: u64` out, 16
reserved. Each buffer range the stream names is copied once, before validation, and validation and
translation read only that copy (`docs/gui/gpu.md` section 1.4.1). A refused stream returns EINVAL, or
EDQUOT for the work cap, with the failing command's index and its code written back.
Validation scratch allocation is fallible: exhaustion returns ENOMEM with error code 22. The submission's
private copies (the stream copy, the buffer snapshot and the translated device chunks) and the device
vertex storage it creates are charged to the process's 192 MiB GPU memory quota and `RLIMIT_AS` while they
are held; `COUNTERS.resource_bytes` includes them. The stream copy and the snapshot are released when the
call returns, the chunks and the vertex storage when the device has returned the submission's requests or a
confirmed reset has ended them. A charge that cannot be made returns ENOMEM with error code 22 at index 0
before anything is sent. A context that is not
live (quarantined or reset) returns EIO. With four submissions in flight the call waits for one to
complete; a signal ends that wait with EINTR, and a kill within 250 ms. A warming context accepts
submissions: a draw waits only for its own pipeline if that pipeline is still warming, without a
catalogue-wide barrier or a wait for unrelated pipelines. The same watchdog and kill bounds apply to
this wait. Work-estimate weights, case deadlines and `WAIT`/fence semantics are unchanged (ruling 33,
#1725).

**`WAIT` (48 bytes):** `context: u64`, `fence: u64`, `timeout_ns: u64` (0 polls), `status: u32` out,
`answer: u32` out, 16 reserved. Returns 0 when the fence completed OK; EIO with `status` and `answer`
written back when it completed otherwise; ETIMEDOUT at the timeout; EINTR on a signal. A fence names its
context: `fence` is `ns << 32 | n` for the context's `n`th fenced operation, a `SUBMIT` or a `PRESENT`
(below), and a fence of another context, or one the context never issued, is EINVAL. A fence is
complete only when every earlier fence of its context is.

| `status` | Meaning |
|---|---|
| 0 OK | the device completed the submission |
| 1 DEVICE_ERROR | the device answered with an error; `answer` is its response type. Two consecutive DEVICE_ERROR completions on one context quarantine that context only (`COUNTERS` state 1); the device is not reset |
| 2 DEVICE_HUNG | the watchdog found it outstanding 2 s after it was sent and quarantined its context |
| 3 DEVICE_RESET | the device was reset with it outstanding; resets happen only for a device fault or the watchdog's no-progress condition (`docs/gui/gpu.md` section 1.4.1) |
| 4 FENCE_MISMATCH | the device's answer carried another submission's fence |

These statuses describe the caller-visible result, not permission to release DMA backing. In particular,
DEVICE_HUNG and DEVICE_RESET do not themselves prove that device access has ended. Backing remains held
until actual device completion or a successfully confirmed reset supplies that proof; a failed reset
leaves it held, including after a timeout, kill or EIO return (section 1.4.1 of `docs/gui/gpu.md`).

**`READBACK` (64 bytes):** `target: u64`, `x, y, width, height: u32`, `pixels: u64` (address),
`pixels_len: u64`, `stride: u32`, `reason: u32` out at 44, `record: u64` (address of 1,024 bytes),
`failed_submission: u64` out at 56. Waits for the accepted submissions that write the target, then reads
the rectangle as canonical BGRA rows at `stride` with record v2 (`docs/gui/gpu.md` section 1.8).
`target` names a target or a texture. An R8 texture reads back one byte per pixel, so its rows are `width`
bytes (record `canonical` 1); the other formats read back as canonical BGRA, four bytes per pixel
(`canonical` 0). A texture's record describes its last completed upload in the fields that describe a
target's last submission: `context` is the upload's device context, `render_serial` its serial (numbered as
a submission's), `render_fence` the fence its transfer carried, `render_status` the device's answer and
`render_kind` `TRANSFER_TO_HOST_3D` (0x0205); they are 0 before its first upload.
Temporary whole-rectangle staging uses guest-backing frames, charged to the process's 192 MiB
guest-backing quota and `RLIMIT_AS`. `READBACK` may fail with ENOMEM when staging cannot be charged to
the process's guest-backing quota or `RLIMIT_AS`. `COUNTERS.resource_bytes` includes the staging while
it is held; the charge is released when the staging is freed. An EIO writes its reason to `reason`,
and only that reason:

| `reason` | Name | Meaning |
|---|---|---|
| 0 | none | the call did not return EIO |
| 1 | unwritten | the two complementary-poison observations differ (`unwritten` in the record is the count). This is how an omitted transfer (point 19), a transfer naming another resource (point 21) and a transfer in a context the device disabled (`docs/gui/gpu.md` section 1.4.1) show |
| 2 | fence-mismatch | a transfer's answer lacks the fence flag or echoes another fence, or a recorded fence belongs to another submission |
| 3 | resource-mismatch | an observation claiming the scanout (record `flags` bit 0 set) names a resource that differs from the device's confirmed scanout binding. An offscreen observation (bit 0 clear) does not require its resource to match the scanout. The transfer answer carries no resource identity, so a transfer the device took from another resource is never reported this way; it is reason 1 |
| 4 | target-invalid | the target is invalid (`docs/gui/gpu.md` section 1.4.1); `failed_submission` is the serial of the submission that invalidated it, numbered as the record's `render_serial` |
| 5 | device-error | the device answered a readback transfer with an error |
| 6 | device-hung | the watchdog ended a transfer the device had not answered 2 s after it was sent; its context is quarantined, as for a submission |
| 7 | device-reset | a device reset was decided while a transfer was outstanding, whether the reset then completed or failed; a failed reset leaves the target's backing held (`docs/gui/gpu.md` section 1.4.1) |
| 8 | target-lost | the target was lost to an earlier device reset; the process must create it again |
| 9 | target-dropped | the target's handle was destroyed (by another thread, or by the process's exit) while the readback waited for its writers |
| 10 | binding-unknown | the target may be the scanout, and a `PRESENT` that binds it or replaces it was sent and has not completed, or failed, so the kernel does not know what the scanout shows; a later completed `PRESENT` or a confirmed reset makes the binding known again |

`failed_submission` is zero for every other reason. Both output fields are zero on input.

Composite observations come after G2. A composite with more than 16 windows returns EIO with reason
`composite-overflow` and no partial record. Its distinct numeric code is assigned with that extension,
alongside the code for a composed window's render fence not having completed OK before the composite was
sent (`docs/gui/gpu.md` section 1.8); neither failure reuses reasons 1 to 10.

EFAULT is atomic only as far as the kernel can guarantee it. Before sending any device work, `READBACK`
checks that every page of its argument, of its record and of its pixel rows at `stride` is mapped and
writable by the caller; a failure there returns EFAULT with nothing sent, no observation serial used, and
nothing written or counted. The kernel cannot hold those pages against the caller's own threads: if one is
unmapped or made read-only after that check, the call returns EFAULT, the readback is not counted, and the
contents of the three output ranges are unspecified, except that every byte the kernel wrote to the pixels
belongs to an observation that passed both poisons.

**`PRESENT` (32 bytes):** `target: u64`, `fence: u64` out at 8, `answer: u32` out at 16, 4 reserved,
`context: u64` at 24. Display owner only (EPERM otherwise). `context` is one of the caller's contexts
(EINVAL otherwise) and must be live (EIO otherwise); the bind and flush are sent in it. Waits for every
accepted submission that writes the target. `PRESENT` of a never-written target (no completed write
after that wait) is refused with EINVAL, and a target created without `RESOURCE_CREATE`'s scanout flag
is refused with EINVAL. An invalid or lost target is refused with EIO. Otherwise it binds the whole
target to scanout 0 and flushes it, and returns once the flush is answered, with `answer` the flush's
response. `fence` is a context fence of `context` (`ns << 32 | n`, numbered in sequence with its
`SUBMIT`s, above), so `WAIT` on that context accepts it and reports the flush's outcome; it is never a raw
device-wide fence number. Record v2 offset 184 carries the same context fence when the observed target is
the scanout. The binding holds the target's resource until a later binding's flush completes or a
confirmed reset ends device access (`docs/gui/gpu.md` section 1.4.1). G2 builds this single-target
operation; G7 adds modes, flipping and the ownership stack.

**`COUNTERS` (256 bytes):** `context: u64` in (0 for the calling process), then `u64`s: submissions 8,
draws 16, instances 24, fragment area 32, work estimate 40, image upload bytes 48, fixture upload bytes 56
(the source bytes `UPLOAD` read, by purpose),
readbacks 64, ring waits 72 (submissions that waited because four were in flight), device errors 80,
state 88 (a context's: 0 live with catalogue warm-up complete, 1 quarantined, 2 reset,
3 live with catalogue warm-up in progress), contexts 96 (the process's live handles),
device contexts 104 (the process's device contexts not yet destroyed, including destroyed handles whose
work is outstanding), resources 112, resource bytes 120 (`resource_bytes`: guest backing held,
including temporary `UPLOAD` and `READBACK` staging charged to the process), warmed 128; reserved to 256.
For a context, `warmed` counts programs whose warm-up completed, including `probe_int` after its first
PROBE; it is not the completion signal for the rendering catalogue. A query with a nonzero context handle
reports that context's warm-up state without waiting for warm-up. State 0 is reported only once its
fixed rendering catalogue has warmed; state 3 permits the same operations as state 0, with the
per-pipeline draw wait described above. A warm-up failure follows the existing quarantine/reset rules
and does not report completion. `performance/first-draw-latency` measures a draw issued after the context
reports state 0; neither its 10 ms bound nor case deadlines or work-estimate weights change.
A clear counts as one draw of one instance covering the target.

## The BGL command stream

32-bit little-endian words. Each command starts with its code and its length in words, counting both;
its index is its position in the stream.

| Code | Command | Words |
|---|---|---|
| 1 | `BEGIN` | `1, 4, target_lo, target_hi`: later commands draw into this target |
| 2 | `CLEAR` | `2, 3, colour`: every pixel of the current target becomes `colour` |
| 3 | `DRAW` | `3, 5 + 4n, pipeline, colour, n`, then `n` rectangles `x0, y0, x1, y1` (signed 32-bit, half-open) |
| 4 | `DRAW_BUFFER` | `4, 8, pipeline, colour, buffer_lo, buffer_hi, offset, n`: `n` rectangles read from a buffer, 16 bytes each from `offset` |
| 5 | `COPY` | `5, 12, source_lo, source_hi, sx, sy, w, h, destination_lo, destination_hi, dx, dy`: a GPU region copy of the source rectangle `[sx, sx + w) × [sy, sy + h)` to the point `(dx, dy)` of the destination (unsigned) |
| 6 | `DRAW_SAMPLED` | `6, 6 + 6n, pipeline, texture_lo, texture_hi, n`, then `n` instances `x0, y0, x1, y1, sx, sy` (signed 32-bit): each half-open rectangle of the current target drawn from the texture, pixel `(x, y)` reading texel `(sx + x − x0, sy + y − y0)` |
| 7 | `PROBE` | `7, 24, routine, word, x0, y0, x1, y1, a0, …, a15`: draw one rectangle using `probe_int`; the sixteen input words are integer bit patterns |
| 8 | `SCISSOR` | `8, 6, x0, y0, x1, y1`: intersect later draws with this half-open rectangle, until another SCISSOR or BEGIN; BEGIN restores the whole target |

<!-- claim-lint:ok: Proposed interface requirements implementing docs/gui/gpu.md section 1.5. -->
G4 defines PROBE and SCISSOR before their implementation. PROBE needs a current target and
an ordered rectangle, like DRAW. It emits one instance; its area is the rectangle intersected with
the target and scissor. It writes the selected result word as canonical BGRA bytes, least significant
byte first, using integer byte extraction and the final `k / 255` conversion. No result is computed
on the CPU. Inputs not used by a routine must be zero; word selects one of the result words below.
An unknown routine, an out-of-range word selector, an operand outside its stated input domain, or a nonzero unused input returns EINVAL, code 18.
The PROBE work estimate is 65,536 steps per pixel, a conservative budget for the fixed arithmetic
program including its 96 division iterations. SCISSOR itself counts no draw, instance or work.
SCISSOR needs a current target, refuses inverted rectangles (code 6), and clips CLEAR as well as
DRAW, DRAW_BUFFER, DRAW_SAMPLED and PROBE. COPY remains independent of drawing state.

<!-- claim-lint:ok: Numerical requirements from docs/gui/gpu.md section 1.5. -->

| Routine | Inputs | Result words, least significant first |
|---|---|---|
| 0 add, 1 subtract, 2 multiply | a0, a1 | wrapping 32-bit result |
| 3 shift left, 4 shift right, 5 arithmetic shift right | a0, a1 (0–31) | shifted word; arithmetic shift interprets a0 as i32 |
| 6 signed less, 7 unsigned less | a0, a1 | 1 when less, otherwise 0 |
| 8 div255 | a0 (0–65,025) | floor((a0 + 127) / 255) |
| 9 multiply two words | a0, a1 | exact unsigned 64-bit product (2 words) |
| 10 multiply three words | a0, a1 (one u64), a2 (u32) | exact unsigned 96-bit product (3 words) |
| 11 divide three words | a0–a2 dividend, a3–a5 nonzero divisor | unsigned quotient (words 0–2), remainder (3–5), restoring division in exactly 96 iterations |
| 12 binary32 add, 13 subtract, 14 multiply | a0, a1 | IEEE-754 binary32 result bits, round to nearest, ties to even |
| 15 binary32 truncate | a0 | signed i32, toward 0; only finite inputs whose truncated value fits i32 |
| 16 binary32 ceil | a0 | binary32 bits of the least integral value not below the input |
| 17 binary32 less | a0, a1 | 1 when less, otherwise 0 |
| 18 fragment x, 19 fragment y | no inputs | floor of the fragment's window coordinate |
| 20 circle fill, 21 circle outline | a0,a1 pixel; a2,a3 centre; a4 radius | membership, 0 or 1, in §5.4 or §5.5's pixel set |
| 22 rounded rectangle | a0,a1 pixel; a2,a3 origin; a4,a5 width,height; a6 radius | membership in §5.6's pixel set |
| 23 triangle | a0,a1 pixel; a2–a7 three integer vertices | membership by edge functions at the pixel centre, with the top-left rule |
| 24 line | a0,a1 pixel; a2–a5 start and end | membership in §5.3's unclipped walk, both endpoints included |

<!-- claim-lint:ok: Proposed numerical requirements implementing docs/gui/gpu.md section 1.5, not measured results. -->
Binary32 arithmetic preserves gradual underflow and signed zero; a NaN result is canonical
0x7FC00000. The suite checks literal finite, subnormal, tie, cancellation and range-end vectors;
the host model is checked against native f32 before the corresponding shader is written
(`gpu.md` section 1.5). Shape inputs are signed i32; nonpositive sizes and negative circle radii
produce empty sets; a rounded rectangle with a nonpositive radius is a plain rectangle. Shape predicates use two or three words as section 1.5 requires,
including coordinate differences that exceed i32. This interface leaves the promised G5
closed-form proofs and the pixel sets unchanged. A triangle with zero signed area is empty;
for positive winding, an edge is inclusive when its dy is negative or its dy is zero and dx
positive (top-left in the top-left coordinate system); negative winding reverses the vertices.

Bit 31 of a drawing command's pipeline word requests a filtered sampler. Exact pipelines refuse
that modifier with EINVAL, code 19, before sending commands. Point 24 injects that modifier into
the next DRAW, DRAW_BUFFER or DRAW_SAMPLED during validation, consumes one firing and returns
code 19 with that draw's index. Arm point 24 with target 0 and parameter 0. Its event target is 0:
it does not filter by the resource named by BEGIN, and a nonzero armed target does not match the event.
The failed submission sends no commands and changes no GPU counters, generations or pixels; the
test-control firing count advances once. The refusal is from the same pipeline check as an explicit modifier.

`colour` is the canonical BGRA bytes as a little-endian word: B in bits 0 to 7, G 8 to 15, R 16 to 23,
A 24 to 31. Rectangles are clipped to the target; one with `x1 < x0` or `y1 < y0` is refused. Any drawing
command may draw into a target the device left invalid; the target becomes valid again only when a
submission that writes it whole (a `CLEAR`, or a `COPY` whose clipped rectangle is the whole target)
completes OK. `DRAW_BUFFER` reads its rectangles only from the submission's snapshot of the buffer,
rather than from the buffer after `SUBMIT`.

`COPY` needs no `BEGIN` and leaves the current target as it is. Its source is a target or texture and its
destination a target, both the process's (code 4 otherwise), of the same format (code 15 otherwise); the
bytes are copied as they are, with no conversion. The source rectangle is clipped to the source, and the
copied rectangle, anchored at `(dx, dy)`, to the destination; what is clipped away is not copied, and a
copy whose clipped rectangle is empty sends no device command. When source and destination are one resource and the
two rectangles overlap, the copy goes through a scratch texture, so the destination receives the source as
it was before the copy (`docs/gui/gpu.md` section 1.3). On VirGL each device copy is a
`RESOURCE_COPY_REGION`.

`DRAW_SAMPLED` draws with a pipeline that samples a texture: `copy` from G3 (`docs/gui/gpu.md` section
1.3). A sampling pipeline in `DRAW` or `DRAW_BUFFER`, or another pipeline in `DRAW_SAMPLED`, is code 16.
The texture is a texture or target of the process (code 4 otherwise) other than the current target (code
17). Each rectangle is clipped to the target and to the texture's footprint at the rectangle,
`[x0 − sx, x0 − sx + texture width) × [y0 − sy, y0 − sy + texture height)`, so each pixel drawn reads a
texel inside the texture. `copy` writes each texel converted channel by channel into the target's format:
B8G8R8A8 and R8G8B8A8 texels keep their four channels, and an R8 texel `r` reads as R `r`, G 0, B 0 and A
255 (section 1.3 of `docs/gui/gpu.md`, sampling).

Validation walks the whole stream before anything reaches the device. Its codes (`error`):

| Code | Refusal |
|---|---|
| 1 | a command's length runs past the stream or is shorter than two words |
| 2 | an unknown command |
| 3 | a drawing command other than `COPY` before any `BEGIN` |
| 4 | a handle the process does not own, or of the wrong kind, or destroyed |
| 5 | a pipeline outside the catalogue, or not built yet |
| 6 | an inverted rectangle |
| 7 | a length that disagrees with the command's layout or rectangle count |
| 8 | a buffer range outside the buffer or not 16-byte aligned |
| 9 | more than 65,536 draws |
| 10 | more than 1,048,576 instances |
| 11 | more than 64 MiB of distinct buffers named, or more than 64 buffers |
| 12 | more than 2³¹ estimated fragment-steps (EDQUOT) |
| 13 | a stream length of 0, not a multiple of 4, or over 80 MiB (index 0) |
| 14 | a command that samples a target or texture the device left invalid: a `COPY` source or a `DRAW_SAMPLED` texture |
| 15 | a `COPY` between resources of different formats |
| 16 | a pipeline used with the wrong drawing command: a sampling pipeline in `DRAW` or `DRAW_BUFFER`, or another pipeline in `DRAW_SAMPLED` |
| 17 | a `DRAW_SAMPLED` whose texture is the target it draws into |

The work estimate is, per draw, the clipped area times the pipeline's per-fragment steps (`solid` and
`copy`: 1). A `COPY` counts as one draw of one instance writing its clipped rectangle, at one step per pixel
for each device copy it makes: one, or two when it goes through a scratch texture.

## G5 parameterized draws

<!-- claim-lint:ok: Proposed requirements implementing docs/gui/gpu.md sections 1.3 and 1.5. -->
Command 9, `DRAW_PIPELINE`, is
`9, 10+20*n, pipeline, colour, texture_lo, texture_hi, buffer_lo, buffer_hi, data_bytes, n`,
then n instances of four signed rectangle corners followed by sixteen parameter words.
The rectangle bounds fragment execution; the shader evaluates the named pipeline at each
pixel. BEGIN and SCISSOR apply as for DRAW. Unused parameters and unused handles are zero.
SOLID, COPY and PROBE retain their existing commands; command 9 serves pipelines 2–15.
All sampled fetches are integer-addressed and outside-source pixels are discarded.

| Pipeline | Parameters, starting at p0 |
|---|---|
| copy_scaled | destination origin x,y; source origin x,y; positive integer scale |
| nearest_scaled, box_filter | destination origin x,y; destination width,height; source origin x,y; source width,height; box_filter requires source dimensions at least destination dimensions |
| over, over_straight | destination origin x,y; source origin x,y |
| coverage | destination origin x,y; source origin x,y; LCD flag 0 or 1 (LCD uses a four-channel mask, scalar uses R8) |
| coverage_spans | destination origin x,y; width,height; crossing-buffer byte offset |
| bitmap_text | destination origin x,y; source origin x,y; glyph width,height; positive integer scale; table format 0 (one byte bit-row) or 1 (one byte coverage per cell) |
| line | start x,y; end x,y |
| circle_fill, circle_outline | centre x,y; radius |
| rounded_rect | origin x,y; width,height; radius |
| gradient_v | origin y; height; bottom BGRA colour (top is colour) |
| triangle | three signed x,y vertices |

<!-- claim-lint:ok: Proposed buffer layout implementing docs/gui/gpu.md sections 1.4.1 and 2.1. -->
coverage_spans names a buffer, and no texture; sampled pipelines name a texture and no
buffer. Shapes and gradient name neither. data_bytes is zero without a buffer; otherwise
it names the prefix snapshotted once before validation, no more than the existing buffer
and submission quotas. The crossing buffer contains, from the parameter's aligned byte
offset, `8*height+1` u32 offsets into the following crossing entries. Each entry is two
u32s: finite binary32 x bits and signed winding (+1 or -1). Offsets start at zero, are
monotonic and end at the entry count; entries on each sub-scanline are sorted by x.
The validator reads those offsets in O(rows*8) for the estimate and validates the bounded
crossing entries. The shader visits crossings in order on each of the eight sub-scanlines,
using section 2.1's fill_start, winding, binary32 accumulation, span and to_byte order.
No coverage pixel is computed or uploaded by the CPU.

A blended command's clipped write rectangles must be pairwise disjoint; overlap is EINVAL
at its command index, code 20. BGL splits before the first overlapping instance, preserving
primitive order and giving each new draw a fresh GPU destination snapshot. Invalid or
nonzero unused parameters are EINVAL, code 21. Each blended draw reserves scratch for its
write rectangle union in the target's format, and copies that union before drawing.
The copy sees earlier draws in the same context. The estimate includes its copied pixels,
in addition to the pipeline's fragment work. No blend uses fixed-function blending.

<!-- claim-lint:ok: Proposed budgets implementing docs/gui/gpu.md section 1.4.1. -->
Per-fragment weights are: copy_scaled and nearest_scaled 8192; over and over_straight 128;
coverage and bitmap_text 256; circle_fill and circle_outline 2048; rounded_rect and
triangle 4096; line 8192; gradient_v 32768. box_filter budgets 512 steps per texel in its
bin plus 16384 for mapping and division. coverage_spans budgets 8192 steps per crossing
on that row's eight sub-scanlines plus 8192 per pixel for accumulation and conversion.
The existing 2^31 work cap and all deadlines are unchanged.

## Test-control points

System call 415 takes its latch and ops from BreenGL's contract, section 21: enabled only in a boot
whose first PID 1 is a `/sbin/suite-*` binary, for root, with the latch and caller checked before any
argument is decoded; ENOSYS otherwise. `ARM` is `point: u32, reserved: u32, target: u64, param: u64`
(24 bytes); `FIRED` is `point: u32, reserved: u32, count: u64` out (16 bytes). Points 19 to 31 are
defined; the ones G1 builds:

| Point | Name | Effect |
|---|---|---|
| 20 | `stale-fence` | the next submission's answer is taken as carrying the next fence; it completes `FENCE_MISMATCH` and no other submission completes by it |
| 23 | `mask-capability` | the next admission decision counts the capability id in `param` as absent; one-shot (above) |
| 27 | `device-error` | the next submission ends with fenced command type `0x02FF`, which is undefined by VirtIO and answered with an error. Armed before each of two consecutive submissions of one context, it quarantines that context only, as two ordinary device errors do; it never resets the device |
| 31 | `withhold-fence` | `param` 0: the next submission's completion is ignored, so the watchdog finds it hung; `param` 1: no completion is taken from the device until it is reset; `param` 2, fence depth: the answers to the next context's first four submissions are taken and kept unpublished, so their fences stay incomplete, until a fifth `SUBMIT` on that context waits at the four-in-flight limit; then the four are published in fence order and the fifth is admitted. No completion is invented and no watchdog deadline changes |

G3 builds two more. `target` is the texture handle the point is armed for, 0 for any:

| Point | Name | Effect |
|---|---|---|
| 29 | `wrong-texture` | the next `DRAW_SAMPLED` of the texture samples another texture of the process instead: of its live textures other than the named one, the first created. With no other texture the point does not fire and stays armed. The draw's fence completes as any other's |
| 30 | `skip-upload-visibility` | the next texture `UPLOAD` writes the texture's backing and returns as a completed upload does, counted and with the generation advanced, but its `TRANSFER_TO_HOST_3D` is not sent, so the device keeps the texture's previous contents |

G6 builds two of BreenGL's points (BreenGL's contract, section 21) for the GPU core, and the gauge
`DISARM_ALL` restarts:

| Point | Name | Effect |
|---|---|---|
| 2 | `irq-hold` | the next `SUBMIT` holds IRQs masked for 2 ms during its validation, through the kernel's `without_interrupts`, so `gpu_irq_masked_max_us` reads at least 2,000 |
| 3 | `emit-diagnostic` | the next syscall 416 call, from any thread, writes BreenGL's 32-byte payload `BREENGEL-EMIT-DIAGNOSTIC-PAYLOAD` and a newline through `serial_println!` before it does anything else, 33 bytes counted toward that thread; armed with `param` 1, the `serial_println!` writer |

<!-- claim-lint:ok: Normative G6 interface requirement from docs/gui/gpu.md section 1.16, not a measured result. -->
`DISARM_ALL` also restarts `gpu_irq_masked_max_us` at 0. Every point is one-shot: it affects the next
occurrence of its event, counts one firing and disarms itself. The other points arm and do nothing until
their milestone builds them.

## Observation outside syscall 416

<!-- claim-lint:ok: Normative G6 interface requirement from docs/gui/gpu.md section 1.16, not a measured result. -->
- `/proc/trace/counters` lists, one `name: value` line each: `gpu_contexts` (the contexts the core holds
  for every process, including destroyed handles whose work is outstanding), `gpu_resources` (the resources
  it holds, including the kernel's per-context and per-submission ones and dropped resources still held),
  `gpu_resource_bytes` (bytes charged to processes' GPU memory quotas and not yet released: resource backing,
  staging, and `SUBMIT`'s copies and vertex storage) and `gpu_irq_masked_max_us` (`docs/gui/gpu.md` section
  1.16).
- `/proc/<pid>/task/<tid>/kernel_output` is the decimal count of bytes the kernel committed for output on
  thread `tid`'s behalf (BreenGL's contract, section 22). No syscall 416 path writes kernel output, except
  point 3 above.

## Provenance and reproduction

The provenance rule in `docs/gui/gpu.md` section 1.9 applies to these layouts and device facts. Every
black-box experiment records the commit, the Vigil run id or harness invocation, and the serial or
output excerpt that showed the fact. Reproduce commands match the runs they cite: the provenance
record's commands use exactly the flags the cited runs used, run folders keep `host-renderer.json`, and
`scripts/vigil-record.sh` records whether the boot had a display window.

### G5 fractional-square literal

<!-- claim-lint:ok: Normative input and expected bytes implementing docs/gui/gpu.md section 2.1; not a measured result. -->
For the closed square with corners (0.25,0.25), (3.75,0.25), (3.75,3.75),
(0.25,3.75), the four-by-four coverage bytes, after eight sub-scanlines and
section 2.1's binary32 to_byte order, are:

```text
143 191 191 143
191 255 255 191
191 255 255 191
143 191 191 143
```

`pipelines/coverage-spans-square` compares the coverage blend of these literal bytes.
