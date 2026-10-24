# Breenix input interface, version 1

This is the E18 contract (W1, W2 and W8). `libs/input-abi` contains the pure
wire types and numbers. It contains no driver or queue implementation.

## Events and batches

The little-endian `Event` is 32 bytes, aligned to 8:

| Offset | Type | Field |
|---|---|---|
| 0 | u64 | serial |
| 8 | u64 | time_ns |
| 16 | u32 | device |
| 20 | u16 | kind |
| 22 | u16 | code |
| 24 | i32 | value |
| 28 | u32 | flags |

Kinds and codes are published input interface numbers: SYN 0, KEY 1, REL 2,
ABS 3 and Breenix TEXT 6. SYN_REPORT is code 0, SYN_DROPPED is 3 and
Breenix SYN_SCENE is 4. KEY values are release 0, press 1 and device repeat 2.
Buttons are KEY codes 0x110 left, 0x111 right and 0x112 middle. ABS_X/Y are
0/1; REL_X/Y are 0/1, REL_HWHEEL is 6 and REL_WHEEL is 8. Positive wheel
values mean right/up. The code numbers are ABI facts, not imported code.

Flags bits 0..3 are the core-derived Shift, Ctrl, Alt and Super state after
this event. Other bits are zero. Both sides' modifier keys contribute, and
held state belongs to a device: removing one keyboard cannot release a
modifier another keyboard holds. Ctrl and Super remain distinct. Device
repeat reaches the console only; bwm generates UI-clock repeats. <!-- claim-lint:ok: Normative E18 contract allocation, W1-W13 and #2367; not implementation acceptance. -->

TEXT's value is the following payload's byte length, 1..256. Its code is
zero. The payload follows the 32-byte header without padding; the next
header follows the payload. It is valid UTF-8, including embedded NUL.
A reader gets the payload whole. Window Text events may split that text
at UTF-8 boundaries to fit the window interface's 240-byte text field;
concatenation preserves the original bytes and order. <!-- claim-lint:ok: Normative E18 contract allocation, W1-W13 and #2367; not implementation acceptance. -->

