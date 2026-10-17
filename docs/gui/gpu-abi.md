# Syscall 416: argument layouts, the BGL stream and its codes

`docs/gui/gpu.md` section 1.4 fixes syscall 416's ops, their errno rules and what each op does. This
document fixes what that section leaves to the implementation: each op's argument layout, the BGL
command stream `SUBMIT` carries, and the codes the kernel reports. It adds to the contract and changes
none of its rules. The single definition in code is the `gpu-abi` crate (`libs/gpu-abi`, from G1), which
the kernel and libbreenix share; the suite checks the layouts at the offsets given here.

Every structure is little-endian with the offsets shown. `len` must equal the structure's size;
reserved fields and output fields are zero on the way in (EINVAL otherwise) and the kernel writes
reserved fields zero. `VERSION` takes no argument (`len` 0).

## Admission

The device is admitted when it has every capability of the profile (`docs/gui/gpu.md` section 1.5,
item 8). Until it is, every op other than `VERSION` and `CAPS` returns ENODEV before its argument is
decoded, and `CAPS` reports why. Ops the effort has not built yet (`UPLOAD` until G3, `MODES`,
`SET_MODE` and `PRESENT` until G2 and G7) return ENOSYS. An op number above 13 is EINVAL.

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
catalogue programs creation compiled and warmed), 16 reserved. EAGAIN past 8 contexts per process.

**`CONTEXT_DESTROY`, `RESOURCE_DESTROY` (16 bytes):** `handle: u64`, 8 reserved. EINVAL for a handle the
process does not hold.

**`RESOURCE_CREATE` (48 bytes):** `kind: u32` (1 target, 2 texture, 3 buffer), `format: u32`, `width:
u32`, `height: u32`, `size: u64` (bytes, buffers only), `handle: u64` out, 16 reserved. Targets are
B8G8R8A8 or R8G8B8A8; textures add R8; sides are 1 to `max_width`. Buffers are 1 byte to 128 MiB.
ENOMEM past 192 MiB of guest backing per process; EAGAIN past 256 resource handles. A new resource is
attached to every context of the process, and a new context to every resource.

**`SUBMIT` (64 bytes):** `context: u64`, `stream: u64` (address), `length: u64` (bytes, a multiple of
4, 1 to 80 MiB), `fence: u64` out, `error_index: u32` out, `error: u32` out, `estimate: u64` out, 16
reserved. Each buffer range the stream names is copied once, before validation, and validation and
translation read only that copy (`docs/gui/gpu.md` section 1.4.1). A refused stream returns EINVAL, or
EDQUOT for the work cap, with the failing command's index and its code written back. A context that is not
live (quarantined or reset) returns EIO. With four submissions in flight the call waits for one to
complete; a signal ends that wait with EINTR, and a kill within 250 ms.

**`WAIT` (48 bytes):** `context: u64`, `fence: u64`, `timeout_ns: u64` (0 polls), `status: u32` out,
`answer: u32` out, 16 reserved. Returns 0 when the fence completed OK; EIO with `status` and `answer`
written back when it completed otherwise; ETIMEDOUT at the timeout; EINTR on a signal. A fence names its
context: `fence` is `ns << 32 | n` for the context's `n`th submission, and a fence of another context,
or one the context never issued, is EINVAL. A fence is complete only when every earlier fence of its
context is.

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
`failed_submission: u64` out at 56. Both output fields must be zero on input. Waits
for every accepted submission that writes the target, then reads the rectangle as canonical BGRA rows
at `stride` with record v2 (`docs/gui/gpu.md` section 1.8). EIO for an invalid or lost target, or when
the two complementary-poison observations differ (`unwritten` in the record is the count).
G2 assigns the previously reserved output words to name EIO refusals without changing record v2:
`reason` 0 none, 1 unwritten (including skip-transfer), 2 fence-mismatch, 3 resource-mismatch,
4 target-invalid, 5 device-error. `failed_submission` names the invalidating submission for reason 4;
otherwise zero. Refused observations publish no pixels. A completed but unwritten observation returns a
complete diagnostic record with its differing-byte count; identity refusals and invalid targets leave the
record untouched. EFAULT leaves the arguments, pixels, record and readback counters unchanged. All three
output ranges are preflighted before a transfer or observation serial is issued.

**`PRESENT` (32 bytes, minimal G2 binding):** `target: u64`, `fence: u64` out, `answer: u32` out,
4 reserved, `reserved: u64`. Outputs and reserved fields are zero on input. Only the process that most
recently called `take_over_display` may present (EPERM otherwise). Waits for the target's writers, refuses
invalid/lost targets, binds the full target to scanout 0, then flushes it with a device fence. Returns only
once the fenced flush answers, with its fence and answer. The binding holds the resource until a later
binding's flush completes or a confirmed reset ends device access. Record flag 1 and present fence/status
name that binding when reading its target; another display owner cannot read it. G7 adds modes, flipping
and the ownership stack; this operation does not claim those features.

**`COUNTERS` (256 bytes):** `context: u64` in (0 for the calling process), then `u64`s: submissions 8,
draws 16, instances 24, fragment area 32, work estimate 40, image uploads 48, fixture uploads 56,
readbacks 64, ring waits 72 (submissions that waited because four were in flight), device errors 80,
state 88 (a context's: 0 live, 1 quarantined, 2 reset), contexts 96 (the process's live handles),
device contexts 104 (the process's device contexts not yet destroyed, including destroyed handles whose
work is outstanding), resources 112, resource bytes 120 (guest backing held), warmed 128; reserved to 256.
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

`colour` is the canonical BGRA bytes as a little-endian word: B in bits 0 to 7, G 8 to 15, R 16 to 23,
A 24 to 31. Rectangles are clipped to the target; one with `x1 < x0` or `y1 < y0` is refused. Any drawing
command may draw into a target the device left invalid; the target becomes valid again only when a
submission that writes it whole (a `CLEAR`) completes OK. `DRAW_BUFFER` reads its rectangles from the
submission's snapshot of the buffer, never from the buffer after `SUBMIT`.

Validation walks the whole stream before anything reaches the device. Its codes (`error`):

| Code | Refusal |
|---|---|
| 1 | a command's length runs past the stream or is shorter than two words |
| 2 | an unknown command |
| 3 | a drawing command before any `BEGIN` |
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
| 14 | a command that samples a target the device left invalid; no G1 command samples, so G1 never reports it |

The work estimate is, per draw, the clipped area times the pipeline's per-fragment steps (`solid`: 1).

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

Every point is one-shot: it affects the next occurrence of its event, counts one firing and disarms
itself. The other points arm and do nothing until their milestone builds them.

G2 additionally builds these one-shot events (the target argument is the observed target handle for
readback events, or 0 for any):

| Point | Name | Effect |
|---|---|---|
| 19 | skip-transfer | omit the next transfer-back; the two observations expose unwritten bytes |
| 20 | stale-fence, extended | change the next readback transfer's returned echo; the identity check returns reason 2 |
| 21 | wrong-resource | transfer a different owned target of sufficient size; refuse its resource identity with reason 3 |
| 22 | corrupt-pixel | append a GPU solid draw changing pixel (0,0) of the last written target to the complement of the poison; the exact comparison detects this outside the case's interior draw |
| 28 | suppress-draw | omit one DRAW or DRAW_BUFFER during translation, while its real device fence still completes |

The shader grammar, formats and byte order are unchanged from G1.
