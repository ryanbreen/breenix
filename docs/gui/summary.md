# The GUI efforts after the SDK: summary

2026-10-15, against `origin/main` `53efe72e`; third and final revision, the basis for authoring the `breengel`
effort. The plan of record is `docs/gui/plan.md`. It replaces the first design and answers the critique's 43 findings,
the re-check's 10 new problems and the second re-check's 8 (Appendix A). No finding is open. The four defects
the second re-check said to fix first are repaired in the plan. Each remaining contract names the milestone and
case that closes it and belongs to a later effort (plan section 23). `breengel` has none open.

Since 2026-10-16 the platform is GPU only: the `gpu` effort (GPU rendering) comes between `sdk` and
`breengel`, and BreenGL is rebuilt on it. `docs/gui/gpu.md` holds that effort's design and contract and governs
wherever this summary or the plan describes CPU rendering or a software compositor.

Since 2026-10-16 x86-64 is also out of the GUI efforts for now. The x86 gate host is VGA only, so x86-64 is
measured up to the shell and no further; `gpu`, `breengel`, `window-manager`, `ui-testing` and `gui-apps` are
measured on ARM64 QEMU, Parallels and VMware (their `targets` in `docs/efforts/path.json`). x86-64 joins them
when a dedicated Breenix hardware box replaces the current x86 gate host, and the x86-64 items below wait for
it.

## What exists today

- **BreenGL is a window client, not a GL.** `libs/breengel` (about 650 lines) creates a window with a CPU
  pixel buffer, presents it and reads input. libgfx draws in software; libimg reads only 24-bit BMP; libbui has
  no text field.
