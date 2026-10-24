# Window-manager suite observations

The E18 suite runs the installed bwm and independent `wm-client_test` processes. The
client owns its window, GPU renderer and event reader. Its private pipes carry commands,
received events, window ids, commit ids and commit-call timestamps to the case. They carry
no compositor instructions. Only bwm calls SET_GEOMETRY, WRITE_EVENTS and COMPOSITE.

The suite defers its scored panel until the end. Each case takes display custody before
starting bwm. The case observes the complete physical scanout with window operation 22,
using the 1,024-byte GPU observation record v2, its actual binding and completed readback
and present fences. Client checks compare the full client rectangle against four literal
quadrant formulas, with opaque X normalization. The expected colours are independently
written in the case; the case imports no bwm code. A client-buffer capture alone cannot
pass a composition check.

The quadrant index is `q = (x >= width/2) + 2*(y >= height/2)`. RGB is
`((37*seed + 61*q) mod 256, (71*seed + 43*q) mod 256, (17*seed + 97*q) mod 256)`.
The producer draws with GPU clears and solid rectangles, without a CPU pixel upload.
The corruption case changes a copied observed content byte and removes a copied record's
window entries; both altered observations must fail the same content predicate.

## Policy expectations

Sizes here are logical pixels multiplied by the display scale. The suite's literal
decoration probes use a 32-pixel titlebar, 2-pixel borders, 20-pixel chrome buttons,
4-pixel button spacing, 28-pixel top bar and 36-pixel appbar. A new window cascades by
50 pixels in each axis. Minimum outer width is 120 pixels. Focused titlebar RGB is
`(40,100,220)`, focused border `(60,130,255)`, close button `(200,50,50)`, taskbar
`(20,22,30)` and appbar `(25,28,38)`. These literals judge bwm's output, not a renderer
or geometry helper imported from bwm.

Resize cases use the current owner-mediated v2 protocol: bwm sends ResizeRequested,
the client calls RESIZE and commits new content, and bwm frames the LIST size. A client
ignoring the request must keep its whole old content. The retired CPU window mapping,
software compositor and v1 drawing calls are not suite paths. The launcher requires GL.

The suite implements the numeric workloads of plan section 12.2: 110 input operations
50 ms apart with 10 discarded, 330 256x256 commits with 30 discarded, a one-second
animation warmup and five-second frame window, three static clients with one-second
settle and five-second CPU accounting, and 20 64 KiB clipboard transfers with two
discarded. CPU accounting includes bwm and the three clients. Stepped stress uses eight
clients and 300 epochs; memory uses eight clients, 60 warmup epochs and 600 measured
epochs. The runner's named allowances are plan section 19's 20, 30 and 40 seconds;
they do not change a latency, rate or memory bound.

## E18 interfaces required by the cases

The GUI plan specifies input events, virtual devices, completion replay, a compositor
status page and clipboard semantics, but leaves their transports open. The following
small wire transports make the cases executable before those layers exist. They are
requirements for those layers, not claims that the current kernel or bwm implements them.
A missing endpoint, malformed response, refused operation or missing client acknowledgment
is a FAIL with its reason. No case passes from a VERSION answer or a successful open alone.
The window ABI and its test-control operations remain as specified by BreenGL v2.

### Input

<!-- claim-lint:ok: Normative E18 input transport requirements, extending docs/gui/plan.md section 5.1. -->
`/dev/input/events` is a separately queued, pollable reader of plan section 5.1's
32-byte little-endian event records: serial u64, timestamp u64, device u32, type u16,
code u16, value i32, flags u32. Type/code are interface numbers; no driver implementation
is copied into the suite. The reader returns whole records. Serial and device are
nonzero, serials strictly increase across devices, and reserved flags are zero.

