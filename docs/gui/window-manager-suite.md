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
pass a native scanout assertion. The remainder remains #2370 until these
identifiers have executable assertions and terminal verdicts.

## Baseline scope and incomplete milestones

This baseline implements a first cut of the plan's 249 cases, not the complete E18
acceptance suite. Additional harness controls do not replace any planned case.
Each incomplete milestone measures both its populated category and an empty
`<category>-remaining` category using the existing planned-category mechanism.
Full-suite and milestone acceptance stay blocked until those cases are implemented.
The outstanding plan case ids are listed below; they are not runtime skips.

- `input-core-remaining`: `uinput-create`, `uinput-nonroot`, `abi-layout`, `key-press-release`, `modifiers-derived`, `pointer-absolute`, `pointer-relative`, `buttons`, `wheel`, `syn-batches`, `time-real`, `two-readers`, `syn-dropped`, `poll`, `grab`, `grab-select`, `grab-drain`, `text-invalid`, `malformed-batch`, `console-keymap`, `console-special-keys`, `console-modifiers`, `ledger-gap`, `console-after-compositor`.
- `input-devices-remaining`: `hid-keyboard-decode`, `hid-rollover`, `hid-tablet-parallels`, `hid-tablet-vmware`, `hid-relative`, `hid-wheel`, `virtio-keyboard-decode`, `virtio-tablet-decode`, `ps2-scancodes`, `replay-wake-fault`, `devices-bound`, `device-capabilities`.
- `startup-remaining`: `no-gpu-required`, `empty-desktop`, `config-test`, `config-invalid`, `fonts-loaded`, `hotkeys-loaded`, `composited-marker`, `epoch-ack`.
- `composition-remaining`: `client-position`, `title-text`, `shadow`, `background`, `appbar-entries`, `chromeless`, `sixteen-clients`, `moving-client`, `frozen-client`, `present-failure`, `visible-only`, `expose-move`, `partial-equals-full`, `damage-bounded`, `outer-bounds`, `scene-identical`, `resized-content`, `cursor-over-window`, `cursor-edge-shapes`, `cursor-stationary-shape`, `clock-test-config`, `no-fps-overlay`.
<!-- claim-lint:ok: Planned case identifiers copied from docs/gui/plan.md section 16, not measured claims. -->
- `stacking-remaining`: `raise-stable-id`, `appbar-order`, `minimize-no-input`, `minimize-close`, `fkeys-skip-minimized`, `chromeless-top`, `dimmer-only-with-chromeless`, `launcher-toggle`, `close-focus-next`, `new-on-top`, `registry-agrees`, `model-sequence`, `minimize-all-restore-all`, `titlebar-drag-raises`.
- `focus-remaining`: `pair-appbar`, `pair-cycle`, `focus-after-close`, `cycle-prev`, `cycle-skips-minimized`, `release-follows-press`, `focus-change-mid-press`, `repeat-delay`, `repeat-rate`, `repeat-cancel`, `repeat-frozen-clock`, `super-return`, `alt-q`, `hotkey-consumed`, `super-double-tap`, `super-single-tap`, `hotkeys-config`, `no-focus-no-delivery`, `focus-new-window`, `routing-after-exit`, `lost-then-delivered`.
<!-- claim-lint:ok: Planned case identifiers copied from docs/gui/plan.md section 16, not measured claims. -->
- `pointer-remaining`: `capture-target-closes`, `scroll-wakes`, `fast-click`, `double-click-threshold`, `double-click-distance`, `chrome-click`, `enter-leave`, `cursor-position`, `modifier-click`, `clamp`, `minimized-none`, `chromeless-click`, `press-raises-delivers`, `hit-test`, `stale-scene`.
- `geometry-remaining`: `drag-top-clamp`, `drag-keeps-titlebar`, `resize-left`, `resize-top`, `resize-corners`, `resize-minimum`, `resize-maximum`, `configure-ack`, `buffer-matches`, `coalesce`, `exit-during-drag`, `exit-during-resize`, `client-title`, `remembered-geometry`, `expose-during-drag`, `resize-repaints`, `geometry-registry`, `chromeless-not-resizable`.
- `lifecycle-remaining`: `close-one-of-two`, `launch-missing`, `launch-fork-fails`, `launch-close-50`, `read-other-denied`, `huge-window`, `commit-flood`, `title-flood`, `never-reads`, `normal-exit-gives-back`, `window-limit`.
- `recovery-remaining`: `death-releases`, `blocked-acquire`, `blocked-wait`, `death-at-composite`, `death-mid-configure`, `death-mid-capture`, `focus-restored`, `clipboard-reset`, `supervisor-gives-up`, `unexpected-crash-fails`.
- `clipboard-remaining`: `set-get`, `empty`, `too-large`, `change-notice`, `concurrent-sets`, `mime-text-only`, `window-required`, `unprivileged`, `textfield-copy`, `textfield-paste`.
- `liveness-remaining`: `deadline-wake`, `compositor-wait-timeout`, `slow-client`, `stopped-client`, `stress-io`, `progress-under-load`, `cursor-shape-idle`.

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