- **Only Parallels shows client windows today.** VMware draws bwm's chrome but leaves client contents black
  (#1047). ARM64 QEMU has no compositor backend, so bwm exits. x86-64 has no window interface or pointer
  driver. (From source and issues; nothing was booted.)
- **The window syscalls are unsafe.** Any process can map, read, inject into or reposition another's window.
  On ARM64, user pointers are read without the EFAULT fixup. Blocking calls have no timeout.
- **Input reaches bwm as terminal bytes,** with no key releases and no key identity. Modifier hotkeys are
  disconnected, focus cycling does not cycle, and minimizing hides the frame but not the window's content.
- **There is almost no testing.** Nothing in the guest checks a pixel, and the desktop boot marker prints
  before bwm checks for a GPU. Host capture and input are unreliable: Parallels captures arrive partial
  (#930), keys drop while the host's screen is locked, and VMware has neither.
- **The apps have real defects in source.** bcheck marks a test killed by a signal as passing. bterm never
  tells the PTY its size and prints `25l` for `ESC[?25l`. Every blauncher command-line entry opens a plain
  terminal. A shared font bug puts the old font back after a change. Gus Kit's undo is disabled. blogd misses
  messages once the kernel ring wraps.
- **The acceptance script accepts skips.** `suite-verdict.py` scores an all-SKIP run as PASS (checked in the
  source). It is fixed first.

## The four efforts

Each effort runs one suite, and a milestone is done when its cases pass on ARM64 QEMU, Parallels and VMware
with zero skips (x86-64 joins with the dedicated Breenix hardware box). Where a target cannot run a layer yet,
getting it there is milestone work, never an exemption.

| Effort | Milestones, in build order | Cases |
|---|---|---|
| **`breengel`**: BreenGL and the kernel window interface | raster · text · font-parser · inflate · images · widgets · text-entry · icons · windows · isolation · composition · events · scanout · panel | 380 |
| **`window-manager`**: bwm and the input core | input-core · input-devices · startup · composition · stacking · focus · pointer · geometry · lifecycle · recovery · clipboard · liveness · performance | 249 |
| **`ui-testing`**: the UI test framework | runner · input · tree · frames · clock · settle · sessions · diagnosis · proof | 206 |
| **`gui-apps`**: every application, the destination | terminal-emulation · bterm · blauncher · bfontpicker · blog · guskit · guskit-collab · bcheck · demos · desktop | 307 |
| **Total** | 46 milestones | **1,142** |

The order and the single applications effort are as the operator ruled. The count rose from 803 because the
critique and the re-check found real gaps: font and image parser safety, display observation faults, input
outcomes, clock boundaries, buffer replay, runner proofs, compositor recovery and app failure paths.

## How the UI tests work

**Everything is decided inside the guest.** The suite runs as PID 1, starts bwm and the app under test as its
descendants, and drives them through virtual devices (`/dev/uinput`) that enter the kernel's input core where
real drivers do, so bwm's real routing and hotkeys run. Real drivers are covered by replaying recorded reports
through their production interrupt paths. Host screenshots and keypresses are never evidence.

**Every frame is one committed transaction.** Apps draw into a back buffer and commit it. The commit carries
the pixels, the semantic tree, the last input event whose handler finished before the frame was begun, and
the clock epoch it drew. A buffer handed back for drawing is first brought up to date with every commit it
missed, copied by the kernel inside `acquire` from a source it holds for the copy, so no other thread can recycle
it mid-copy. The screen is read back from each display device with proof that the device received that frame.
Checks compare against independent expectations (contract formulas and tables, the certified text renderer,
reviewed images), exactly, with one reference for every target.

**Nothing waits on wall time.** Mode and fonts are pinned. A UI clock moves only when the test steps it, and
every app acknowledges each step, which never needs a new frame. Every input event ends in one recorded outcome:
handled by the window manager, delivered, rejected or lost, with a reason. "Settle" waits until the app has
handled the event *and* its result is on screen; minimized windows are never waited on for pixels. Every limit
(parser work, deadlines, repeat timing) is a written number.

**Every check is shown able to fail.** Fault builds of a test app and bwm plant one defect at a time (an
off-by-one pixel, a dropped key, a lying label, a misrouted key), and a scoped inner runner must report each as
the exact expected failure. The real suite runner is itself run as a fixture, to prove it fails cases that never
ran a check or crashed. Kernel fault points work only in suite boots, as the operator decided. A case can only pass or
fail. On failure, a supervisor that the runner starts beside the case saves the failed frames, the tree of the
compared commit and the input log, and the runner waits for it before killing anything. Every parser, decoder
and rasterizer loop is counted against a written bound that includes output-area work, and kernel output is
counted once per byte at a single boundary, so a quiet-errors check cannot double-count nested writers.

## Biggest risks

1. **Parallels observation.** VirGL composes in host GPU memory, never read back by the guest. One boot
   decides: if fenced readback works, VirGL stays; if not, Parallels uses the software compositor. It never
   passes on a reconstructed image.
2. **Effort one's kernel work is large.** It covers an arch-neutral window interface, the commit protocol, a
   software compositor on three display backends, and readback. It may need a minimal Tier-1 edit to x86-64
   syscall dispatch.
3. **x86-64 rendering speed.** Userspace there is soft-float. If glyphs differ between targets or the latency
   bounds fail, the rasterizer moves to fixed point. The bounds are not relaxed.
4. **Two limits that come back to the operator if they bind.** Session start stays at the adopted 300 ms. Glyph
   rasterization refuses any glyph whose computed work bound passes 2²⁸ steps. By estimate that safety
   ceiling sits about a hundred times above what a 72 px glyph needs, and a case checks it by counter. If
   measurement shows either limit out of reach, it comes to the operator as a ruling; it is not quietly relaxed.
   The fault-point question is settled: the operator chose the suite-mode gate (plan section 3.4).
5. **Runtime.** 1,142 cases need longer gate timeouts (up to about an hour for `gui-apps`), set from measured
   costs early on. Old Parallels issues (#454, #457) are unreproduced and need fresh specimens before anyone
   blames them.

## Open contracts by effort

`breengel` has nothing open; everything it delivers is settled in the plan. The later efforts each settle a
short list when they are authored (plan section 23):

- **`window-manager`:** bwm's per-gesture pointer grab (stale-scene checked only on a gesture's first batch),
  its `compositor_ack` epoch acknowledgment, and the per-target driver coverage map once the spikes have run.
- **`ui-testing`:** gesture ids with Breengel pointer capture and retry of unstarted gestures only;
  frame-check non-vacuity on the domain left after masks; the evidence arena's tree slot and publication order;
  session supervisors as subreapers with a runner fallback; the real-runner fixture and collection handshake;
  settle over the watermark and presentation.
- **`gui-apps`:** the save protocol on each side of the rename; the file-size-limit witnesses as written (signal
  generation proven separately, recovery by the same process); blogd's log-as-cursor with the sequence-aware
  kernel log; collaboration acknowledgments with forced short sends and EAGAIN; bterm's streaming digests.

## What happens first

1. Enforce zero skips in the shared verdict, with test fixtures, after checking that every existing suite's
   newest run has none.
2. Run four one-boot entry spikes: Parallels device readback (the gate), VMware's mode and surface readback,
   x86-64's 1280x800 mode plus a tablet device, and ARM64 QEMU's fenced 2D transfers.
3. Write `docs/gui/breengel.md`, the contract every BreenGL case cites.
4. Start two tracks in parallel. The library track starts with `raster`, which fixes a clipping bug that writes
   out of bounds today. The kernel track starts with the arch-neutral window interface and the commit protocol.
5. Re-triage the old GUI issues against current main.