<!-- claim-lint:ok: Normative E18 creation transport requirements for docs/gui/plan.md section 5.1. -->
Root opens `/dev/uinput` read/write. Its first atomic write creates one device:
eight bytes `BINPUT01`, flags u32 (bit 0: injector timestamps), capability count u32,
then that many `(type u16, code u16, min i32, max i32)` entries. The descriptor owns the
device until close; a four-byte read returns its nonzero device id. Cases filter their
event readers by these ids. Later atomic writes are batches of the input records above ending
in SYN_REPORT (type/code zero). Serial and device in the written records are zero;
the core assigns them. For EV_TEXT (type 6), value is the length of the UTF-8 bytes
following that record, before the terminating SYN_REPORT. Invalid UTF-8 and decreasing
injector timestamps are rejected without publishing a partial batch. A normal desktop
without a restricted session selection admits these virtual devices. The core supplies
release events when a device is removed.

`/proc/breenix/input-ledger` snapshots one row per terminal disposition as
`serial disposition window_id [reason]`. The disposition is `wm-handled`, `delivered`,
`rejected`, `client-rejected` or `lost`; the window id is 0 when no client is involved.
The mixed-script case requires exactly one row per source serial, checks the target of
delivered rows and waits for that client's processed watermark. A later successful
serial cannot cover a missing earlier one.

`/proc/breenix/input-replay` is a root-only suite-latched control descriptor. A 64-byte
request asks the *bound instance's production completion function* to consume a recorded
report and use its production requeue path. Selector u32 at 0 is 1 keyboard or 2 pointer;
bit 8 suppresses input::report once. Bytes 8..15 hold the USB boot-keyboard A report
`00 00 04 00 00 00 00 00`; the keyboard request includes its all-zero release report.
For virtio input the equivalent recorded events are KEY_A (30) values 1 and 0; these
are interface facts. Pointer x/y i32 at 16/20 request an absolute position and a left
press/release at that position, converted to the bound tablet's recorded report layout.
Read returns the request with report-call count u32 at 32, posted buffers u32 at 36
and ring size u32 at 40. The ring must be rearmed; the suppression receipt reports zero
calls. Other bytes are reserved. Missing replay is a failing driver category, even if
ordinary QMP keyboard or pointer injection works.

The host's virtual keyboard and tablet also drive desktop interaction cases. The shared
HOSTKEY transport sends chords. HOSTPOINTER adds
`seq=N x=X y=Y width=W height=H buttons=B wheel=D`, with physical coordinates, HID button
bits left/right/middle 1/2/4 and signed wheel steps. The watcher injects it once through
the target's own virtual device. QMP absolute axes are 0..32767. A host send is never a
verdict: cases require client events, geometry and/or device scanout. Host transport errors
remain in the hostkeys log and missing delivery fails the guest case. QMP smoke does not
replace the production-completion driver cases.

### Clock and completed composites

The suite supplies BreenGL's existing shared BGCLK001 clock through BREENIX_UI_CLOCK,
publishes time then epoch with release ordering and wakes the shared futex at byte 32.
`/proc/breenix/compositor` exposes a snapshot containing `epoch=N`, the epoch bwm has
acknowledged after handling its work. A missing status page fails stepped cases. Clients
publish their epochs and processed input serials through v2 commit metadata. A no-change
acknowledgment does not redraw the client.

`/proc/breenix/composites` is a kernel-owned completion history, readable by root, with
at least the latest 4,096 window entries. Each line is five decimal integers:
`composite_serial completed_monotonic_ns window_id commit_id pixel_generation`.
Only successful device-completed compositions enter it. Entries for unchanged generations
do not count as new animation frames. Performance cases require these receipts, a real
client's post-handler commit and a completed scanout observation covering that commit.
They measure completion time, excluding the observer's readback and IPC costs. A bwm FPS
print, snapshot reservation or submission success cannot supply a receipt. No synthetic
history is staged by the host or suite.

### Clipboard

bwm serves `/run/bwm/clipboard` as a Unix stream socket. The peer is a real process
owning a registered window in this desktop. A request is `BCLIP001` (8 bytes), op u32
(1 SET, 2 GET), payload length u32, window id u64, then the length-counted bytes. GET
has no payload. A reply is status i32 (0 or negative errno), payload length u32, change
serial u64, then payload. SET returns an empty payload; GET returns the stored bytes.
The serial starts nonzero and increases on replacement. UTF-8 and embedded NUL are
length-counted, with the plan's 16 MiB bound. Closing the setting client's window does
not clear the stored bytes. The suite's clients call the real socket; there is no
in-suite clipboard store and no host-staged result.