SYN_SCENE's value is 32 and its payload is four u64s: scene version,
window id, layout version and gesture id. It precedes a synthetic pointer
batch's events. Gesture ids are nonzero and identify a press through its
release, or a single motion/wheel batch. Real devices have no scene tag.
Only a gesture's first batch is checked against scene/layout versions;
subsequent captured batches follow the window and capture owner. See
[plan section 5.5](plan.md#55-targeting-scripted-pointer-events).

A batch ends with SYN_REPORT. A write publishes the complete batch or
returns EINVAL without publishing any of it: unknown kind/code, undeclared
capability, invalid value/range, truncated header/payload, invalid UTF-8,
oversize text, missing SYN_REPORT or decreasing injector time are invalid.
Serial and device are zero on injection; the core fills nonzero device ids
and boot-global strictly increasing event serials. A rejected write consumes
no event serial. Timestamps are CLOCK_MONOTONIC for real devices. A virtual
device with injector time uses the supplied nondecreasing per-device time. <!-- claim-lint:ok: Normative E18 contract allocation, W1-W13 and #2367; not implementation acceptance. -->

## Devices and readers

Root opens `/dev/uinput` read/write; another uid gets EACCES. Its first atomic
write is `BINPUT01` (eight bytes), flags u32 (bit 0 injector time), capability
count u32, then that many 12-byte Capability entries: kind u16, code u16,
minimum i32, maximum i32. Reserved flag bits, duplicate capabilities and
non-increasing absolute ranges are EINVAL. A four-byte read returns the
created device id. Later writes are batches. Closing the last reference to
the descriptor removes the device and synthesizes releases for its held
keys and buttons, followed by SYN_REPORT.

`/dev/input/events` is a pollable, independently queued reader. Two opens
receive the same published stream. A read returns whole batches, including
variable payloads; a buffer too small for the next batch gets EINVAL and
consumes nothing. An empty nonblocking read gets EAGAIN. POLLIN means a batch
is queued. A reader's bounded queue holds 1024 event headers plus their
payloads. Overflow emits SYN_DROPPED before the next retained batch, resets
that reader's pressed-state view and records the dropped serials as lost.
No successful read is evidence that a client handled an event. <!-- claim-lint:ok: Normative E18 contract allocation, W1-W13 and #2367; not implementation acceptance. -->

Reader ioctls are `GRAB` 0x424901, `UNGRAB` 0x424902 and `DISPOSITION`
0x424903. GRAB's 16-byte Selection is devices pointer u64, count u32,
reserved u32. The pointed-to array contains distinct u32 device ids. Count
zero selects all devices for an unrestricted desktop. A session supplies its
own explicit ids. Only the live compositor claimant may GRAB or report a WM
disposition (EPERM otherwise); a second holder gets EBUSY. UNGRAB takes no
argument. Taking a grab drains queued input and resets pressed state. <!-- claim-lint:ok: Normative E18 contract allocation, W1-W13 and #2367; not implementation acceptance. -->

The grab belongs to the open reader description, including duplicated fds.
Closing its last reference, including process death, releases the grab and
synthesizes held-key/button releases to their original recipients. Closing
an unrelated reader does not release it. Selected devices reach the grab
holder; other devices end rejected/not-selected. The console receives input
when there is no grab. Reserved Ctrl+Alt+F1..F12 chords reach the console
under a grab, and neither their presses nor releases reach the holder.

Absolute coordinates map by floor((value-min)*(extent-1)/(max-min)), using
wide signed arithmetic and clamping to 0..extent-1. Relative axes accumulate
then clamp. The pointer core includes left/right/middle and both wheel axes.

## Dispositions and observations

DISPOSITION takes a 32-byte record: source serial u64, window u64,
disposition u32, reason u32, derived index u32, reserved u32. Dispositions
are 1 wm-handled, 2 delivered, 3 rejected, 4 client-rejected, 5 lost.
Reasons are 0 none, 1 no-focus, 2 minimized, 3 not-selected, 4 stale-scene,
5 target-gone, 6 grab, 7 stale-layout, 8 gesture-cancelled, 9 overflow,
10 ring-full and 11 compositor-died. A nonzero derived index identifies a
repeat, focus notice or synthetic release in the source's derived namespace.
It never consumes another source serial or masks a missing source outcome. <!-- claim-lint:ok: Normative E18 contract allocation, W1-W13 and #2367; not implementation acceptance. -->

The core ledger is gap-aware. `/proc/breenix/input-ledger` exposes terminal
rows `serial disposition window_id [reason]`. A delivered row becomes
processed only when the target's published metadata acknowledges the serial,
or ends client-rejected. A check of [from,to] returns the first missing
terminal serial, even if a later serial succeeded. The core accounts for
reader overflow and compositor death; bwm accounts for consumption/routing.
No event silently disappears at a full client ring or in bwm's bounded backlog.

A stalled grab holder can leave the bounded source ledger full of pending
rows. Virtual injectors receive EAGAIN when ledger space cannot be reserved.
Physical producers instead terminate pending rows as lost/overflow before
reclaiming space, so physical input and reserved console chords can progress.
The current pressure recovery clears reader backlogs and reports
SYN_DROPPED to the readers. It also terminates rows already read by the holder;
a later source DISPOSITION for such a row receives EALREADY. Narrowing this
recovery to the stalled holder's rows and queue is tracked in #2454.
`input-devices/stalled-ledger` fills 8192 pending rows, checks virtual-injector
backpressure, then checks physical replay progress, SYN_DROPPED and a retained
lost/overflow ledger row within the existing case deadline.

## Driver coverage

| Target | Keyboard | Pointer | E18 evidence |
|---|---|---|---|
| ARM64 QEMU | virtio-input MMIO keyboard | virtio-input MMIO tablet | GL effort row |
| x86 hardware | i8042 PS/2 | virtio-input PCI tablet | hardware lane |
| Parallels | xHCI USB HID boot keyboard | xHCI HID tablet, Parallels layout | catch-up |
| VMware | xHCI USB HID boot keyboard | xHCI HID tablet, merged endpoints | catch-up |

A decoder's literal report tests are distinct from production-completion
replay. Replay uses only an instance actually bound on this target, invokes
the production decode/report/wake/requeue path, and checks device rearming
and actual reader/client delivery. `input-report-skipped` (33) and
`input-wake-skipped` (34) plant one-shot failures on that path. They obey the
immutable PID-1 suite latch and root authority in BreenGL section 21.
`/proc/breenix/input-replay` uses the 64-byte request/receipt in
[the suite observation contract](window-manager-suite.md#input).
A host structure check maps production report callers to this table; an
uncovered driver is a gap. A host's successful input send is transport only.
