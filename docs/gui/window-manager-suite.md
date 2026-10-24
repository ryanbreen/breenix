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

`/proc/breenix/input-replay` is a root-only test-control descriptor governed by
[plan section 3.4](plan.md#34-suite-mode-test-controls). Suite-latched means enabled
only by the kernel's first exec of a suite as PID 1, immutable for that boot;
all replay operations return ENOSYS in shell, desktop, probe and program boots.
Root alone never enables replay. It is a named input control in the section 11
point table, covered by the section 3.4 production absence and latch checks;
`breengel/isolation/test-hooks-absent` must include its entry point before any
implementation lands. This PR specifies a client for an absent endpoint and adds
no kernel endpoint or input injection authority. A 64-byte
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

### Pure decoders and bound-device inventory (W1/W8)

The decode-only test transport is `/proc/breenix/input-decode`, under the same
immutable suite latch and root authority as input-replay. It must call the pure
production decoder, including the production per-endpoint button merger for HID;
it uses the decoder implemented for production input. Production-absence coverage must
include it before an implementation lands.

A virtio request is 16 bytes: selector u32 1, reserved u32 zero, then the eight
recorded event bytes (kind u16, code u16, value i32). Its eight-byte reply is the
normalized core kind/code/value. <!-- claim-lint:ok: Normative W1/W8 decoder-test transport; userspace/programs/src/wm_suite/input_devices.rs sends and judges this layout. -->

A HID request is five u32s (selector 2, endpoint,
descriptor length, report length, reset-state flag), then descriptor and report
bytes. Its 16-byte reply is four i32s: raw X, raw Y, merged buttons and wheel.
Reset-state is 1 only for the capture's first report; subsequent reports retain
the real endpoint merger state. Decoder tests assert the literal result rather than
transport success. A missing implementation fails by endpoint name.

`/proc/breenix/input-devices` snapshots bound input device rows as decimal id,
driver name and whitespace-separated capability tokens. The ARM64 QEMU names are
`virtio-keyboard-mmio` and `virtio-tablet-mmio`. The keyboard declares
`key=1..255`; the tablet declares `abs=0:0..32767`, `abs=1:0..32767`,
`key=272..274` and `rel=6,8`. These rows describe actual bound instances, not
fixtures or a hard-coded coverage claim.

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
client's post-handler commit and a final completed scanout observation covering the workload.
A sample or stepped epoch requires device-completed receipts; full readback
is kept outside the per-sample loop so observation does not dominate the workload.
The client reports its handling commit separately from subsequent animation commits.
They measure completion time, excluding the observer's readback and IPC costs. A bwm FPS
print, snapshot reservation or submission success cannot supply a receipt. No synthetic
history is staged by the host or suite.

### Clipboard

W4 replaces the proposed bwm socket with the kernel's CLIPBOARD_SET,
CLIPBOARD_GET and CLIPBOARD_SUBSCRIBE operations on syscall 414, defined in
[BreenGL 17.1](breengel.md#171-the-system-call). Clients call those operations
with their own registered window. Lengths, UTF-8, embedded NUL, serial ordering,
retained RLIMIT_AS charge and claim-end clearing follow that contract. There is
no in-suite clipboard store or host-staged result.

## W12 recount

The plan's 249 identifiers become **248 E18 identifiers** after
`input-devices/ps2-scancodes` moves to the x86 hardware suite. The five existing
extra controls remain additional: input-core/virtual-keyboard and
virtual-pointer, startup/desktop-control and independent-clients, and
composition/oracle-corruption. The completed table therefore has 253 cases.
HID decoder cases exercise recorded layouts; Parallels/VMware bound completion
rows and boots remain the W8 catch-up, not ARM64 execution evidence.

| Category | Planned after W12 | Extra controls | Runnable target |
|---|---:|---:|---:|
| input-core | 31 | 2 | 33 |
| input-devices | 14 | 0 | 14 |
| startup | 12 | 2 | 14 |
| composition | 27 | 1 | 28 |
| stacking | 19 | 0 | 19 |
| focus | 30 | 0 | 30 |
| pointer | 21 | 0 | 21 |
| geometry | 24 | 0 | 24 |
| lifecycle | 20 | 0 | 20 |
| recovery | 15 | 0 | 15 |
| clipboard | 16 | 0 | 16 |
| liveness | 12 | 0 | 12 |
| performance | 7 | 0 | 7 |
| Total | 248 | 5 | 253 |

Changed assertions, with their identifiers retained:

- W5 geometry/configure-ack waits for LIST's owner size before the frame changes.
- W5 geometry/coalesce allows at most one outstanding ResizeRequested until
  LIST reports a size change.
- W5 recovery/death-mid-configure leaves the owner's size unchanged when
  a pending request is abandoned by compositor death.
- W6 startup/no-gpu-required requires bwm to stay alive and compose on the
  target GPU core, with ARM64 QEMU GL inside the effort.
- W10 composition/chromeless, stacking/chromeless-top and
  stacking/dimmer-only-with-chromeless require chromeless content above both
  bars with the full-mode dimmer geometry in bwm.md.

W11 gives the physical display to each case's bwm. Between cases the stage
restores its own frame. A host send, a submission or a marker alone cannot
pass a native scanout assertion. Runtime verdict evidence and the missing recorded tablet fixtures remain tracked in #2370.

## Executable W12 inventory

The table contains the 248 retained plan identifiers and five additional controls,
253 cases in thirteen categories. The former planned remaining categories are
removed only because their identifiers now have executable case bodies. Runtime
failure, including a missing recorded fixture or production endpoint, is still
failure; an executable inventory does not establish E18 acceptance.

The two target tablet decoder cases need captured descriptors and reports under
`/usr/share/breenix/input-fixtures/parallels.txt` and `vmware.txt`. The image builder installs them from `testdata/input/`. Those recordings
are not available in this change. Missing recordings are harness blockers; they
must be supplied before lane acceptance, not generated from another target's
layout or replaced by synthetic reports.

Each non-comment fixture line is seven whitespace-separated fields: decimal
endpoint id, descriptor hex, report hex, expected raw X, raw Y, merged button
mask and wheel steps. Expected values are independent literal annotations on the
recording. Each fixture has at least two report transitions; VMware includes
multiple endpoints with buttons held and released independently.

## Baseline review corrections

Shared routing setup waits for registry geometry, without requiring client scanout;
composition cases keep their explicit device observation. The `desktop-control` case
checks a completed empty desktop against a literal taskbar pixel to demonstrate a
positive native-readback path independent of the failing client-presentation cases.
A prerequisite failure reports its actual reason rather than diagnosing a distinct
routing defect. Presentation failures remain tracked in #2365.

The existing-windows and restart-adopts cases stop their independent client and wait
for the SIGSTOP acknowledgment before starting bwm. Neither calls the paint barrier;
only pixels committed before startup or restart can satisfy their device comparison.
The registry-wake case paints an unregistered client before the idle interval, then
registers it without painting, pointer input or clock stepping. Only the registry
transition can initiate its observation.

Latency starts after the clear-events reply. The client reports the post-handler
commit separately from later animation commits. Each sample and each stepped client
still needs a device-completed receipt; the final scanout covers the workload without
hundreds of full physical readbacks inside the measurement loop. The workload counts,
latency bounds, memory bounds and case deadlines remain unchanged.

All-minimized waits for both clients' MINIMIZED flags and absence of focus, and checks
both event logs after the negative input interval. Deferred review refinements are
tracked in #2369.

## Reviewed ARM64 GL baseline

Boot `20261024T012512Z-qemu-13504` ran commit
`e3877258e0e72a8ac43a17620a57af90979aa4ed` with a clean tree, four vCPUs,
GL and its display open through the mac-boot slot. Vigil recorded completion and
exit status 1. The runtime result was **9 PASS, 73 FAIL, 0 SKIP**, with 82 terminal
case records. The empty-desktop control, compositor claim, chrome, overlap,
cascade, corruption oracle, client resize, compositor-steal and inject-denied
cases passed. The baseline remains red; incomplete milestones remain blocked
by their planned categories, tracked in #2370.

| Category | Passed | Failed | Skipped |
|---|---:|---:|---:|
| input-core | 0 | 9 | 0 |
| input-devices | 0 | 3 | 0 |
| startup | 2 | 4 | 0 |
| composition | 4 | 2 | 0 |
| stacking | 0 | 5 | 0 |
| focus | 0 | 9 | 0 |
| pointer | 0 | 6 | 0 |
| geometry | 1 | 5 | 0 |
| lifecycle | 2 | 7 | 0 |
| recovery | 0 | 5 | 0 |
| clipboard | 0 | 6 | 0 |
| liveness | 0 | 5 | 0 |
| performance | 0 | 7 | 0 |
| **Total** | **9** | **73** | **0** |

Memory reclamation failed: free pages fell from 84915 to 84833, an 82-page loss
against the unchanged 48-page allowance (#2366). Missing input, epoch, completion
history and clipboard interfaces remain #2367; presentation/update failures remain
#2365. Minor refinements remain #2369. The manifest table check reported only the
inherited BreenGL title mismatch (#2364), with no window-manager mismatch.

Serial SHA-256: `38106664b610bb5c22195b20ac9e515e12d37a9d86475912daa22e9239c76358`.

## Configurable client

`wm-client_test` renders its id-derived quadrant pattern through BreenGL v2. The
case starts it with `--control`; command and reply descriptors are 3 and 4, and
descriptor 5 maps the shared event log. The 32-byte `WMEVT001` header contains
producer, consumer and lost counters. Entries start at page offset 4096 and hold
a window id followed by its 320-byte window event. The producer publishes an
entry before acting on the event, including an event-triggered crash. A full
8192-entry ring reports loss instead of overwriting unread entries.

The command protocol lives in `wm_suite/protocol.rs`. A reply identifies the
selected window, its latest commit, dimensions, pattern seed, published event
count, commit timestamp, processing commit, second window, animation counter and
last pixel-changing commit. Readers stop at the reply's event watermark. A
`COMMIT_NO_CHANGE` reply retains the previous pixel-changing commit: scanout
observation still requires the native composite's render fence, target
generation and client entry for that pixel-changing commit.

The standalone flags are `--ignore-close`, `--slow-commit[=N]`, `--hang`,
`--crash-after=N`, `--two-windows`, `--animate`, `--refuse-resize` and
`--never-read-events`. Slow commit defaults to a four-step period; `N` selects
another positive period. Animation draws a decimal frame counter over the
pattern. Control commands select either window, step animation, change the slow
period or behavior flags, set a title, request a resize, register an initially
unregistered window, create another window, exit or exec. Clipboard commands use
the kernel window operations. `--chromeless` and `--seed=N` configure composition
fixtures. Hang stops command and event servicing after the initial frame;
never-read-events continues servicing commands without draining the window fd.


## Fixed scene hash

`composition/scene-identical` uses the scale-2 2560x1600 mode and exactly three
windows: a normal seed-11 client, a small seed-23 chromeless client and a mode-sized
seed-41 chromeless client. The last window covers both earlier windows and both
bars under W10. The arrow is at (2559,1599), clipped to its black tip. The expected
whole image is the seed-41 quadrant formula, except that final opaque black pixel.
FNV-1a over canonical B,G,R,FF bytes is `aa15277dfb38ce7d`, derived from that literal
formula before observing bwm. The case compares 4096000 of 4096000 mode pixels with the formula. <!-- claim-lint:ok: userspace/programs/src/wm_suite/composition.rs scene_identical loops over the required 2560x1600 mode. -->

The text-field clipboard cases embed the real libbui TextField in wm-client_test
and pass bwm-delivered Ctrl+C/Ctrl+V into its production key handler. The fixture
supplies selection state and reads field text; it performs no clipboard operation
on the widget's behalf. Missing production widget integration therefore fails.


Case configuration files live in a per-process guest configuration root selected
by BREENIX_CONFIG_ROOT. Killing a timed-out case cannot alter the next case's
bwm.conf or hotkeys.conf. Clipboard notice delivery ids and stored change serials
are distinct under W4: the case waits for the notice kind, then checks GET's
change serial and bytes. The fixed scene waits for a later completed pointer
frame before evaluating its formula and hash.
