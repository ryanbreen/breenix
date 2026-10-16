# The GUI efforts after the Developer SDK: plan of record

Written 2026-10-15 against `origin/main` `53efe72e`; third and final revision the same day. This plan replaces
the first design as the plan of record and is the basis for authoring the `breengel` effort. It takes the first
design, four recon reports (graphics, the window manager, the applications and the test infrastructure), the
gpt-6.1-sol critique (43 findings), the re-check of the first revision (17 findings still open, 10 new problems
N1 to N10) and the second re-check (6 still open, 8 new problems N11 to N18) as inputs. Those working documents
are not kept in the repository; this plan and `docs/gui/summary.md` are what stands. Every finding is fixed
below, answered with the reason the plan stands, or given an explicit contract together with the milestone and
case that prove it. Nothing in the `breengel` effort is left as an open contract. Appendix A lists each finding
with what changed, and section 23 lists what each later effort must settle when it is authored. Facts about the tree come from the recon unless marked *(checked)*, which means
confirmed in the checkout while writing this plan.

**The operator's ruling stands.** Four efforts follow `sdk`, in this order: **`breengel`** (the BreenGL graphics
library and the kernel window interface), **`window-manager`** (bwm and the kernel input core), **`ui-testing`**
(the UI test framework, as its own effort) and **`gui-apps`** (every GUI application in one effort, the
destination). Nothing in this revision splits, merges or reorders them.

## 0. What changed from the first design, in one paragraph

The order, the single applications effort, the destination and the path JSON shapes are unchanged. What
changed is everything that let a check pass without observing the thing it names. Windows now commit immutable
buffers, so a capture is always one whole committed frame. Each display backend has a written observation
contract, and Parallels gets an entry gate instead of a fallback that rebuilt the expected image in software.
Every injected input event ends in exactly one recorded disposition. Repeat, double-click and gesture timing
move into bwm and run on one declared clock. A step of the UI clock is an epoch that every participant
acknowledges. Settling waits for a causal barrier (the event's handler returned before the frame that shows it
was begun, *and* that frame's pixels were composited), not for a queue to drain. Acknowledging an epoch never
requires a composite. Proof cases run in a scoped inner executor, and the runner's own conversions are proven by
running the real runner as a scoped fixture. Assertions travel on a mandatory transport separate from the live
panel. A session supervisor that the runner starts beside the case collects evidence, and the runner waits for
it before killing anything. The acceptance policy enforces zero skips. Kernel fault points that the negative
cases need exist only when the kernel boots a suite. Expectations must be derived independently of the code
under test. Every numerical limit is written down (section 12). Milestones are smaller, and the case lists are
rewritten: **1,142 cases** in place of 803.

## 1. Decisions at a glance

1. **BreenGL is a 2D library and window client, not a GL.** Its effort also owns the kernel window interface,
   the software compositor and the display observation path, because nothing above it reaches four targets
   until they exist. GPU or 3D client rendering is a later, separate effort.
2. **Windows commit immutable buffers** (section 3.1). A client draws into a back buffer and `commit()`s it.
   The kernel publishes pixels, tree, processed-input watermark, clock epoch and damage as one transaction,
   and keeps the committed buffer immutable until the compositor and any capture in progress are done with
   it. A buffer handed back for drawing is first brought up to date, by the kernel inside `acquire`, with
   every commit since its own. No process holds a pin between syscalls. This replaces the single shared
   buffer and the tree seqlock.
3. **One software compositor serves ARM64 QEMU, x86-64 and VMware.** On Parallels, VirGL composition stays
   only if fenced device readback of a changing scene works. If it does not, the software compositor becomes
   Parallels' production path too (section 4.2). No target ever passes on a reconstructed image.
4. **Each backend has an observation contract** (section 4.1). It says what the guest reads, which completion
   proves it reached the device, how the readback is tied to a composite serial, and what remains outside the
   guest's view. Planted faults prove each contract can fail.
5. **The window interface is arch-neutral and authority-checked.** It has a published table of which caller
   may do what, with exact errors (section 3.2), `copy_from_user`/`copy_to_user` on every op, a destroy op,
   bounded waits with EINTR, named event fields, and no test hooks reachable outside suite boots.
6. **A kernel input core replaces "keyboard as stdin bytes"** (section 5). It carries a versioned Breenix
   event ABI with Linux keycodes. Every real driver reports into it, and a root-only `/dev/uinput` injects into
   the same entry point. Timing policy (repeat, click counting, Super double-tap) belongs to bwm, not the kernel.
7. **Every input serial gets exactly one terminal disposition** (section 5.3): handled by the WM, delivered to
   a named window, rejected for a stated reason, or lost for a stated reason. The ledger is gap-aware, so a
   later success cannot hide an earlier loss.
8. **Time is declared per domain** (section 6.1). Case deadlines and real performance use `CLOCK_MONOTONIC`.
   UI behaviour (animation, timers, repeat, clicks, gestures, the taskbar clock) uses the session's UI clock,
   which advances only in acknowledged epochs.
9. **The truth is decided in the guest.** Host tools read logs, extract artifacts and render authoring
   references. Host capture, host input and host pixel comparison never decide a case.
10. **Expectations are independent of the code under test** (section 7.3). They come from literal tables,
    formulas written from the contract, independent fixtures, or a lower layer that its own cases have already
    certified. A shared layout or keymap crate never generates the expectation for itself.
11. **Zero skips is enforced by the acceptance policy**, not hoped for: the verdict rejects any `skipped` count
    above zero, and the GUI suite API has no skip call (section 2.2).
12. **Fault points live in the real components.** Kernel fault controls work only in suite boots, as the
    operator decided (section 3.4); app fault modes live in separately installed `*_test` builds of the real
    program. There is no parallel fake renderer or runner.
13. **The four-target bar has no exemption mechanism.** Where a layer cannot reach a target yet, reaching it is
    named milestone work (section 2.4). One reference image serves all four targets. A divergence between
    targets is a failing case until someone fixes it.

## 2. The bar

### 2.1 What a passing GUI case means

A shell-suite case runs a program with fixed input, files and environment, and compares exact output and exit
status against the specification. A GUI case does the same with different nouns:

| Shell suite | GUI suite |
|---|---|
| fixed stdin bytes | a scripted sequence of key, pointer and text events, each with a serial and a disposition |
| fixed files, umask, environment | a fixed session: mode, fonts, UI clock, seed, locale, time zone, config root, selected devices |
| exact stdout and exit status | committed frames compared to independent expectations, semantic trees checked by role, dispositions and app-side acknowledgments |
| child process with a deadline | the session's compositor and apps are the case's descendants, under its deadline, with failure collection reserved before the kill |
| a missing utility fails the case by name | a missing window, node, frame, disposition or acknowledgment fails the case by name |

A case fails on wrong behaviour, and it also fails when it cannot observe. A window that never commits, an event
with no disposition, a tree never published, a frame check that never ran, a readback without its completion
record, or a target without a path: each one is a FAIL.

### 2.2 Zero skips, enforced

*(checked)* `scripts/suite-verdict.py` today requires `failed=0` and consistent counts, but accepts `skipped`
above zero: a complete, consistent all-SKIP transcript scores PASS (the critique reproduced this with a
synthetic transcript, and the source agrees: lines 108-111 compare the tally and never test `skipped`). The
acceptance policy for every suite therefore changes, before any GUI suite exists:

- A boot passes only when `skipped == 0` and `passed == total == the manifest's case count`, in addition to
  today's rules (one START, every CASE in manifest order, a consistent DONE, no fatal output).
- The rule lives in the shared policy, because zero skips is already the project's bar for every effort. It
  lands with host-test fixtures: an all-SKIP transcript, one SKIP among passes, a DONE missing its last CASE, a
  DONE with a wrong count, and a transcript cut off before DONE. All must score FAIL. Before it lands, the newest
  run of every existing suite on every target is checked for `skipped=0`. Any suite that skips today is
  fixed in the same change ("any failure you find is your problem"), not exempted.
- GUI suites declare themselves with `Suite::gui()`. The manifest validator (`tests/suite_manifests.rs`)
  rejects any call to `skip` in a GUI suite's source, so a GUI case can return only PASS or FAIL.

### 2.3 Host tools are never in a verdict

The host may parse guest-produced records (the verdict script), extract saved artifacts from a stopped disk
for a person to view, render authoring-time references, run host unit tests as the developer loop, run
repository structure tests, and record screenshots for people. It may never compare pixels, inject input, use a
host capture as evidence, or let a host timeout stand in for a guest verdict. The `diskChecks` mechanism, which
reads bytes from the stopped image with debugfs, is not used by any GUI suite. Persistence is proved inside the
guest (section 18, Gus Kit and bfontpicker).

### 2.4 Four targets: milestones, not exemptions

Every case runs on ARM64 QEMU (HVF), x86-64 QEMU on a Linux host, Parallels and VMware. Where a layer cannot
reach a target today, the plan names the work that gets it there and the milestone whose cases fail until it
lands:

| Gap today | Work that closes it | Milestone whose cases fail until then |
|---|---|---|
| Window, registry, input and mouse syscalls compiled only for aarch64 | arch-neutral window interface | `breengel/windows` |
| ARM64 QEMU has no compositor backend; bwm exits | software compositor on virtio-gpu 2D | `breengel/composition` |
| x86-64 has no window interface and no compositor | software compositor on the GOP framebuffer | `breengel/composition` |
| VMware never composes client pixels (#1047) | software compositor into the SVGA3 screen target | `breengel/composition` |
| Parallels has no proven in-guest observation of the composed frame | entry gate, then device readback or the software path (section 4.2) | `breengel/scanout` |
| Modes differ per target (VMware observed at 1920x1200) | 1280x800 mode set on every backend | `breengel/scanout` |
| x86-64 has no pointer device or driver | `virtio-tablet-pci` on the gate and the revived PCI virtio-input driver | `window-manager/input-devices` |
| virtio-keyboard modifiers never reach bwm | modifier state derived in the input core | `window-manager/input-core` |

A case that cannot run on a target prints FAIL with the reason. The design's allowance for a "per-target
reference with a reason and an issue" is removed: a target that renders differently fails until it is fixed.

### 2.5 Independence of expectations

A check is only as good as the independence of its expected value. The rules:

1. An expected value comes from one of these: a literal table written from the contract; a formula written from
   the contract in the test crate; an independent fixture with recorded provenance; or a lower layer whose
   own cases certify it first (for example, the certified text renderer drawing the expected label in an app
   test).
2. The component under test never produces its own expectation. bwm's layout code does not compute the
   expected chrome rectangles, `libkeymap` does not produce the keycodes for typing tests, and Gus Kit's
   drawing code does not produce the expected canvas.
3. Reviewed goldens are used for whole composed scenes where no formula is practical. A golden proves
   determinism and records what a person checked. It never replaces an independent check of an algorithmic
   contract.
4. Each layer that shares code with its expectations is proved able to fail through a planted fault in that
   shared code: `bwm-fault_test` with off-by-one chrome arithmetic and a swapped keymap entry
   (`ui-testing/proof`). Faults in libgfx are planted by mutating a copy of the surface in the test, not by
   shipping a second rasterizer.

## 3. The window interface (delivered by `breengel`)

`docs/gui/breengel.md` is the written contract the `breengel` cases cite, as the POSIX suites cite the
standard. This section fixes its load-bearing parts.

### 3.1 Commit protocol

- A window owns a **buffer set**: two buffers by default, three on request, each `width × height × 4` bytes
  of BGRX (section 7.4). At any time each buffer is *free* (owned by the client), *front* (committed and
  immutable) or *retired* (replaced, but still being read by a composite or a capture copy).
- `commit(window, buffer, meta)` publishes, in one transaction: the buffer's pixels; the tree page written
  for that buffer (section 7.1); `processed_serial`, the input watermark defined in section 6.4; `epoch`, the
  UI-clock epoch the frame renders; and `damage`, at most 16 rectangles that cover every pixel that differs
  from the previous front (section 7.4). The commit gets a `commit_id` that strictly increases per window and
  is never reused. A commit with non-empty damage also starts a new **pixel generation** of the window
  (`pixel_gen`, strictly increasing per window).
- After a commit the buffer is read-only to the client: the kernel maps it read-only in the owner until it is
  released, and a write to it raises SIGSEGV. The previous front becomes retired. It returns to free only when
  the compositor has finished every composite that used it, no capture call is copying it, and no `acquire` is
  replaying from it.
- **Each buffer has a base.** A buffer's `base` is the commit id whose pixels it holds: the id under which it
  was last committed, the commit it was last replayed to by `acquire`, or *none* for a buffer that has never
  held a commit (new, or reallocated by a resize). The kernel keeps each window's **damage log**, the damage of
  its last 8 commits.
- **`acquire` replays in the kernel.** `acquire(window, timeout)` takes a free buffer and, before returning,
  brings it up to date inside the syscall:
  1. Under the window's commit lock it reads the newest front *F* (its commit id and buffer) and takes a
     **replay pin** on *F*'s buffer. It computes the **replay set**: the union of the damage of every commit
     after the acquired buffer's base up to *F* (replaced by its bounding rectangle when it would need more
     than 16 rectangles), or `full` when the base is none or when any commit in that range has left the damage
     log. It then drops the lock, so other threads may commit and acquire during the copy.
  2. Without the lock, it copies the replay set from *F*'s buffer into the acquired buffer. A buffer holding a
     replay pin cannot return to free, so no thread can acquire, overwrite or reallocate *F* while it is the
     copy's source; a concurrent commit only makes *F* retired, and a concurrent destroy or resize frees it
     only after the copy.
  3. It drops the replay pin, sets the acquired buffer's base to *F*'s commit id, and returns the buffer, its
     base and the replay set it copied.

  The buffer therefore equals exactly the committed generation its returned base names, even when other
  threads committed while it was copied. The set is empty, and nothing is copied, when the window has no front
  yet or the buffer's base is already *F*. The pin never outlives the syscall, so the no-pin rule below holds.
  Raw-API clients get the same replayed buffer; neither Breengel nor a raw client copies replay data in
  userspace. Copying only the newest commit's damage is wrong: with three buffers, a buffer two commits old
  would keep the pixels of the commit it missed. With no free buffer, `acquire` blocks until a release, the
  timeout (ETIMEDOUT), a signal (EINTR) or the window's destruction (ENOENT). A thread waiting for the buffer a
  replay is copying from waits for at most that one copy.
- **Concurrent acquires and rebasing.** When an owner holds two acquired buffers (only the raw API and the
  three-buffer option allow it; Breengel's `Window` hands out one at a time) and commits one whose base is
  older than the current front, the commit is accepted and the frame is that buffer's whole content: the last
  committer wins, and drawing the other thread did not copy is not merged. The kernel widens that commit's
  damage by the damage of every commit after its base up to the previous front, so the damage still covers
  every pixel that changed. **When that history is not available** (the buffer's base is none, or any commit in
  the range has left the 8-entry damage log), the commit's damage is the whole window. The same fallback rule
  serves `acquire` and stale commits. An app that wants both threads' drawing in one frame serializes its
  drawing.
- A commit with no compositor present succeeds: the buffer becomes front, and the previous front is released as
  soon as no capture call is copying it. A commit never waits for composition, so a missing or dead compositor
  cannot hang a client.
- `commit_no_change(window, meta)` publishes new metadata (a processed serial, an epoch acknowledgment, a
  tree) against the current front's pixels. It gives a new `commit_id`, keeps the current `pixel_gen`, and its
  damage is empty. It never causes a composite; it is presented by whichever composite already holds its pixel
  generation (section 6.4).
- Two threads of the owner may commit concurrently. The kernel serializes them, a buffer can be committed
  once per acquire (a second commit returns EBUSY), and the commit ids show the order.
- **Captures** (`capture_window`, `scanout_readback`) pin the committed buffer they read only for the duration
  of the copy inside the call, then drop the pin before returning. No pin survives a syscall, so no reader can
  hold a client's buffer pool down: with two buffers, the most a client ever waits for is one composite and one
  capture copy in progress (and, when two of its own threads acquire concurrently, one replay copy). A resize
  or destroy that happens during a capture retires the buffer but frees it only after the copy ends. A
  capture therefore returns a whole committed generation, or ENOENT when the window was destroyed before the
  copy began. `capture_window` takes an optional second destination for the tree
  page; when given, it copies the same committed buffer's tree page under the same pin and returns the commit
  id both copies belong to, so pixels and tree saved from one call always come from one commit (the evidence
  arena relies on this, section 10.3).
- **Remnants.** The only retention past a buffer's normal life is explicit. A root process may register as a
  window's **evidence holder** (the session supervisor does, section 10). When the owner of a window with an
  evidence holder exits or crashes, the kernel keeps the window's last front and its tree page as a read-only
  **remnant** for the holder instead of freeing them. A remnant is outside the dead owner's buffer pool,
  counts against the holder's `RLIMIT_AS`, is limited to 16 per holder (the seventeenth owner death frees its
  buffers as usual and the holder is told `remnant-limit`), and is freed when the holder releases it or exits.
  Nothing is retained while the owner lives.

### 3.2 Authority and errors

Principals: the **owner** (the process, by its kernel identity rather than its PID number, so PID reuse grants
nothing), the **compositor** (the process holding the compositor claim of the current epoch), **root** (uid 0)
and an **unrelated** process.

| Operation | Owner | Compositor | Root (not compositor) | Unrelated | Notes |
|---|---|---|---|---|---|
| create, register, set title | yes | — | — | — | EINVAL on bad size or title; ENOMEM against `RLIMIT_AS` |
| acquire, commit, commit_no_change | yes | no (EPERM) | no (EPERM) | EPERM | EBUSY on a double commit; ENOENT after destroy |
| resize | yes | request only, through configure (section 16) | EPERM | EPERM | the old buffers stay intact on any failure |
| destroy | yes | no (EPERM) | no (EPERM) | EPERM | ENOENT for a stale id |
| read events | yes | — | — | EPERM | |
| write events to a window | no | yes | EPERM | EPERM | |
| set position, z, visibility | no | yes | EPERM | EPERM | |
| map committed buffers read-only | yes | yes | EPERM | EPERM | |
| capture a window | yes | yes | yes | EPERM | |
| map a window's tree page read-only | yes | yes | yes | EPERM | |
| composite, map compositor texture | — | yes | EPERM | EPERM | |
| claim the compositor | — | — | yes if no live claim | EPERM | EBUSY while a claim lives |
| scanout readback | — | yes | yes | EPERM | |
| register as evidence holder; read and release remnants | no (EPERM) | no (EPERM) | yes | EPERM | EBUSY when another holder is registered; ENOENT for a stale id |
| `/dev/uinput` open | — | — | yes | EACCES | |

- **Inheritance:** window mappings are not inherited across `fork` (the kernel marks them do-not-copy), so a
  child has neither a mapping nor authority. Windows are released at `exec` and at exit.
- **Ids:** window ids, commit ids, composite serials and input serials are 64-bit and never reused within a
  boot. A stale id gives ENOENT.
- **Pointers:** every op copies through `copy_from_user`/`copy_to_user`. A pointer range that overflows, is
  unmapped, crosses into an unmapped page or is read-only for output gives EFAULT. An op that fails with
  EFAULT has no side effect: copies in are completed before any state changes, and copies out are completed
  before a commit point.
- **Rollback:** create, register and resize each have a commit point. A failure before it (forced through
  `RLIMIT_AS` at each allocation step) leaves the registry, mappings and accounting exactly as before.
- **Quiet errors:** no window op writes to the serial log on any path. The `serial_println!` calls on the
  syscall path are removed.
- **Test hooks:** ops 0-5, 7, 9 and 27-30 leave the production ABI (ENOSYS). The wait-stress hooks move behind
  `boot_tests`.

### 3.3 Compositor epochs and display ownership

- The compositor claim carries an **epoch** that increases at every claim. Clients see the change as an
  `Event::CompositorChanged { epoch }`. Windows outlive the compositor: they stay registered with their
  last geometry, stacking, visibility and focus as last published, so a new compositor can adopt them.
- When the compositor dies or releases its claim, the kernel resets input grabs and pressed state, sending
  releases for held keys and buttons to the windows that received the presses. It clears the clipboard
  (section 16, `recovery`), returns display ownership to the process that gave it, and advances the epoch.
- **Display ownership is a stack.** The suite runner owns the display between cases. For a GUI case the
  runner gives it to the case's session supervisor (section 10), a process the runner starts beside the case,
  and the supervisor hands it to the session's compositor. When that compositor dies, ownership returns to the
  session supervisor, not to the runner, so evidence can still be collected. The runner gets it back only after
  the collection handshake of section 10 and its sweep have reaped the case's and the session's processes.

### 3.4 Suite-mode test controls

Some negative cases need a fault at a point inside the kernel, such as a skipped transfer, an unanswered fence
or an `fsync` that fails. Some driver cases need to push a recorded report through the production completion
path. These **test controls** are one kernel facility with these properties:

- **The latch.** The kernel enables them only when the first program it executes as PID 1 is a suite binary
  (`/sbin/suite-*`). It latches that fact at that exec, before any userspace code runs, and the latch never
  changes for the rest of the boot: no op sets or clears it, and later execs by PID 1 or anyone else do not
  re-evaluate it. In shell, desktop, probe and program boots every test-control op returns ENOSYS.
- **The production absence check belongs to the real boot path only.** `init` running as PID 1 verifies at
  startup that every test-control op returns ENOSYS and only then prints a new stage, `[init] test controls
  absent`. The shell and desktop boot-path stages include it, the production-profile boot gate
  (`run-aarch64-prod-profile-boot-test.sh`) requires it, and the desktop-mode row of the clean-main matrix
  (section 14.1) requires it. `init` running as an ordinary child (the `gui-apps/desktop/init-desktop` case)
  is inside a suite boot, where the controls are legitimately present. It therefore skips the check, prints
  `[init] not PID 1: test-control check skipped`, and makes no absence claim. A child launch never stands in for
  PID-1 wiring, and `breengel/isolation/controls-latched` proves both halves of this in a suite boot.
- Only root may use them, and every fault is one-shot, scoped to a named point and a named target (a window,
  an inode, a socket, a process or a device), and disarmed at case end.
- The fault points are branches in the real code paths. No parallel implementation exists.
- A host structure test, in the pattern of `tests/fs_fault_production_clean.rs`, checks that every control
  entry point tests the latched flag first, and that the latch is written in exactly one place.

**Decided by the operator: the suite-mode gate.** Kernel fault points for the GUI efforts are runtime test
controls behind the PID-1 suite latch above, compiled into the production kernel and unreachable outside suite
boots. This departs from the compile-time precedent of `fs_fault_inject` on purpose: every case, fault cases
included, runs the one production kernel binary that the rest of the suite certifies, and each milestone stays
one boot per target. A separately compiled fault kernel is not built. The controls are listed in section 11;
the production absence check above and `isolation/controls-latched` are what stand in for the compile-time
absence guarantee.

## 4. Display backends and the observation contract (delivered by `breengel`)

### 4.1 Per-backend contract

The screen capture point is `scanout_readback`. It returns canonical BGRX pixels (section 7.4) together with
an **observation record**: the backend; the scanout resource or address and mode the device is bound to; the
composite serial; every `(window id, commit id)` composed into it; the completion evidence that the device
received it; and the remaining boundary. The record and the pixels are produced atomically for one composite
serial.

| Backend (target) | What the guest reads | Completion that must be present | Bound to the composite | What stays outside the guest's view |
|---|---|---|---|---|
| virtio-gpu 2D (ARM64 QEMU; Parallels if the gate fails) | the full guest backing of the resource bound by `SET_SCANOUT` | for every damaged rectangle of serial *S*, a fenced `TRANSFER_TO_HOST_2D` answered OK, then `RESOURCE_FLUSH` answered OK | the kernel records the fence ids of serial *S*'s transfers and flush, and readback refuses (EAGAIN) until they are answered | the host's copy of the resource and the host window. virtio-gpu 2D has no transfer back, so equality of the host copy rests on the spec's transfer semantics, independently checked through binding and completion |
| GOP framebuffer (x86-64) | the **complete** GOP framebuffer memory after `copy_to_display`, converted from RGB or BGR | the copy of every damaged band of serial *S* has returned, with a store barrier | copy completion is recorded per band against *S* | the host's scanning of that memory into its window |
| SVGA3 screen target (VMware) | the surface bound to the screen target, copied on the host by `SURFACE_COPY` into a readback surface, then written by `READBACK_GB_SURFACE` into that surface's own guest MOB, fenced; the MOB is cleared first and a pixel the device did not write is refused | `UPDATE_GB_SCREENTARGET` for every damaged rectangle of *S*, with a `SURFACE_COPY` of the rectangle in the same command buffer, fenced and answered; the readback fence | fence ids recorded against *S*; the readback copies the surface bound to the screen target, and refuses (EIO) when that is not the composed surface | the host window |
| VirGL (Parallels, only if the gate passes) | the scanout resource read back by a fenced `TRANSFER_FROM_HOST_3D` into a guest buffer | the composite's `SUBMIT_3D` fence and the transfer fence, both answered | fence ids recorded against *S* | the host window |

Rules common to every backend:

- The full destination is read. No sampled band stands for the frame.
- A missing completion, a binding that differs from the composed resource, or a fence that belongs to a
  different serial makes readback fail with a named reason. The readback never returns pixels without its
  record.
- A host display window showing the frame cannot be inferred from guest bytes. The plan claims only what the
  record names.
- **Planted faults** (test controls, section 3.4) prove each rule can fail: a skipped transfer or update, a
  flush or scanout bound to the wrong resource, a fence answered for the wrong serial, and corruption written
  into the last row of the destination outside any damage. The matching `scanout` case must report each one
  with its exact reason, then recover and pass once the fault is disarmed.

### 4.2 Entry spikes, run before the kernel work starts

One boot per target, written as a throwaway program, before the window-interface work is planned in detail:

1. **Parallels (the gate).** Compose three different known scenes with VirGL, read each back with a fenced
   `TRANSFER_FROM_HOST_3D` of the scanout resource, and compare it to its formula.
   - If all three match, VirGL stays Parallels' production compositor, observed through device readback.
     Nearest sampling, integer-aligned quads and no sRGB conversion are then contract, and the `composition`
     cases use the same formula expectations as the software targets.
   - If transfer back is refused, returns stale data or cannot be fenced, the software compositor becomes
     Parallels' production path, on the virtio-gpu PCI device's 2D resources, observed like ARM64 QEMU. VirGL
     composition moves to the later GPU effort, with its own observation path as a precondition.
   - In neither case does a reconstructed image count as evidence.
2. **VMware.** A 1280x800 mode set takes effect; `READBACK_GB_SURFACE` of the screen target works or not. That
   picks which row of section 4.1 applies.
   - **Result.** The 1280x800 screen target works. `READBACK_GB_SURFACE` of the surface the guest uploads
     completes and answers its fence but writes nothing: all 1,024,000 pixels of the cleared MOB stay as they
     were. After a `SURFACE_COPY` of that surface into a second surface on its own cleared MOB, a fenced
     `READBACK_GB_SURFACE` of the copy writes all 1,024,000 pixels, equal to the uploaded scene. The device
     skips readback of a surface the host has not modified, so VMware is observed through a host-side copy of
     the bound surface (section 4.1).
   - **The device uploads lazily.** A fenced `UPDATE_GB_IMAGE`, with or without `UPDATE_GB_SCREENTARGET`,
     answers its fence before the device has read the MOB: when the guest rewrites the MOB right after the
     fence, a later copy returns the rewritten pixels for the whole surface; 100 ms after the fence it returns
     the uploaded ones. With a `SURFACE_COPY` of the surface in the same command buffer as the update, the copy
     returns the uploaded pixels, and the device keeps them after the MOB is rewritten. A MOB change with no
     update never reaches the device's copy. So every presented rectangle carries that copy before its fence.
   - **Proof that it observes the device.** On VMware the `corrupt-outside-damage` fault presents the corrupted
     pixel to the device and then restores the guest backing. A readback of guest memory would miss it; the
     `scanout/fault-outside-damage` case finds it in every VMware suite boot.
3. **x86-64.** The bootloader honours 1280x800 with `virtio-vga,xres=1280,yres=800`; adding
   `virtio-tablet-pci` to the gate leaves every existing x86 suite's verdict unchanged.
4. **ARM64 QEMU.** A fenced 2D transfer plus flush round-trip, with fence ids recorded per serial.

The spike results are written into section 4.1 before the `scanout` cases are written.

## 5. Input (delivered by `window-manager`)

### 5.1 Core and ABI

- `kernel/src/input/` owns devices and events. Every real driver calls `input::report(device, batch)` and
  nothing else. Today's `push_byte_from_irq` keyboard path becomes a consumer: with no compositor attached, a
  console keymap turns key events into console bytes, so shell, probe and suite modes behave as they do now.
- **The Breenix input event ABI, version 1** (`docs/gui/input.md`): `struct breenix_input_event { u64 serial;
  u64 time_ns; u32 device; u16 type; u16 code; i32 value; u32 flags; }`, 32 bytes, little-endian, with literal
  offsets in the contract. Types and codes follow Linux's published `input-event-codes.h` (interface facts).
  Batches end with `SYN_REPORT`. `SYN_DROPPED` reports overflow. One Breenix type, `EV_TEXT`, carries a UTF-8
  payload of 1 to 256 bytes in a following length-prefixed record. A write with invalid UTF-8, a truncated
  record or an oversize payload is rejected whole with EINVAL.
- **Devices** declare capabilities: keys, absolute axes with their ranges, relative axes, buttons and wheel.
  Events outside a device's capabilities are rejected. Absolute axes map to screen pixels by the contract's
  formula: `x = (value - min) * (width - 1) / (max - min)`, rounded down.
- **Serials** are assigned by the core per event and are strictly increasing across all devices.
  Timestamps: a real device's events carry `CLOCK_MONOTONIC`; a `/dev/uinput` device created with
  `time=injector` carries the injector's timestamps, which must not decrease per device (EINVAL otherwise).
  A session creates its devices this way and stamps them with UI-clock time (section 6.1).
- **Modifiers** are derived in the core from key events, the same way for every driver. This removes the
  per-driver statics and the HID-only Super latch. Ctrl and Super are distinct keys. Treating Ctrl as Super is a
  setting in `/etc/hotkeys.conf`, off by default.
- **Device removal** (a uinput descriptor closed, a USB device gone) sends releases for every key and button
  the device holds.
- **Repeat is compositor policy.** The core passes through device repeats (`value=2`) only to the console
  consumer. Under a compositor, bwm generates repeats on its clock (section 6.1).

### 5.2 Real drivers: decoders, completion path and binding

Three layers of evidence, each failable in the guest:

1. **Decoders** are pure functions from a recorded report (USB HID boot keyboard, Parallels and VMware tablet
   layouts, virtio-input events, PS/2 scancode sequences) to core events, checked against literal tables.
2. **The completion path.** A test control (section 3.4) pushes a recorded report through the driver's
   production completion function: the one the IRQ handler calls after it takes a filled buffer from the ring.
   That runs decode, `input::report`, the wake of every reader, and the requeue of the buffer through the
   production requeue function. The case checks that the event reached a reader with a disposition, that the
   compositor woke, and that the device is armed again: for a ring device, its posted-buffer count is back to
   its ring size; for PS/2, which has no ring, its byte-assembly state is idle (no E0 or E1 prefix held). Two
   planted faults prove the case can fail: the report call skipped, and the wake skipped.
3. **Binding.** Each target binds the expected drivers by name, with their capabilities.

**Per-target coverage map.** A completion replay runs only through a driver instance the target actually bound
to a present controller. The plan never instantiates a driver for an absent controller, and it never counts
enumeration as execution. Each target exercises its own keyboard and pointer drivers, and together the four
targets cover every driver that reports input:

| Target | Keyboard: driver and completion function | Pointer: driver and completion function |
|---|---|---|
| ARM64 QEMU | virtio-input over MMIO (`virtio-keyboard-device`) | virtio-input over MMIO (`virtio-tablet-device`) |
| x86-64 QEMU | PS/2 (i8042) interrupt path | virtio-input over PCI (`virtio-tablet-pci`, added in this effort) |
| Parallels | xHCI USB HID boot keyboard | xHCI USB HID tablet (Parallels layout) |
| VMware | xHCI USB HID boot keyboard | xHCI USB HID tablet (VMware layout, merged endpoints) |

The map is a literal table in `docs/gui/input.md`. In the guest, `input-devices/devices-bound` requires this
target's row exactly, and `keyboard-completion-path` and `pointer-completion-path` replay through the row's two
drivers. A host structure test lists every kernel function that calls `input::report` and fails when one
belongs to a driver that no row of the map names, so a driver no target exercises is a visible gap rather than
an unmeasured one. If a target's device configuration changes, the map and the gate configuration change in
the same commit. *(checked)* On x86-64 the PS/2 scancode is read and handled inside `keyboard_interrupt_handler`
in `kernel/src/interrupts.rs`, a Tier-2 file. The step after the port read is factored into a function that
both the handler and the replay call, under the Tier-2 rules (no logging, no locks without a fallback, boot
evidence).

What remains outside the guest's view is the host delivering a physical or host-injected event into the
device's ring. QMP `input-send-event` on QEMU stays an optional smoke check for people. It decides nothing.

### 5.3 Dispositions

Every serial ends in exactly one terminal disposition, recorded in the input **ledger**: a kernel ring that
root and the session can read.

| Disposition | Who records it | Meaning |
|---|---|---|
| `wm-handled` | bwm | a hotkey, gesture, chrome click or drag consumed it |
| `delivered(window)` | bwm records the routing; the kernel completes it | written to that window's ring; it becomes *processed* when the window commits with `processed_serial ≥ s` |
| `rejected(reason)` | bwm or the core | `no-focus`, `minimized`, `not-selected` (device outside the session), `stale-scene`, `target-gone`, `grab` |
| `client-rejected(reason)` | Breengel, in the window's next commit metadata | `stale-layout`: the first event of a tagged synthetic gesture whose layout version no longer matches the window's layout when Breengel dispatches it; `gesture-cancelled`: a later event of a gesture whose capture was cancelled (section 5.5). The delivery then ends client-rejected instead of processed |
| `lost(reason)` | the core | `overflow` (with `SYN_DROPPED`), `ring-full` (with the window's lost count), `compositor-died` |

- The ledger is gap-aware: `ledger.check(from, to)` returns the first serial without a terminal disposition.
  A later success cannot hide an earlier loss.
- Events bwm derives (repeats, focus notices, synthesized releases) carry a derived id `(source serial,
  n)` in a separate namespace and are reported against their source.
- Expected non-delivery is always tested next to a positive twin. A key with no focus is `rejected(no-focus)`
  only after the same key reached a focused window in the same case.

### 5.4 Session device selection

A session's compositor holds an exclusive grab of exactly the devices the session selected, which are its own
uinput devices. Events from every other device, real hardware included, get `rejected(not-selected)` and reach
no client. Taking the grab drains queued events and resets pressed state. Hardware is covered by the driver
cases in section 5.2. The `ui-testing/input/real-input-excluded` case proves a non-selected device cannot
disturb a session.

### 5.5 Targeting scripted pointer events

bwm publishes a **scene version** that increases at every change of geometry, stacking or visibility, and a
**hit test** that returns the topmost window at a screen point. libuitest resolves `click(node)` against the
newest committed tree and scene: it intersects the node's bounds with its clip and with the window's visible
region on screen, chooses the centre of the largest visible rectangle, and confirms with the hit test that the
window there is the node's window. A node with no visible area fails the step by name.

The scene version covers only window geometry, stacking and visibility. A node can also move inside an
unchanged window, when its container scrolls or relays out. Each window therefore has a **layout version**:
Breengel increments it whenever any node's bounds or clip can have changed, which libbui's scroll containers,
lists and layout calls report themselves and custom-drawn apps report through `window.layout_changed()`. It
counts changes made in event handlers at once, before the next frame is drawn, and each commit carries the
version its tree was built at.

Synthetic pointer devices tag a batch with what it was resolved against (a Breenix `SYN_SCENE` record carrying
the scene version, the window id and that window's layout version). The check happens twice:
- bwm records a batch whose scene version is older than the current scene as `rejected(stale-scene)` and
  delivers nothing;
- Breengel, when it dispatches a delivered tagged event to the app, compares the tag's layout version with the
  window's current one. On a mismatch it does not run the handler and reports the serial as
  `client-rejected(stale-layout)` in its next commit metadata.

**Gestures** (contract; settled when `window-manager` and `ui-testing` are authored, section 23). A rejection
is safe to retry only if no handler ran, and a press can change the layout before its release arrives, so both
checks apply to a gesture's start and never to its middle:
- Every synthetic pointer batch carries a **gesture id** in its `SYN_SCENE` record. A gesture is a press
  through its matching release (a click, or a drag with its motions), or a single batch with no button change
  (a motion, a wheel step).
- **Only the first batch is checked.** bwm applies the stale-scene check to a gesture's first batch only. Once
  it delivers that batch, it holds an implicit pointer grab on the receiving window until the gesture's release,
  and delivers the gesture's later batches to that window in its current window-local coordinates, whatever
  the scene version has become. Breengel applies the stale-layout check to the first event only. When the
  first event's handler has run, the node that took the press holds **pointer capture**: the gesture's later
  events go to that node whatever the layout version has become, so a press that expands a list or resizes its
  control does not get its own release rejected.
- **Cancellation.** If the capturing node disappears from the tree, its window is destroyed or the grab is
  broken (compositor death, section 3.3), the gesture is cancelled: Breengel delivers `PointerCancel` to the
  capture owner, which clears its pressed state without activating, the capture and the grab end, and every
  remaining event of the gesture is recorded `client-rejected(gesture-cancelled)` (or `rejected(target-gone)`
  by bwm when the window is gone).
- **Retry only an unstarted gesture.** Breengel reports, per gesture id in its commit metadata, whether the
  gesture `started` (the first event's handler ran) and whether it `completed` or was `cancelled`. libuitest
  retries `click(node)` or a drag only when the rejection fell on the first batch and the gesture never
  started: it resolves the node again from the newest commit, at most three times, then fails the step by
  name. A gesture that started and was then cancelled fails the step by name and is never reissued, since its
  press may already have acted.

Events from real devices carry no tag and are never checked this way. The expected window is part of every
delivery assertion. `ui-testing/input/click-stale-scene` and `click-stale-layout` prove the start checks and
the retry, and `click-stale-layout` and `drag-steps` prove that a layout change on the press or mid-drag ends in
exactly one activation and no held state.

## 6. Time

### 6.1 Time domains

| Behaviour | Clock | Who reads it | Boundary rule |
|---|---|---|---|
| case deadlines, watchdogs, `case_ms_left()` | `CLOCK_MONOTONIC` | runner, libuitest | always real, even when the UI clock is frozen |
| real performance measurements (`performance` categories) | `CLOCK_MONOTONIC` | the case | stated bound, workload and window (section 12) |
| animation, app timers, frame stamps | UI clock | Breengel `clock`, libicon, apps | a timer due at *t* fires in the first epoch whose time is ≥ *t* |
| key repeat | UI clock | bwm | first repeat at press time + delay, then one per period; the boundary is inclusive (a repeat due exactly at *t* fires in the epoch reaching *t*) |
| double-click, Super double-tap | event timestamps | bwm | the second press counts when `t2 - t1 ≤ interval` (inclusive); in sessions timestamps are UI time |
| taskbar clock text, wall time in apps | UI clock's wall value | bwm, apps | fixed at 2026-01-01 00:00:00 UTC plus elapsed UI time |
| WM deadlines (close escalation, launch acknowledgment) | UI clock | bwm | as for timers |

Without a session, the UI clock is `CLOCK_MONOTONIC` and event timestamps are real. Inside a session every
participant reads the session clock, and only session devices deliver events, so the two domains never mix.

### 6.2 The UI clock

- `breengel::clock` (delivered in `breengel`, used by libicon, bwm and apps) reads a clock page named by
  `BREENIX_UI_CLOCK`. Without the variable it uses `CLOCK_MONOTONIC`. The page holds `epoch`, `now_ns`, the
  fixed wall base and a futex word. A participant table in the same page lists each participant's id, its next
  deadline, an animating flag and its last acknowledged epoch.
- The provider counts every read of a real clock that goes through `breengel::clock` while a UI clock is set,
  and publishes the count in commit metadata (`real_clock_reads`). In a session it must stay 0. This gives the
  wall-clock fault a deterministic detector (section 17, `proof`). Its scope is stated: it counts reads through
  `breengel::clock`, and `uitest-app_test`'s `wallclock` fault reads the real clock through that provider. A
  program that calls `clock_gettime` directly bypasses the counter and is outside this detector.

### 6.3 Stepping protocol (delivered by `ui-testing`; the layer-specific use in `window-manager` is noted in section 14.3)

- `step(dt)` writes `now += dt` and `epoch += 1` into the page and wakes every sleeper. `dt` may be 0.
- **Required participants for epoch E** are those with a deadline ≤ the new `now`, those flagged animating, and
  bwm. The harness computes the set from the participant table when it steps.
- **Each required participant acknowledges E**, after it has run every timer due at or before `now`. An
  acknowledgment is metadata, not presentation: it is recorded when the kernel accepts it, and it never needs
  a composite.
  - A client window acknowledges by committing with `epoch = E`, or with `commit_no_change` when it has
    nothing new to draw.
  - bwm is not a client window. It acknowledges in its compositor status page through the
    `compositor_ack(epoch, handled_serial, presented)` op, where `handled_serial` is the newest WM-handled
    serial whose handling has finished, and `presented` is either the composite serial that shows the result
    of everything bwm handled up to that epoch (its chrome, taskbar clock, launcher state) or `none-needed`
    when nothing bwm draws changed. An idle step is acknowledged with `none-needed` and composes nothing.
- **Timer order:** by deadline, then by registration order. A periodic timer whose period passed more than once
  in one step fires **once** and reports `missed = n`, as `timerfd` reports overruns.
- **Integration:** simulations and springs integrate in fixed quanta of 1 ms up to `now`, so the state after
  `step(1000)` equals the state after a thousand `step(1)`. Frames render one per epoch at most.
- **Stamps:** commit metadata carries both the epoch number and the UI time in milliseconds. Cases name
  which one they assert.
- **Hidden and minimized windows** still acknowledge epochs and still process their serials, by
  `commit_no_change` when nothing is drawn. They are not required to be presented (section 6.4). A **stopped
  or hung** participant is not dropped silently: `step` fails naming it once the step bound passes (real time,
  2 s; section 12.3). A case that stops a client on purpose calls `session.exclude(window)` and asserts
  separately that the window stays stale.
- **No clock-compositor deadlock:** composition is never gated by the UI clock. A commit is composited
  whenever it arrives, and `acquire` waits only for buffer releases, which composition provides.

### 6.4 Settle and the causal barrier

**The processed watermark.** Returning an event from a poll is not handling it. Breengel apps receive events
through `window.dispatch(handler)`. The handler returns `Redraw` when the event changed something the window
draws and `NoChange` when it did not, and a serial counts as *handled* only once its handler has returned. Each
commit's `processed_serial` is fixed by Breengel as follows:
- For `commit(buffer)`, it is the highest serial whose handler had returned when that buffer was acquired,
  that is, before the frame began to be drawn. Breengel records the watermark at `acquire` and commits it with
  the buffer. A frame begun before a handler returned cannot claim its event, even when an animation commits that
  frame later.
- For `commit_no_change`, it is the highest serial *s* such that every serial up to *s* handled since the last
  pixel commit returned `NoChange`. A `Redraw` event stops the watermark until a frame acquired after its
  handler returned is committed. A stale or unrelated `commit_no_change` therefore cannot acknowledge an event
  that needs drawing.
- Serials Breengel refused at dispatch (`client-rejected(stale-layout)` or `client-rejected(gesture-cancelled)`,
  section 5.5) are listed in the commit metadata and count as finished.

Raw-API clients set the watermark themselves and are held to the same rule by the proof cases.

**The presentation rule.** Each composite record lists, for every window that was **presentation-required**
when it was composed, the window's `pixel_gen` that the composed frame is correct for. That record is written
even when the window's new pixels are wholly occluded: the compositor then publishes a record with zero composed
pixels and no backend transfer. A window is presentation-required while the registry shows it visible (not
minimized, not hidden) and the case has not excluded it. A commit is **presented** when a composite whose
readback completion is present (section 4.1) lists that window at a `pixel_gen` ≥ the commit's. A
`commit_no_change` keeps its window's `pixel_gen`, so it is presented as soon as the composite that already
holds that generation exists, with no new composite. A window that is not presentation-required is never
waited for. When it becomes visible again (restore), it is presentation-required from that moment, and the
next settle waits for its current generation.

`settle()` returns when all of these hold, and fails naming the first that does not hold within the settle
bound (section 12.3):

1. Every required participant acknowledged the current epoch (section 6.3).
2. Every injected serial has a terminal disposition. Every `delivered(window)` serial is **processed** (the
   window committed with `processed_serial ≥ s`) or `client-rejected`. Every `wm-handled` serial is ≤ bwm's
   acknowledged `handled_serial`. An app that drains events and never commits leaves the serial unprocessed,
   and settle fails naming the window and the serial.
3. Every presentation-required window's newest commit is presented, and bwm's acknowledged `presented`
   composite, unless it is `none-needed`, has its readback completion.
4. Every named async predicate the case registered holds: a tree predicate (bterm's `pty-consumed` counter
   reached *n*, a status node shows "Saved") or an app acknowledgment on a side channel.

Settle never waits for animation to stop: future deadlines are allowed. A case that needs a quiet scene asserts
it explicitly. Three consequences follow: a minimized window acknowledges and
processes but is never waited on for pixels (`settle/minimized-acks`); an idle step is acknowledged by bwm
without a composite (`settle/bwm-ack-no-composite`, `window-manager/liveness/idle-no-composites`); and a
metadata-only commit is presented by the composite that already holds its pixels (`settle/no-change-no-composite`,
`breengel/composition/no-change-commit`).

## 7. Observation

### 7.1 The semantic tree

- Each buffer of a window has a tree page (64 KiB by default, up to 1 MiB on request). Breengel writes the
  tree for the frame into the page of the buffer being committed, so the commit binds tree and pixels (section
  3.1). There is no seqlock, because a committed tree is immutable.
- A node is `{key, parent, role, name, value, bounds, clip, flags}`. `clip` is the node's visible rectangle
  after its ancestors' clips. Roles: window, button, checkbox, slider, textfield, label, text, tablist, tab,
  menubar, menu, menuitem, list, listitem, scrollbar, canvas, image, terminal, dialog, status. Flags: focused,
  enabled, visible, selected, checked, pressed, hovered, expanded, editable.
- A `terminal` node carries its grid: each row's text, each cell's attributes (foreground, background, bold,
  inverse, underline), the cursor row, column and visibility, the counters `pty-consumed` (bytes read from
  the PTY and committed) and `parse-errors`, and two streaming witnesses (section 18, bterm): `stream-digest`
  over the bytes consumed and `rows-retired` with `rows-digest` over the rows that have left the screen.
- With no reader attached, Breengel does not serialize the tree. A counter of serializations makes this
  observable (`tree/no-reader-no-serialize`).
- bwm publishes the desktop's tree in the compositor's page: each window's id, title, owner, screen bounds,
  z order, focus, minimized and chromeless state, the scene version, the appbar entries and the launcher state.
- **A tree is the app's claim, so it is never the oracle for what the pixels show.** Text and custom content
  are checked against scenario-specific expected pixels (section 7.3). The tree checks are role-specific
  invariants: menu items inside their menu, tabs inside their tablist, a scrollbar thumb inside its track, and
  the focused node drawing the theme's focus ring for roles the theme gives a ring. WM policy is checked
  against an independent operation model in the test crate plus client event logs plus screen pixels. The
  kernel registry is another record of bwm's publication, not an independent oracle.

### 7.2 Capture points

1. **Surface:** a case renders into a surface in its own process. Used for BreenGL's library categories, it
   works on all four targets from the first day.
2. **Window:** `capture_window` of a committed generation (section 3.1).
3. **Screen:** `scanout_readback` with its observation record (section 4.1).

### 7.3 Expectation kinds

In order of preference: **formula** (computed from the contract in the test crate: a predicate, a Bresenham
walk from a literal table, a client's id-derived pattern at its expected position); **structural** (a property
of a region: uniform colour, no colour outside R, at least K colours, two regions equal, differs from the
previous commit); **certified-layer render** (the expected label or terminal cell drawn by the text renderer
that `breengel/text` certifies, at the position the contract gives); **golden** (a reviewed PNG with a SHA-256
pin, for whole scenes). Goldens are rendered on the host at authoring time by `tools/gfx-refgen` from the same
`no_std` code, reviewed by a person, and recorded in `docs/suites/frames/PROVENANCE.json` (generator commit,
reviewer, date, what was checked). Frames saved from runs never become references.

### 7.4 Arithmetic and region contracts

- **Pixel formats.** Window and screen pixels are BGRX8888. libgfx writes X as `0xFF`, and every capture
  normalizes X to `0xFF` before hashing or comparing. Surfaces that carry alpha are premultiplied BGRA8888, and
  a valid premultiplied pixel has each colour channel ≤ alpha (fully transparent is all zero).
- **Rounding.** `div255(x) = (x + 128 + ((x + 128) >> 8)) >> 8`, which is exact round-to-nearest of `x / 255`
  for `0 ≤ x ≤ 65025`.
- **Premultiplying** a straight colour `c` with alpha `a`: `div255(c × a)`, applied exactly once at the API
  boundary that accepts straight colour.
- **Source-over on premultiplied values:** `out_c = s_c + div255(d_c × (255 - s_a))` and `out_a = s_a +
  div255(d_a × (255 - s_a))`. The result cannot exceed 255 for valid inputs, so no saturation is needed.
  Literal checks in `raster/alpha-over-examples`: premultiplied red 128 at alpha 128 over black gives 128; over
  white (255) gives 128 + 127 = 255; 64 at alpha 128 over 200 gives 64 + 100 = 164.
- **16-bit to 8-bit** (PNG): `(v × 255 + 32767) / 65535`, integer division.
- **Dirty and damage.** Dirtiness means *written*, not *changed*: a primitive marks the bounding rectangle
  of the pixels it may write after clipping, even when it writes the same colour. A draw clipped to nothing
  marks nothing. libgfx accumulates a single bounding rectangle per surface. A commit's damage is up to 16
  rectangles and must cover every pixel that differs from the previous front. The contract fixes each
  primitive's rectangle exactly (literal table in `docs/gui/breengel.md`). The WM's damage for a move or resize
  is the union of the window's old and new **outer painted bounds**: frame, border, the shadow's offset and
  spread from the theme table, and the cursor sprite when the cursor is inside.
- **Text.** Glyph advances accumulate in 26.6 fixed point and are rounded once per glyph position, never
  truncated per glyph. Layout is one codepoint at a time with legacy `kern`-table pairs. Non-goals, stated in
  the contract: shaping (GSUB, GPOS), hinting, bidirectional text, combining-mark positioning, CFF outlines,
  colour and bitmap glyphs.

### 7.5 Frame check schema (version 1)

`frameChecks` in a manifest, mirrored in the suite's static table and validated against it by
`tests/suite_manifests.rs`:

```json
"frameChecks": [
  { "version": 1, "case": "bterm/command-output", "check": "after-echo",
    "kind": "stored",
    "source": { "kind": "window", "window": "bterm-main" },
    "at": { "checkpoint": "after-echo", "requires": ["settled", "processed:enter"] },
    "reference": { "path": "frames/gui-apps/bterm/command-output/after-echo.png", "sha256": "<64 hex>" },
    "regions": [ { "name": "grid", "x": 0, "y": 28, "w": 640, "h": 352 } ],
    "mask": [], "tolerance": { "channel": 0, "maxDiffering": 0 } }
]
```

- `kind` is `stored` (a reference PNG), `formula` (`"formula": "<name>"`, a function in the suite source named
  by the manifest; its parameters are listed in `"params"`), or `structural` (`"predicate": "uniform" |
  "outside" | "count" | "equal-regions" | "differs-from-previous"` with its parameters).
- `source.window` is a stable window key the scenario assigns when it launches the app; `screen` and `surface`
  are the other sources.
- `at.checkpoint` names the scenario step at which the check runs, and `at.requires` lists the barriers that
  must hold first (section 6.4).
- **Field types.** Every field has one type, and unknown fields are errors. Integers are JSON integers, never
  fractions or strings. A **rect** is `{"x", "y", "w", "h"}` with `x`, `y` ≥ 0, `w`, `h` ≥ 1, and the
  rectangle inside the frame. A **colour** is `"#rrggbb"`. `version` is the integer 1. `tolerance.channel` is
  an integer from 0 to 8. `tolerance.maxDiffering` is an integer from 0 to ⌊compared area / 1000⌋. `regions` is
  a non-empty list of named rects with unique names. `mask` is a list of `{"rect", "reason"}`, each rect inside
  one of the regions, `reason` a non-empty string. Schema version 1 has **no edge exemption**: `edgeRadius` is
  not a field of version 1, and a check that carries it fails validation. A later version that adds one must
  define its type, range, cap and non-edge minimum first, as a schema change with its own validator cases.
- **Formula parameters.** Each formula is declared in the suite source with a typed signature: a list of
  `(name, type)` where the type is `u32 in [lo, hi]`, `i32 in [lo, hi]`, `colour`, `rect`, `window-key` or
  `seed` (u64). `params` must name exactly the declared parameters, each of its declared type and inside its
  declared range. A missing, extra, mistyped or out-of-range parameter fails validation.
- **Structural predicates and their parameters** (contract; settled when `ui-testing` is authored, section 23).
  Operands are bound to the check's declared comparison regions: `region`, `a` and `b` are the **names** of
  entries in `regions`, never free rectangles, and `rect` must lie inside the named region. A mask applies to
  the region that contains it. The **compared set** of a region is its pixels not covered by any mask, and each
  predicate's **effective domain** is computed from compared sets, after masks, never from geometry alone. All
  parameters are required unless marked optional:

  | Predicate | Parameters | Passes when | Effective domain, and its non-vacuity rule |
  |---|---|---|---|
  | `uniform` | `region` (name), `colour` | every pixel of the domain equals `colour` | the region's compared set; at least 2 pixels. `colour` is required, so a blank frame cannot pass by being uniform in some other colour |
  | `outside` | `region` (name), `rect`, `colour` | no pixel of the domain has `colour` | the compared pixels of the region that lie outside `rect`; `rect` lies strictly inside the region, and the domain holds at least 1 pixel **and** at least 75% of the region's geometric area outside `rect`, so masking the outside away cannot leave a check that examines nothing |
  | `count` | `region` (name), `min`, `max` (optional) | the number of distinct colours in the domain is in [`min`, `max`] | the region's compared set; `min` ≥ 2, `max` ≥ `min` when present, and the domain holds at least `min` pixels, so the threshold is achievable |
  | `equal-regions` | `a`, `b` (names) | every compared pair is equal | the pairs (*a*+*p*, *b*+*p*) in which both pixels are compared; equal sizes, not overlapping, and at least 2 pairs and at least 75% of the region's area |
  | `differs-from-previous` | `region` (name), `minDiffering` | at least `minDiffering` pixels of the domain differ from the same source's previous check in this case | the pixels compared in both this check and the previous one; `minDiffering` ≥ 1 and at most the domain's size, and a previous check of the same source exists in the case when this one runs (otherwise the check fails, naming the missing previous check) |

- **Validation** in the guest (before comparing) and in the structure test: every field has its type and
  range; every region lies inside the frame; every predicate operand names a declared region; the compared area
  left after masks is non-empty and at least 75% of each region; every mask has a `reason`; any non-zero
  tolerance has a `reason`; `maxDiffering` is checked against the effective domain; formula parameters match
  their signature; each structural predicate's effective domain meets its rule; `version` is known. Masks that
  can be checked only against the captured frame's size are checked again in the guest, before comparison. A
  check that fails validation fails its case. An empty or under-sized effective domain is always a validation
  failure, never a match.
- The guest's FRAME record (section 8.3) includes the check's definition hash, the reference hash and the
  window, commit and epoch it compared, so a later reader knows exactly what was checked. The boot tooling
  copies the manifests, references, font pins and provenance file into each run's evidence directory before
  boot. That copy is storage, not verdict.

### 7.6 Tolerance policy

Exact (`channel` 0, `maxDiffering` 0) for everything BreenGL draws and everything either compositor
composes. The cursor is in a frame only when the case is about the cursor. No non-zero tolerance is planned.
Adding one needs a reason in the manifest, and the validator caps it as above.

## 8. Runner integration

### 8.1 `Suite::gui()`

Built in two steps:
- **In `breengel`:** display handoff, and the GUI spawn rule. The runner draws its panel only between cases.
  Before forking a case it gives the display away, and after reaping the case it takes it back and redraws. The
  live strip is off for GUI suites. `libbreenix::suite::spawn` (fork, close every runner-owned descriptor in
  the child, then run a closure or exec) is the only way a GUI suite's case creates a process; section 8.2 gives
  the rule.
- **In `ui-testing`:** the mandatory result transport, declared frame checks and FRAME accounting, the session
  supervisor and its collection handshake (section 10), the descendant-scoped sweep, the inner executor, and
  the transcript descriptor that lets the real runner run as a fixture (section 8.4).

### 8.2 Mandatory result transport

- For every GUI case the runner creates a **result pipe**, separate from the optional live-record pipe and
  always present, whatever `/etc/breenix/suite-live` says.
- Records are at most 4,096 bytes (`PIPE_BUF`), so each blocking write is atomic. The writer blocks when the
  pipe is full. The runner drains it while it polls for the case to exit, and after the exit until end of file,
  so the last record before exit is never lost.
- Each record names the running case and a declared check. A record that is malformed, duplicated, names
  another case or names an undeclared check fails the case.
- A case may write at most 256 records. The validator rejects a suite table whose declared checks for one case
  exceed that, so the capacity is checked before the boot. Overflow at run time fails the case closed.
- **Only the case process holds the descriptor.** Every runner-owned descriptor a case inherits (today's report
  pipe, the live pipe, and from `ui-testing` on the result pipe and the supervisor control socket) is
  close-on-exec, and `libbreenix::suite::spawn` closes all of them in every child it creates, including
  children that never exec. **The rule is the same in all four suites.** It is delivered with `Suite::gui()` in
  `breengel`, before any GUI suite exists, so the first two suites, which do not use libuitest, already create
  their helper processes (`kiosk_test`, `wm-client_test`, unrelated principals, uid 1000 children) through
  `suite::spawn`. libuitest's own spawn, delivered in `ui-testing`, is built on `suite::spawn` and adds nothing
  to the descriptor rule. The validator rejects a direct `fork`, `vfork`, `clone` or `posix_spawn` call in any
  `Suite::gui()` source from `breengel` on. When `ui-testing` adds the result pipe, `suite::spawn` closes it as
  well, so the earlier suites satisfy the later validation unchanged and are not rewritten. `runner/forgery-fork`
  proves the rule for a non-exec child, and `runner/forgery-exec` for an exec'd one.
- **The supervisor's channel is separate.** The session supervisor (section 10) never holds the result pipe.
  It reports to the runner on its own **collector socket**, which carries only COLLECT records (section 8.3)
  and the collection handshake.
- Live records (VALUE, WAIT, PULSES) keep their existing best-effort pipe and their limit of 12 per case.

### 8.3 Records and verdict

```text
SUITE <id> FRAME <cat>/<case> <check> def=<sha8> ref=<sha8|formula|structural> source=<window:<key>|screen|surface> commit=<n> epoch=<n> w=<n> h=<n> sha256=<hex> result=match
SUITE <id> FRAME <cat>/<case> <check> ... result=mismatch compared=<n> differing=<n> first=<x>,<y> got=#rrggbb want=#rrggbb artifacts=<complete|partial:<why>|none:<why>>
SUITE <id> TREE <cat>/<case> <name> window=<key> commit=<n> nodes=<n> sha256=<hex>
SUITE <id> INPUT <cat>/<case> sent=<n> processed=<n> handled=<n> rejected=<n> lost=<n> [missing=<serial>]
SUITE <id> COLLECT <cat>/<case> trigger=<fail|crash|timeout|save-all> screen=<at-failure|at-collection|none:<why>> windows=<n> remnants=<n> artifacts=<complete|partial:<why>|none:<why>>
```

The runner writes them to serial from the result pipe and, for COLLECT, from the collector socket. The CASE
line still decides the case, and the runner turns an `Ok` into FAIL when a declared check has no
`result=match` FRAME record, or when any mismatch record was written ("frame check `after-echo` never ran",
"frame check `after-echo` mismatched"). A COLLECT record never changes a verdict, except that a malformed one,
or one for a case that did not request collection, fails the case. A missing one is written by the runner as
`artifacts=none:collector-timeout`. The verdict script applies the same rules to the serial: it reads guest
results and never compares pixels.

### 8.4 The inner scenario executor

Proof cases and harness self-tests need a check to fail *inside* a case that must pass. libuitest provides
`run_scenario(spec) -> ScenarioOutcome`:
- The scenario runs in a child that is a subreaper. `prctl(PR_SET_CHILD_SUBREAPER)` is added to the kernel
  in this effort with Linux semantics *(checked: no subreaper exists in the kernel today)*. Inside it, the
  scenario's logic and its own session supervisor run as a case and supervisor pair under the same runner-side
  code as the outer runner's (section 10), so its session, apps and helpers are its descendants. The
  scenario's supervisor is itself a subreaper (section 10.1), so a scenario's orphans go to that supervisor,
  and to the scenario child only if the supervisor has died. Teardown uses
  the descendant-scoped sweep below and never `kill(-1)`.
- It returns a typed outcome over a pipe: `Pass`, or `Fail { class, operation, evidence }`. Classes include
  `pixel-mismatch`, `tree-mismatch`, `unprocessed-serial`, `missing-disposition`, `settle-timeout`,
  `app-crashed`, `app-hung`, `clock-violation` and `scene-stale`.
- Its FRAME, TREE and INPUT records go into the outcome, never to the outer suite's result pipe.
- The outer case first runs the same scenario without the fault and requires `Pass` (the positive control).
  It then requires the exact expected class, operation and evidence: for example `pixel-mismatch` at check
  `label` with first pixel (12,4). A different class fails the case. So do "any failure", a missing reference, an
  infrastructure error, an app crash in place of a pixel fault, and a kernel fault.

The inner executor proves scenario-level detection. It cannot prove what the **outer runner** does with a
case's records, because an inner outcome never passes through the runner's conversion code. That is proven by
running the real runner as a scoped fixture:

- **The descendant-scoped sweep.** The runner's `sweep()` (*(checked)* `libs/libbreenix/src/suite.rs:580`
  today: as PID 1 it sends `kill(-1, SIGKILL)` and reaps until `ECHILD`; as any other PID it does nothing)
  becomes descendant-scoped in both roles: it finds the runner's children (its direct children, plus orphans
  reparented to it, found by scanning `/proc/<pid>/stat` for its pid as parent), kills them, reaps them, and
  repeats until `waitpid` reports `ECHILD` or the existing 2 s limit passes. As PID 1 every user process is
  reparented to the runner, so the result equals today's sweep. As a subreaper that is not PID 1, it sweeps
  exactly its own descendants. The change lands with a full run of every existing suite on all four targets,
  because every suite shares the runner.
- **The transcript descriptor.** When `BREENIX_SUITE_TRANSCRIPT_FD` names an inherited descriptor, the runner
  writes its transcript (START, CASE, FRAME, COLLECT and DONE records) there instead of the console. A runner
  that is not PID 1 makes itself a subreaper at start, so its orphans come back to it, and after DONE it exits
  instead of idling as PID 1 does. These are the only places the runner branches on its role, and a host
  structure test pins that.
- **The fixture.** `suite-runner-fixture_test`, installed only in `/usr/local/test/bin`, is a GUI suite built
  from the real `Suite::gui()` runner. Its cases misbehave on purpose: a check declared and never run; a
  malformed record; a duplicate record; a record naming another case; `Ok` returned after a mismatch record;
  the case killed in the middle of a scenario; a supervisor that never answers `collect`; a supervisor that
  dies; a double-forked helper left behind. One case passes cleanly as the positive control. Each fixture case has a 3 s limit, and
  each outer case runs the fixture with only the case it checks: the fixture builds its case table from its
  argument, and the runner code is unchanged.
- **How the outer case uses it.** A `ui-testing/runner` case asks its session supervisor to start the fixture
  as a subreaper with the transcript descriptor and the display, reads the transcript to DONE, and requires the
  exact CASE line the real runner produced for each fixture case (for example a FAIL whose message is "frame
  check `label` never ran"), the exact COLLECT record, and that nothing of the fixture's survives its sweep. The runner, its
  conversions, its collection handshake and its sweep are the code under test; only the transcript's
  destination differs from a boot. The cases are `runner/never-ran`, `mismatch-is-fail`, `record-malformed`,
  `record-duplicate`, `record-binding`, `outer-positive`, `outer-case-death`, `outer-collector-timeout` and
  `sweep-descendants`.
- The host scorer (`scripts/suite-verdict.py`) is covered separately by its host fixtures (section 2.2). Host
  tests check the scorer; the guest fixture checks the runner. Neither decides a GUI case.

### 8.5 Manifests and validators

- Manifests `docs/suites/{breengel,window-manager,ui-testing,gui-apps}.json`, binaries
  `/sbin/suite-<id>`, sources `userspace/programs/src/suite_breengel.rs`, `suite_window_manager.rs`,
  `suite_ui_testing.rs` and `suite_gui_apps.rs` (underscores in file names, hyphens in ids).
- `area` is required by the validator and by Vigil's manifest decoder, but none of these is a POSIX area.
  As for the terminals suite, each manifest's `area` is its effort's title: "BreenGL graphics library",
  "Window manager", "UI test framework", "GUI applications". All four are `"timingSensitive": true`.
- `tests/suite_manifests.rs` learns the GUI declaration API (`Suite::gui()`, `frame_check(...)`,
  `.case_limit_ms(...)` and the per-case allowance table), keeps its ordered id and title checks and its
  no-hidden-cases rule, rejects `skip` and direct `fork` in GUI sources, and validates every `frameChecks` entry
  (section 7.5). It gets regression fixtures for a malformed schema, a missing category, a missing reference, a
  check with no table entry, a wrong source file, an unexpected method, a GUI source calling `skip`, and a GUI
  source calling `fork`, `vfork`, `clone` or `posix_spawn` directly instead of `suite::spawn`. These land with
  `Suite::gui()` in `breengel`.
- Vigil's suite manifest decoder and the record parser gain `frameChecks` and the FRAME, TREE and INPUT
  records. Tolerant consumers must report an unknown category, check or boot-path id as an error rather than
  dropping it.

## 9. Sessions and state isolation (delivered by `ui-testing`)

- `Session::desktop()` starts bwm, and `Session::kiosk()` starts `kiosk_test`. Each session runs under a
  **session supervisor** (section 10) that the runner starts beside the case, with its own **config root**:
  `/tmp/uitest/<case-pid>/` holding `etc/fonts.conf`, `etc/hotkeys.conf`, `etc/bwm.conf`, `home/`, `var/log/`
  and the case's fixtures.
- `BREENIX_CONFIG_ROOT` names it. Every participant resolves its configuration paths through it: bwm,
  Breengel's `FontConfig` and `FontWatcher`, bfontpicker's write target, blog, blauncher's catalog and Gus Kit's
  save directory. Production leaves it unset and uses `/`.
- The session environment fixes `TZ=UTC`, `LANG=C.UTF-8`, `BREENIX_UI_CLOCK`, `BREENIX_UI_SEED` and the mode.
  Apps sort directory listings and catalogs by name in byte order, which the contract states.
- Variable system data (PIDs, `/proc` statistics, kernel messages) is never compared to a whole golden. Cases
  compare it to values the case samples independently, or check a narrowly stated structure.
- **Credentials.** The harness runs as root and keeps observation authority. A case that needs a permission
  failure launches the app as uid 1000 with a config root that uid cannot write. It asserts the precondition
  (root's write of the same path succeeds; uid 1000's `access(W_OK)` fails) before it relies on the failure.
- **Cleanup.** The supervisor destroys the session's uinput devices (the core releases held keys) and removes
  the config root. Before and after every GUI case, the runner hashes the global files a GUI program could touch
  (`/etc/fonts.conf`, `/etc/hotkeys.conf`, `/etc/bwm.conf`, `/usr/share/applications/`, `/var/log/`). A
  difference fails that case and restores the file from the runner's copy, so the next case starts from defaults
  whether the previous case passed, failed, crashed or timed out.

## 10. Failure collection and diagnosis

### 10.1 Who runs where

For each GUI case that declares a session (every libuitest case in `ui-testing` and `gui-apps`; the cases of
the first two suites declare none and keep today's order), the runner (PID 1 in a suite boot) creates the
result pipe, a **collector socket** (a socketpair between the runner and the supervisor) and a **control
socket** (a socketpair between the case and the supervisor). It then forks the **session supervisor** first and
the **case** second. Both are the runner's children, and neither is the other's descendant. The supervisor
holds the display (section 3.3), the collector socket and its end of the control socket, and never the result
pipe. The case holds the result pipe and its end of the control socket.

The case never forks session processes itself. libuitest's `Session` asks the supervisor over the control
socket to start bwm, `kiosk_test`, apps and helpers (with their argv, environment, credentials and the
descriptors the runner owns closed). They are therefore the supervisor's descendants. The supervisor reports
each one's pid to the case and each one's exit status as it happens, which is its process-status monitoring.
The case may signal them (it is root) but does not reap them. The supervisor registers as the evidence holder
of every session window (section 3.1) and drains each app's stderr into a ring capped at 64 KiB per app.

**The supervisor is a subreaper** (contract; settled when `ui-testing` is authored, section 23). Under the
Linux semantics `PR_SET_CHILD_SUBREAPER` is given (section 8.4), an orphan goes to its nearest live ancestor
that is a subreaper, and the flag is not inherited across `fork`. Every session supervisor therefore calls
`prctl(PR_SET_CHILD_SUBREAPER, 1)` as its first action, before it launches any descendant, and refuses to start
the session if the call fails. An orphaned grandchild of an app (a double-forked helper) is then reparented to
the supervisor, which reaps it and reports its pid and wait status to the case like any session process. The
same supervisor code runs in boot sessions, in the inner executor's scenarios and in the real-runner fixture,
so all three are proven by one path. **If the supervisor itself dies**, its orphans go to the next subreaper
up: the runner (PID 1 in a boot; the fixture runner and the inner scenario child are subreapers themselves,
section 8.4). The runner then writes `COLLECT ... artifacts=none:supervisor-died` for the case, reaps those
orphans in its sweep, and the case's primary failure stands; a case that passed fails, naming the dead
supervisor.

### 10.2 The collection handshake

The runner's per-case limit *L* is split by the **collection reserve** *R* (1.5 s by default; a case's named
allowance in section 19 includes it):

1. The case runs until it exits or until *L − R*, when the runner kills the case process only (SIGKILL to that
   pid; the supervisor and the session live on). libuitest's scenario deadlines are set inside *L − R*.
2. The runner then always sends one message on the collector socket: `end collect <trigger>` when the case
   failed, crashed or timed out, or when `/etc/breenix/uitest-save` reads `all`, and `end discard` after a
   clean pass otherwise.
3. The supervisor collects if asked (section 10.3), destroys the session's uinput devices, removes the config
   root, writes one COLLECT record when it collected, and answers `done`.
4. The runner waits for `done` until *L* at the latest, relaying COLLECT records to serial as they arrive. Only
   then does it run the sweep, which kills and reaps the supervisor, the session and anything left over,
   takes the display back and redraws the panel. A supervisor that has not answered by *L* is swept anyway,
   and the runner writes `COLLECT ... artifacts=none:collector-timeout`. The case's primary failure stands in
   every outcome.

*(checked)* Today the runner closes the case's report reader and calls `sweep()` immediately after the case
exits (`libs/libbreenix/src/suite.rs:735-744`), and as PID 1 the sweep kills every remaining process. Step 4
moves the sweep after `done` for cases with a session. Other cases keep their current order, since they start
no supervisor. `runner/outer-case-death` and `outer-collector-timeout` prove the order through the real runner
(section 8.4).

### 10.3 What is kept, and how

No process pins a live client buffer between syscalls (section 3.1), so collection never shrinks a client's
buffer pool. Evidence is kept by copies and remnants instead:

- **The evidence arena.** The supervisor creates a shared memory region, owned by the supervisor and mapped by
  the case, with a **screen slot** and a **frame slot** (8 MiB each, enough for a 1280x800 frame) and a **tree
  slot** (1 MiB, the largest tree page, section 7.1). libuitest captures the screen and the checked window or
  surface for each check directly into the arena. For a window check, `capture_window` copies the committed
  buffer's pixels and that same buffer's tree page into the frame and tree slots in one call, under one pin,
  so the saved tree is the compared commit's tree and not a later one (contract; settled when `ui-testing` is
  authored, section 23). Surface checks have no tree, and screen checks save the trees of their windows only as
  section 10.4 states.
- **Publication order.** Each slot has a state word: `free`, `writing` or `final`. libuitest sets `writing`,
  copies into the slot, then writes the slot's record (check name, window key, commit id, epoch, composite
  serial, byte lengths, and the SHA-256 of the pixels and of the tree), and only then stores `final` with
  release ordering. The supervisor reads a slot only after an acquire load sees `final`, and checks the record's
  hashes before writing an artifact from it. A slot found `writing` when the case died is never used; the
  artifact is recorded `partial:case-died`. A slot marked final is never reused in the case, so the arena holds
  the exact frame and tree that failed after any number of later commits and composites, after the case dies
  before writing artifacts, and after the window's owner dies. A frame larger than its slot keeps its first
  8 MiB of rows and is recorded `partial:oversize`. The tree slot is as large as the largest tree page, so a
  tree is never cut by the slot; a tree the app had already flagged truncated (section 7.1) is saved with that
  flag.
- **Remnants** keep the last front and tree of every session window whose app died (section 3.1), so an app that
  crashes right after a commit still has that commit and its tree.
- **At collection** the supervisor also takes a fresh screen readback, captures every live session window, and
  copies the ledger and the composite records.
- **Who writes.** On a failing check, libuitest writes that check's artifacts itself before it reports the
  failure, each file under a temporary name, synced, then renamed. When collection is requested, the supervisor
  writes everything else, and the failed check's artifacts too if the case died before writing them. It names
  its source in COLLECT: `screen=at-failure` from a final arena slot, `screen=at-collection` from its own
  readback. A file the case was writing when it died is never left half-written, and the supervisor records
  `partial:case-died` for it.

### 10.4 What is saved, and what is guaranteed

- On a failure, or always when `/etc/breenix/uitest-save` reads `all`, under
  `/var/uitest/<suite>/<cat>/<case>/`, each file synced: `<check>.got.png` and `<check>.diff.png` (differing
  pixels in magenta over a dimmed reference), `tree-<window>.txt` for the compared commit of a window check
  (from the arena's tree slot; for a screen check, each window's current tree at collection, named with its
  commit id, and `partial:tree-superseded` for a window whose front has moved past the commit the observation
  record lists), `input.log` (every
  serial with its disposition), `screen.png` with its observation record, each dead app's remnant frame and
  tree, and each app's stderr.
- **What is guaranteed when images cannot be kept:** the FRAME record's first differing pixel, both colours,
  counts and hashes, the INPUT record's first missing serial, and the COLLECT record, all on serial. Artifacts
  are capped at 32 MiB per boot. Past the cap, or on an I/O error, the record says `artifacts=none:cap` or
  `none:io` and the primary failure stands. A save failure never turns into a PASS.
- `scripts/uitest-artifacts.py --disk IMAGE --out DIR` extracts the files from a stopped disk and writes an
  `index.html` that shows got, reference and diff side by side. It is a viewer, and its only exit status is
  "extracted".

## 11. Forcing the negative cases

Every negative case names how it forces its condition, asserts the precondition, checks the healthy path first,
then asserts the exact error and the preserved state, then proves recovery. No case waits for a condition to
happen by itself.

| Condition | Forcing mechanism, and the witness that it happened | Cases |
|---|---|---|
| allocation failure in create or resize | `setrlimit(RLIMIT_AS)` just below the next allocation, varied to hit each step; the precondition (the same call succeeds one page higher) is asserted first | `windows/resize-rlimit`, `windows/resize-rollback`, `geometry/client-refuses` |
| backend error, skipped transfer, wrong resource, stale fence, corruption outside damage | display test controls; each reports that it fired | `composition/backend-error`, `scanout/fault-*`, `window-manager/composition/present-failure` |
| panel cursor without save and restore | display test control | `panel/cursor-trail-detected` |
| damage ignored, IRQs held in the copy, a diagnostic on the window path | display test controls `ignore-damage` (the next composite processes the whole window) and `irq-hold` (the next display copy holds IRQs masked for 2 ms); the window test control `emit-diagnostic(path)`, under which the next window op on the calling thread writes one fixed 32-byte payload through the named kernel writer (section 12.1 lists the paths) and reports that it fired | the negative twins in `composition/damage-work`, `composition/flush-irq-bound`, `isolation/quiet-errors` (section 12.1) |
| a replay copy overlapping a recycle of its source | window test control `acquire-stall`: the next `acquire` on a named window pauses after copying half of its replay set's rows, reports that it paused, and resumes when the case releases it or after 1 s | `windows/stale-base-commit` |
| driver report or wake dropped | input test controls on the completion path | `input-devices/replay-*-fault` |
| `fork` failure in a parent | process test control: the next `fork` of pid P returns EAGAIN | `bcheck/spawn-failure`, `lifecycle/launch-fork-fails` |
| `write`, `fsync`, `rename` or directory `fsync` failing, `ENOSPC` | VFS test control: one-shot errno on a named inode and operation; the control reports that it fired | `guskit/save-fail-before-rename`, `save-fail-after-rename`, `bfontpicker/write-failure-preserves`, `dirsync-failure`, `blog/blogd-write-failure` |
| a process dying between two steps of a save or an append | VFS test control `kill-after`: SIGKILL to the calling process as soon as a named operation on a named inode returns, at each boundary in turn (after the data write, after the file `fsync`, after `rename`, after the directory `fsync`) | `guskit/save-interrupted`, `bfontpicker/write-interrupted`, `blog/blogd-killed` |
| short write to a file | `RLIMIT_FSIZE` placed inside the next write, set on the app's pid with `prlimit64` *(checked: `sys_prlimit64` takes a target pid)*. POSIX gives a short count for a write that crosses the limit, and fails a write at the limit with EFBIG while generating SIGXFSZ. The witnesses are separate. **The mechanism**, on the case's own file in the case's own process: a write crossing the limit returns the exact short count; with SIGXFSZ blocked (default action, not ignored), the next write returns EFBIG and `sigtimedwait` with a zero timeout accepts SIGXFSZ, which proves the signal was generated; with a counting handler installed, one more write returns EFBIG and the handler count rises by exactly 1. **The app**: Gus Kit and blogd set SIGXFSZ to `SIG_IGN` at startup (stated in their contracts), and an ignored signal leaves nothing pending to observe, so the app case asserts only what the contract promises: the exact short count, then EFBIG, the error shown, and the same process still running (it acknowledges the next UI-clock step). **Recovery**: the case lifts the limit with `prlimit64` and requires the app's next save or append, by the same pid, to complete exactly | `guskit/save-short-write`, `blog/blogd-short-write` |
| permission denied | the app runs as uid 1000 against a root-owned path; precondition asserted | `guskit/save-error-permission`, `bfontpicker/unwritable`, `blog/read-error` |
| a short send on a socket | socket test control `short-send`: the next `send` on a named socket accepts exactly *k* bytes (1 ≤ *k* < the request) and returns *k*. The host's `partial-writes` counter must rise by exactly 1, and the peer must receive the operation whole once the rest is sent | `guskit-collab/short-send` |
| EAGAIN on a socket | socket test control `eagain`: the next *n* `send` calls on a named socket return EAGAIN with nothing sent, then POLLOUT is raised. The host's `eagain` counter must rise by exactly *n*, and the queued operations arrive in order | `guskit-collab/eagain` |
| a peer that never reads | `collab-peer_test` sets `SO_RCVBUF` to its minimum and never reads, so the host's queue reaches 1 MiB through real backpressure; the host's tree shows the queue growing first | `guskit-collab/queue-limit` |
| an operation lost before the host admits it | `guskit_test` built with `drop-before-admit` (the real Gus Kit, discarding one received operation before sequencing), run in the inner executor | `guskit-collab/lost-before-admission-detected` |
| bytes lost between the PTY and the grid | `bterm_test` built with `drop-byte` (the real bterm, discarding one byte at offset 512 KiB), run in the inner executor | `bterm/throughput-fault-detected` |
| malformed peers and inputs | fixture programs that write exact malformed bytes | `guskit-collab/invalid-*`, `font-parser/*`, `inflate/*`, `png-decode/*` |
| hung, stopped and crashing clients | `wm-client_test` and `uitest-app_test` flags, `SIGSTOP`, `SIGSEGV` | `liveness/*`, `lifecycle/*`, `proof/*` |
| WM layout, routing, keymap and disposition faults | `bwm-fault_test`: the real bwm built with one named fault, installed in `/usr/local/test/bin` only | `proof/fault-misroute`, `fault-chrome-offset`, `fault-keymap-swap`, `fault-lost-disposition`, `fault-real-input` |
| runner protocol violations | `suite-runner-fixture_test`, the real GUI runner with misbehaving cases (section 8.4) | `runner/never-ran`, `mismatch-is-fail`, `record-*`, `outer-*`, `sweep-descendants` |

Like `bwm-fault_test`, `guskit_test` and `bterm_test` are the real program built with one named fault, and
`suite-runner-fixture_test` is the real runner with fixture cases. All are installed only in
`/usr/local/test/bin`, and the production builds contain none of these faults.

## 12. Measurements

### 12.1 Guest observables

Kernel counters use the tracing framework's lock-free counters (`trace_count!`) and are read through
`/proc/trace/counters`, except where a row names a per-thread or per-process file. One new gauge holds a
maximum (the IRQ-masked interval). No logging is added on any hot path. Every observable has a **positive
control** (a known workload must move it by a known amount) and every claim measured with it has a **negative
twin** in the same case (a planted regression the case must report), so a counter that is never incremented,
or a case that cannot fail, is caught.

| Observable | Producer | Used by | Positive control and negative twin |
|---|---|---|---|
| composites completed; pixels composed per composite; damage rectangles per commit | software compositor and VirGL backend | `composition/damage-work`, `window-manager/composition/damage-bounded`, `liveness/idle-no-composites` | a full-screen damage composes exactly 1,024,000 pixels; with `ignore-damage` armed, `damage-work` reports the excess pixels by count |
| window buffer allocations | window interface | `geometry/coalesce`, `windows/churn` | one resize of a two-buffer window counts exactly 2; a twin client that acknowledges every configure separately raises the count to the number of acknowledged configures |
| IRQ-masked maximum in the display copy (gauge, microseconds) | `copy_to_display` and backend copies | `composition/flush-irq-bound` | with `irq-hold` armed the gauge reads at least 2,000 µs, then it is reset and the bound is measured |
| **kernel output bytes, per thread** | the single accounting boundary each produced byte crosses once (see below) | `isolation/quiet-errors` | with `emit-diagnostic(path)` armed, the calling thread's counter rises by exactly the payload's 32 bytes for every path, nested wrappers included, while a second thread emitting meanwhile moves only its own counter |
| soft-lockup reports | the soft-lockup detector | `liveness/progress-under-load` | secondary only: the case passes on completed work, the composite counter rising in every 100 ms window for 5 s, and also requires no new report |
| `fbmmap` calls per process | graphics syscalls | `demos/no-fallback` | the case's own `fb_mmap`, as display owner, counts exactly 1 for itself |
| tree serializations per window | Breengel (in the window's metadata) | `tree/no-reader-no-serialize` | attaching a reader makes it exactly one per commit |
| real-clock reads under a UI clock | `breengel::clock` | `clock/real-clock-counter`, `proof/fault-wallclock` | the test app's deliberate real-clock read counts 1 |
| process CPU time; resident memory | `/proc/<pid>/stat` utime and stime; `/proc/<pid>/status` VmRSS (added to procfs if absent) | `performance/idle-cpu`, `liveness/memory-bounded` | a helper that spins for 500 ms of its own `CLOCK_PROCESS_CPUTIME_ID` shows that much utime + stime within one tick; touching 1 MiB raises VmRSS by at least 1 MiB |
| app counters: `pty-consumed`, `parse-errors`, stream and row digests (bterm); `queued-bytes`, `partial-writes`, `eagain`, submitted and acknowledged ids, op log (Gus Kit host) | the app's tree | `bterm/throughput`, `guskit-collab/short-send`, `eagain`, `submitted-acked` | `bterm/throughput-fault-detected` and `guskit-collab/lost-before-admission-detected` (fault builds); the forced short send and EAGAIN move their counters by exactly 1 and *n* |

**Kernel output accounting.** A single global count of serial bytes cannot support `isolation/quiet-errors`:
a count kept by the logger misses output that bypasses it, and unrelated logging elsewhere would make the case
fail. The counter is
therefore per thread, and it is incremented where bytes are produced, not where they are drained.

The public writers nest: *(checked)* on ARM64 `_log_print` calls `_print`, `emergency_print` calls
`try_print` and `write_byte` calls `write_bytes_atomic` (`kernel/src/serial_aarch64.rs:324-373`), and on x86-64
`write_byte` calls `write_bytes_atomic` (`kernel/src/serial.rs:602-603`). Counting at each of them would count a
nested byte twice. The count is therefore taken at **one accounting boundary per byte**: the lowest function
that commits a byte for output on the producing thread, below every wrapper and above every drain.

- **The boundaries.** *(checked against the writers at `53efe72e`)* On x86-64: `queue_byte`, which puts a byte
  on COM1's transmit queue (reached from `write_bytes_atomic` and from the `Unit` writer used by `_print`,
  `try_print` and `write_str_bounded`); the log ring's `append` inside `enqueue_log`, which puts a formatted
  line on COM2's ring (reached from `_log_print` and the kernel logger); and `emergency_print`'s direct port
  writer. On ARM64: `uart_owner::write`, through which every `serial_line` record passes (`_print`,
  `try_print`, `emergency_print`, `_log_print`, `write_byte`, `write_bytes_atomic`, `try_write`). Lock-free
  raw writers in non-Tier-1 files (`tracing::output`'s `raw_serial_char` and its string writer) are
  boundaries too. Each boundary adds the caller's payload bytes once, with one relaxed atomic add, to the
  current thread's `kernel_output_bytes`. Wrappers never count, so nesting cannot double-count.
- **Drains never count.** Bytes leave the COM1 queue in the transmitter interrupt and the COM2 ring in
  `drain_until`, `write_burst` and the idle drain, often on another thread. Those paths sit below the boundary
  and never call it, so a later drain by another thread does not move any counter.
- **Deferred lines are counted when posted.** A `log_notice` line (posted where logging is not allowed, and
  written out later by `write_notices` from whichever thread runs next) counts toward the posting thread when it
  is posted. `write_notices` passes those bytes through the boundary marked already counted, which adds
  nothing.
- **Lead bytes are not payload.** The newline that `serial_writer::lead` inserts before a record when another
  writer's line is open is not counted. It is written only ahead of a payload, so zero payload means zero
  output, and the exact witness below does not depend on another writer's timing.
- **Unattributed bytes.** Where the current thread cannot be read safely (before per-CPU data exists, on the
  panic and double-fault paths, and in `emergency_print`), the boundary adds to a global
  `kernel_output_unattributed` counter instead. On ARM64, where `emergency_print` is a wrapper of `try_print`,
  it passes an unattributed mark down to the same boundary. TTY and console output (`Writer::Tty`,
  `Writer::Console`) is userspace output and is not counted.
- **Uncounted raw sites.** Some breadcrumbs write the port or UART directly and stay as they are: the two
  unreachable markers after `iretq` in `kernel/src/syscall/entry.asm` (Tier-1), the double-fault handler's
  inline breadcrumbs in `kernel/src/interrupts.rs`, and the `test_userspace.rs` markers. The structure test
  keeps them on an explicit list by location. The list is completed from the source when the test is written,
  and a site may be on it only if it runs before per-CPU data exists, on a path that never returns, or in
  test-only code. No window op reaches any of them, and `quiet-errors` covers that reachability.
- A thread's counter is read through `/proc/<pid>/task/<tid>/kernel_output`, added with this counter. None
  of the boundaries is in a Tier-1 file.
- **The structure test** (host, in the pattern of `tests/fs_fault_production_clean.rs`) parses the serial
  sources and fails unless: every boundary in the list above adds to the counter exactly once per byte; no
  boundary calls another boundary or a public writer; every public writer reaches exactly one boundary per
  byte, directly or through wrappers that do not count; the drain paths call no boundary; `write_notices`
  passes the already-counted mark; and every port or UART write in the kernel is a boundary, a drain, or on
  the uncounted raw-site list.
- **`isolation/quiet-errors`**, on both architectures, makes 1,000 failing window calls on one thread and
  requires that thread's counter and the unattributed counter unchanged. Its twin arms `emit-diagnostic(path)`
  once for each path and requires the exact increase each time: a direct `serial_println!`; a `log::warn!` line,
  whose bytes the case computes from the logger's record format quoted in `docs/gui/breengel.md` with the
  control's fixed target and payload; each nested wrapper (`write_byte`, and on ARM64 `_log_print` and
  `emergency_print`); each direct writer (`write_bytes_atomic`, `try_print`, `write_str_bounded`); and a
  notice posted on the calling thread and written out by another (+32 to the poster, +0 to the writer).
  `emergency_print` adds 32 to the unattributed counter and nothing to the thread. While the twin runs, a
  second thread arms the same control on its own window, and its bytes appear only on its own counter.

### 12.2 Numerical contracts

Functional cases use deterministic counts: frames, epochs, operations, bytes. Real performance lives only in the
`performance` categories and in the few named cases below. Every bound uses `CLOCK_MONOTONIC`, holds on all four
targets, and is the contract. Changing a number takes an operator ruling recorded in this plan. A failing run
is fixed by making the system faster, never by relaxing the bound. Boots run under the existing shared-host
rules: `timingSensitive` records host load and does not wait for a quiet host (`docs/suites/README.md`).

| Case | Workload and warmup | Measured | Bound |
|---|---|---|---|
| `breengel/composition/present-rate` | one 256x256 window, 300 commits as fast as buffers release; first 30 discarded | commits composited per second | ≥ 60/s |
| `breengel/composition/flush-irq-bound` | full-screen damage, 20 composites | IRQ-masked maximum | ≤ 500 µs |
| `window-manager/performance/typing-latency` | idle desktop, one client; 110 keys at 50 ms; first 10 discarded | injection to the composite containing the processing commit | median ≤ 20 ms, max ≤ 50 ms |
| `window-manager/performance/pointer-latency` | as above with 110 absolute motions | as above | median ≤ 20 ms, max ≤ 50 ms |
| `window-manager/performance/latency-under-animation` | four clients committing as fast as released, typing as above | as above | median ≤ 33 ms, max ≤ 100 ms |
| `window-manager/performance/present-latency` | 330 commits of 256x256; first 30 discarded | commit to composite | median ≤ 16 ms, max ≤ 33 ms |
| `window-manager/performance/frame-floor` | one client animating as fast as released; 1 s warmup, 5 s window | composites per second | ≥ 60/s |
| `window-manager/performance/idle-cpu` | three static clients; 1 s settle, 5 s window | utime + stime of bwm and clients over the window | ≤ 2% of one CPU |
| `window-manager/performance/clipboard-latency` | 64 KiB set in one process, get in another, 20 times; first 2 discarded | set to completed get | max ≤ 50 ms |
| `ui-testing/sessions/start-desktop` | bwm plus one app | session start to the app's first composited commit | ≤ 300 ms |
| `ui-testing/sessions/start-kiosk` | `kiosk_test` plus one app | as above | ≤ 300 ms |
| `ui-testing/frames/compare-cost` | two 1280x800 frames differing in one pixel | one comparison | ≤ 50 ms |
| `gui-apps/bterm/throughput-time` | 1 MiB of numbered lines through the PTY | first byte to the commit with `pty-consumed` = 1 MiB | ≤ 8 s |
| `gui-apps/guskit-collab/large-sync` | joiner to a 1024x768 canvas | join to the joiner's commit of the synced canvas | ≤ 2 s |
| `gui-apps/desktop/idle-all-apps` | every app open and idle; 2 s settle, 5 s window | utime + stime of the session | ≤ 5% of one CPU |
| `window-manager/liveness/memory-bounded` | eight animating clients, 60 epochs warmup then 600 epochs | bwm VmRSS growth | ≤ 64 KiB, with caches at their documented capacity |
| `breengel/text/cache-bound` | 2,000 distinct glyph and size pairs | cache entries and bytes | ≤ 256 entries and ≤ 4 MiB |

**Session start is 300 ms.** The first design adopted "a session start (bwm plus one app at first composite) must be
under 300 ms on every target", and both session-start cases use that bound; kiosk is held to
it because it is a session start too. The case's outer time limit (the default 10 s) is a separate thing and does
not loosen the bound. Section 19 measures session start early. If that measurement shows 300 ms out of reach on
a target, the measurement goes to the operator as an evidence-backed request to correct the expectation, and the
case keeps failing at 300 ms until there is a ruling.

### 12.3 Functional limits and deadlines

Functional cases also depend on limits that are not performance numbers: how much work a parser may do, how
long a blocked call may take to notice an event, how the UI clock's timing rules are parameterized. Each one is
written here, is the contract, and changes only by an operator ruling recorded in this plan, like the bounds
above. Geometry constants (the cascade offset, the minimum titlebar strip, chrome rectangles) are literal
entries in `docs/gui/bwm.md`'s table, typed into the test crate by hand (section 16, foundation item 6).

**Real-time deadlines (`CLOCK_MONOTONIC`).**

| Limit | Value | Cases |
|---|---|---|
| a blocked call noticing a kill, a destroy or a compositor death ("promptly") | returns within 250 ms of the event (the `kill` return, the destroy return, or the compositor's exit as reaped) | `isolation/killable`, `composition/compositor-dies`, `recovery/blocked-acquire`, `blocked-wait` |
| a commit with no compositor | returns within 250 ms | `composition/no-compositor` |
| a timed wait | returns no earlier than its timeout and no later than the timeout plus 100 ms | `windows/acquire-timeout`, `events/wait-timeout`, `liveness/compositor-wait-timeout` |
| the step bound | 2 s for every required participant to acknowledge an epoch; past it, `step` fails naming the participant, and the inner executor classifies the scenario `app-hung` | `clock/stopped-unacked-fails`, `proof/fault-hang` |
| the settle bound | 2 s by default; a case may pass a longer bound, up to its remaining time less the collection reserve | `settle/timeout-names`, `input/undelivered-fails`, `proof/fault-slow-commit` |
| the collection reserve | 1.5 s (section 10.2) | `diagnosis/reserve` |

**UI-clock timing (deterministic, on the stepped clock).**

| Limit | Value | Cases |
|---|---|---|
| key repeat | first repeat 500 ms after the press, then one every 50 ms, inclusive; a key held for 600 ms gives exactly 3 repeats | `focus/repeat-*`, `input/hold-repeat` |
| double click and Super double tap | second press within 400 ms of the first, inclusive; a double click also needs both presses within 4 pixels of each other on each axis | `pointer/double-click-*`, `focus/super-double-tap`, `text-entry/filepicker-mouse`, `input/double-click` |
| the font watcher | checks `fonts.conf` and every configured font file every 250 ms of its clock, comparing inode number, size and modification time, so a replacement by rename (new inode) is always seen; a change made at time *t* is in use in the window's first commit for an epoch at or after the first check after *t* | `text/fontwatch-*`, `bfontpicker/live-*` |
| the WM's launch acknowledgment | a launched GUI program's first window within 5,000 ms of UI time; otherwise the error is shown in the first epoch at or after that | `blauncher/launch-no-window`, `lifecycle/launch-ack` |
| icon settling | *(checked: `libs/libicon/src/physics.rs:40`)* settled means offset below 0.1 and velocity below 0.1 per second; every animation in libicon's catalogue settles within 2,000 ms of UI time after its last input | `icons/settles` |
| `sin_approx` | absolute error at most 0.0012 against sin at 4,096 evenly spaced points of [−π, π] *(checked: the current approximation's error is 0.0011)* | `icons/sin-approx` |
| Bounce's speed | a factor from 1/4 to 4, halved or doubled by each key press or wheel step | `demos/bounce-speed-*` |

**Parser, decoder and raster work.** Each library keeps an always-on `work` counter that counts one step per
iteration of every loop, including iterations that reject or skip, and one step per call of every recursive
function, so a bound on steps also bounds work that writes nothing. A counted path never calls an uncounted
library sort or search: libfont sorts scanline crossings with its own bottom-up merge sort, which counts one step
per element moved. No loop is left out of the count to meet a bound. Allocations are counted by the library's
allocator wrapper in the test build of the case.

| Limit | Value | Cases |
|---|---|---|
| font load (table directory, cmap, hmtx, loca validation) | at most 16 × file bytes + 65,536 steps; at most 2 × file bytes + 64 KiB allocated | `font-parser/work-bound`, `alloc-bound`, `corpus` |
| one glyph's outline | compound depth at most 8; at most 256 components after expansion; at most 65,535 points and no more than maxp declares; at most 16 × the glyph data bytes visited + 4,096 steps; at most 64 bytes per point + 64 KiB allocated | `font-parser/compound-*`, `glyf-points`, `coords-extreme`, `work-bound` |
| one glyph's flattening into line segments | quadratic curves subdivide to depth at most 9, so at most 512 segments per curve; at most 65,536 segments per glyph, beyond which the glyph is refused with `GlyphTooComplex`; at most 1,028 × *P* steps, where *P* is the glyph's point count after compound expansion | `font-parser/coords-extreme`, `work-bound` |
| one glyph's rasterization at size *s* px | a bitmap at most 4*s* × 4*s*; at most *E* + *H* × (10*C* + 8*E* × (3 + ⌈log₂(*E* + 1)⌉) + 17) + 64 steps and at most *H* × *C* + 4*C* + 48*E* + 4,096 bytes allocated, where *H* is the bitmap's rows, *C* its sampled columns (its width, or three times its width for LCD subpixel text) and *E* its segments; a glyph whose step bound exceeds 2²⁸ (268,435,456) is refused with `GlyphTooExpensive` before any raster loop runs | `font-parser/coords-extreme`, `work-bound`, `text/size-range` |
| inflate | at most 4 × output bytes + 16 × input bytes + 1,024 per block steps; 32 KiB window plus at most 64 KiB of tables besides the caller's output | `inflate/output-limit`, `byte-split`, `png-decode/bomb` |
| image dimensions (BMP, PNG decode and encode) | width and height at most 16,384, and width × height at most 16,777,216 pixels, checked before any allocation | `bmp/huge`, `size-overflow`, `png-decode/huge`, `png-encode/limits` |
| PNG decompressed size | rejected at the first byte beyond rows × (1 + row bytes) for the image's own dimensions, per pass for Adam7 | `png-decode/bomb` |
| raster primitive at any coordinates | at most 2 × (clipped area + clipped rows + clipped columns) + 64 steps, where the clipped quantities are those of the primitive's clipped bounding rectangle; a primitive wholly outside the surface or clip does at most 64 | `raster/extreme-work-bound`, `guard-pages` |
| surfaces | width and height at most 16,384, stride at least width × bytes per pixel, stride × height at most 2³¹ − 1, base 4-byte aligned | `raster/surface-validate`, `stride-padding` |

**Where the rasterization bound comes from.** It describes the scanline rasterizer libfont has today
*(checked: `libs/libfont/src/rasterizer.rs`)*, counted honestly. For each of the *H* rows it clears *C*
accumulators, runs 8 sub-scanlines, and converts the accumulators to coverage (at most *C* more). Each
sub-scanline visits every one of the *E* segments for crossings, sorts the *k* ≤ *E* crossings (at most
*E* × ⌈log₂(*E* + 1)⌉ moves), walks them (at most *E*), and fills the filled spans pixel by pixel. Under the
non-zero rule those spans are disjoint, so their fill visits at most *C* pixels. One row therefore takes at most
1 + *C* + 8 × (*C* + *E* × (3 + ⌈log₂(*E* + 1)⌉) + 2) + *C* steps, which is the per-row term; the leading
*E* is the LCD path's one pass that scales the segments, and 64 covers setup. The bound grows with the output
area (the 10*C* per row) as well as with the edges, because the fill and the coverage passes really do touch
every pixel. A bound without that term cannot hold. The bound is computable from *H*, *C* and *E* before any
raster loop runs, which is what makes the 2²⁸ refusal possible. The limit is a safety ceiling, not a target. By
the bound's own arithmetic, a 72 px glyph of a few hundred segments comes to a few million steps, two orders of
magnitude below it. That is an estimate, and `text/size-range` checks it by the counter at the sizes the apps
and tests use. If the ceiling ever refuses a glyph an app needs, the remedy is a rasterizer with a
smaller bound (an active-edge list), adopted with its own bound by an operator ruling, never a larger ceiling
or an uncounted loop.

**The independent fixture.** `font-parser/work-bound` rasterizes a four-edge square 1,022 px on a side, made
at authoring time outside libfont, whose bitmap is 1,024 × 1,024 (the outline plus libfont's two-pixel margin).
With *H* = *C* = 1,024 and *E* = 4, the bound is 4 + 1,024 × (10,240 + 192 + 17) + 64 = **10,699,844** steps.
The case also requires **at least 8,388,608** steps (8 × 1,024 × 1,024). Computed from today's loops under the
counting rules above (not measured), the rasterizer takes about 10.5 million steps on this square, while an
accounting that leaves the span fill out comes to about 2.2 million, so the floor catches a counter that skips
the fill.

**Raw surfaces.** `Surface::new` allocates and owns its memory. `Surface::from_raw(base, width, height, stride,
format)` is `unsafe`, but it still checks every numeric rule in the table above and returns the same errors as
`new`. Its one unchecked obligation, stated in its documentation, is that `base` stays valid for reads and
writes of `stride × height` bytes, and is not accessed through another mutable path, for the surface's
lifetime. `raster/surface-validate` tests the numeric checks on both constructors, and the guard-page cases test
that no primitive writes outside a valid surface.

## 13. Host tools

Allowed, never in a verdict: `scripts/suite-verdict.py` (parsing guest records), `scripts/uitest-artifacts.py`
(viewer), `tools/gfx-refgen` (authoring-time goldens), `scripts/host-test-graphics.sh` (host unit tests of
libgfx, libfont, libimg, libbui, libicon, libpixcheck and breengel as the developer loop, wired into the host
test harness), repository structure tests, QMP or `prlctl` screenshots for people, and Vigil. Host capture and
host input are the fragile parts today (the Parallels input wedge, keys dropped while the host's screen is locked,
partial Parallels frames in #930, and no VMware path at all), and the plan depends on neither.

## 14. The four efforts

### 14.1 Summary

Appended after `sdk` in this order. Subsystem ids are Vigil's taxonomy: 17 Graphics & display, 18 Input & USB,
21 Kernel test framework & gates, 22 libbreenix, 24 Userspace programs, 25 GUI stack. None of the four has a
POSIX `area`, so the effort objects omit it. Each suite manifest's `area` is the effort's title (section 8.5).

| # | Effort | Suite | Subsystems | Milestones | Cases |
|---|---|---|---|---|---|
| 1 | `breengel`: BreenGL graphics library | `breengel` | 17, 22, 25 | 14 | 380 |
| 2 | `window-manager`: Window manager | `window-manager` | 17, 18, 25 | 13 | 249 |
| 3 | `ui-testing`: UI test framework | `ui-testing` | 18, 21, 25 | 9 | 206 |
| 4 | `gui-apps`: GUI applications (the destination) | `gui-apps` | 24, 25 | 10 | 307 |
| | **Total** | | | **46** | **1,142** |

The desktop's real startup is measured by suite cases, not by a `bootPath` milestone. The bootPath measure
counts only the full tests gate on x86-64, and that gate never runs `init desktop`, so a desktop boot-path
milestone could not pass on x86-64 without changing the measure's definition and Vigil's stage selection.
Instead:
- `window-manager/startup` starts bwm through the same desktop supervisor code that `init desktop` uses.
- `gui-apps/desktop/init-desktop` runs the real `/sbin/init desktop` binary as a child and requires bwm and a
  bterm prompt on screen. `init` runs desktop mode as an ordinary process when it is not PID 1, which it does
  not do today. As a child inside a suite boot it skips the PID-1-only test-control absence check and says so
  (section 3.4); this case proves the desktop's composition, never the PID-1 wiring.
- The desktop-mode boot-path stages are repaired anyway, so a desktop boot is honest health evidence. The
  vacuous `[bwm] Breenix Window Manager starting` marker is replaced by `[bwm] composited client frame`, which
  bwm prints only after a composite record lists a client commit. `[bterm] prompt composited` is printed only
  after bterm recognizes its shell's fixed prompt in a committed, composited frame. Desktop mode becomes a row
  in the clean-main matrix on all four targets, and that row also requires `[init] test controls absent`, the
  stage PID-1 init prints after it has verified that every test-control op returns ENOSYS (section 3.4). These
  stages are health signals, not effort measures; the production absence of test controls is checked by that
  row and by the production-profile boot gate, both on the real boot path.

### 14.2 Dependency map

```
breengel   library track:  raster ─ text ─ font-parser ─ inflate ─ images ─ widgets ─ text-entry ─ icons
           kernel track:   [entry spikes] ─ windows ─ isolation ─ composition ─ events ─ scanout ─ panel
                                 (library and kernel tracks proceed in parallel; widgets need text)
window-manager:            input-core ─ input-devices ─ startup ─ composition ─ stacking ─ focus ─ pointer
                           ─ geometry ─ lifecycle ─ recovery ─ clipboard ─ liveness ─ performance
ui-testing:                runner ─ input ─ tree ─ frames ─ clock ─ settle ─ sessions ─ diagnosis ─ proof
gui-apps:                  terminal-emulation ─ bterm ─ blauncher ─ bfontpicker ─ blog ─ guskit
                           ─ guskit-collab ─ bcheck ─ demos ─ desktop (integration last)
```

Each effort starts after the previous one's milestones pass on all four targets. Within an effort the order
above is the planned build order. A milestone counts as done when its cases pass on all four targets,
whatever order the work landed in.

### 14.3 Early cases and what they rely on

The first two suites are written with direct guest assertions, layer-specific acknowledgments and small test
clients, as the POSIX suites are. They do not use libuitest, which is effort three's product. They do follow
the GUI spawn rule from the start: every helper process is created through `libbreenix::suite::spawn`, which
`breengel` delivers with `Suite::gui()` (section 8.2), so when `ui-testing` adds the result pipe and its
validator, the earlier suites already satisfy it. This table settles every early case that the design made
depend on later infrastructure.

| Case or need | What it uses | Delivered by |
|---|---|---|
| `breengel/events/serial`, `processed-ack` | commit metadata's `processed_serial` written by Breengel; events written by `kiosk_test` | `breengel` (the commit protocol) |
| widget and icon animation | explicit time values passed to pure library functions (`icon.update(t_ms)`) | `breengel` |
| `breengel/text/fontwatch-idle` | `breengel::clock` provider plus a case-owned clock page stepped by the case | `breengel` (the provider) |
| screen observation in `breengel` and `window-manager` | `scanout_readback` with its observation record | `breengel` |
| display handoff between the panel and a case | `Suite::gui()` handoff only | `breengel` |
| helper processes and descriptor hygiene | `libbreenix::suite::spawn`; the validator rejects direct `fork` in GUI sources | `breengel` (with `Suite::gui()`) |
| the processed watermark and `Redraw`/`NoChange` dispatch | `window.dispatch` in Breengel (section 6.4) | `breengel` (the commit protocol) |
| biconkit's first frame, Rectangles' window path | moved to `gui-apps/demos` | `gui-apps` |
| a moving client for WM composition | `wm-client_test --animate` drawing its frame counter, stepped by commands from the case; Bounce is tested in `gui-apps/demos` | `window-manager` |
| exact repeat, double-click and double-tap timing in `window-manager` | the UI clock page (from `breengel`) stepped by the case; bwm publishes its acknowledged epoch in the compositor status page; uinput devices created with `time=injector` | `breengel` (clock page), `window-manager` (bwm's use and acknowledgment) |
| WM focus, stacking and routing truth | an operation model in the test crate, `wm-client_test` event logs in a shared ring, the registry, and screen readback | `window-manager` |
| scripted input, semantic trees, the general epoch-and-settle protocol, mandatory FRAME accounting, the inner executor, sessions, artifacts | libuitest and its runner integration | `ui-testing`, adopted by `gui-apps`. The earlier suites are not rewritten onto it. |

### 14.4 Sizing

These are estimates for sequencing, not ceilings: the operator's standing ruling forbids line or file budgets
on fixes. Sizes are S (one PR), M (two to four), L (five or more). Each milestone's verification is one suite
boot per target once its cases exist, plus the boots its PRs need under the small-PR ratchet.

| Effort | S | M | L |
|---|---|---|---|
| `breengel` | bmp, icons, panel | raster, text, font-parser, inflate, images, widgets, text-entry, events, scanout | windows, isolation, composition |
| `window-manager` | clipboard, liveness, performance | input-devices, startup, stacking, focus, pointer, geometry, lifecycle, recovery | input-core, composition |
| `ui-testing` | diagnosis | input, tree, frames, clock, settle, sessions, proof | runner |
| `gui-apps` | bfontpicker, demos | blauncher, blog, bcheck, guskit-collab, desktop | terminal-emulation, bterm, guskit |

## 15. Suite `breengel`

Manifest: id `breengel`, title "BreenGL graphics library", area "BreenGL graphics library",
`"timingSensitive": true`, binary `/sbin/suite-breengel`, source `suite_breengel.rs`. Summary: *"BreenGL draws
shapes, text, images and widgets exactly to its written contract and rejects malformed fonts and images within
bounded work, and windows created, committed, composed, observed on the display and destroyed through the
kernel's window interface behave the same, safely, on every target."*

| Milestone | Title | Categories |
|---|---|---|
| `raster` | shapes, blits, clipping & alpha | `raster` |
| `text` | TrueType & bitmap text | `text` |
| `font-parser` | font parsing safety | `font-parser` |
| `inflate` | clean-room inflate | `inflate` |
| `images` | BMP & PNG images | `bmp`, `png-decode`, `png-encode` |
| `widgets` | widgets | `widgets` |
| `text-entry` | text fields, focus & file picker | `text-entry` |
| `icons` | animated icons | `icons` |
| `windows` | window lifecycle on every target | `windows` |
| `isolation` | window authority & EFAULT safety | `isolation` |
| `composition` | commit, composition & present | `composition` |
| `events` | window input events | `events` |
| `scanout` | display observation on every target | `scanout` |
| `panel` | suite & probe panels | `panel` |

**Foundation work, in order:**
1. The entry spikes (section 4.2).
2. `docs/gui/breengel.md`: pixel formats, rounding, alpha, the dirty and damage contract, clipping (half-open
   rectangles, i32 coordinates that clip and never wrap), the clip stack, text rules and non-goals, the commit
   protocol, the authority table, epochs, the observation contract, the event ABI v2 and every op's errors.
3. Kernel: the arch-neutral window interface with the commit protocol and authority checks; the software
   compositor on virtio-gpu 2D, the GOP framebuffer and the SVGA3 screen target; VirGL texture slots by
   allocation in place of `id % 8`; `scanout_readback`; the mode set to 1280x800 on every backend in GUI suite
   boots; damaged-band copies for `copy_to_display` (#1299); per-buffer bases, the damage log, replay sets
   copied inside `acquire` under a replay pin, the whole-window fallback for stale commits, `capture_window`'s
   tree destination, remnants and evidence holders (section 3.1); suite-mode test controls, including
   `acquire-stall` and `emit-diagnostic(path)`; the counters in section 12.1, including per-thread kernel output
   accounting at one boundary per byte, with its structure test. Making the syscalls arch-neutral may touch
   x86 dispatch in `kernel/src/syscall/handler.rs`, a Tier-1 file. Under the Tier-1 policy that edit is minimal, committed alone, explained in the PR and landed with
   boot evidence.
4. Libraries: libgfx gets surfaces that own their memory with validated constructors, premultiplied alpha,
   the clip stack, blits, rounded rectangles and gradients, and fixes for the `fill_rect`, `fill_circle` and
   `copy_row_from` defects. The kernel's `primitives.rs` drawing is replaced by libgfx (it is `no_std`), so one
   tested raster serves the kernel screens and the libraries. libfont moves to 26.6 fixed point if
   `text/raster-identical` cannot hold with f32. libimg gets the inflater, the PNG decoder and encoder, and the
   BMP overflow fixes. libbui gets a text field, focus traversal and a keyboard-driven file picker. libicon
   takes explicit time and integrates in 1 ms quanta. Breengel gets `Drop`, the commit protocol (using the
   buffer `acquire` returns already replayed; Breengel copies nothing), `window.dispatch` with the processed
   watermark (section 6.4), event ABI v2, honest resize errors, `breengel::clock` and the FontWatcher fix with
   its 250 ms check (section 12.3). libgfx, libfont and libimg get their always-on `work` counters (section
   12.3); libfont also gets its counted merge sort for crossings, the flattening limits and the
   `GlyphTooComplex` and `GlyphTooExpensive` refusals.
5. `libs/libpixcheck` (`no_std` + alloc): exact, tolerance, region and mask comparison; structural
   predicates; first-difference reports; SHA-256 of canonical frames, with the FIPS 180-4 known-answer vectors
   as its host and guest tests.
6. `kiosk_test`, a test compositor that places windows where it is told and composes on commit, and
   `raw-window_test`, a client that uses the raw libbreenix calls without Breengel.
7. `Suite::gui()` display handoff (section 8.1).
8. Host unit tests for every graphics crate (`scripts/host-test-graphics.sh`), sharing fixtures with the guest
   cases. They are the developer loop. The milestones are measured in the guest.

Surface cases draw into surfaces bracketed by `PROT_NONE` guard pages, and padded rows end in sentinel bytes
that every case checks.

#### `raster` — shapes, blits, clipping & alpha (40)
- `fill-rect` — fill_rect writes exactly the pixels of a half-open rectangle and no others.
- `fill-rect-clip-left` — A rectangle partly off the left edge fills only its visible columns.
- `fill-rect-offscreen-left` — A rectangle wholly left of the surface changes no pixel.
- `fill-rect-offscreen-top` — A rectangle wholly above the surface changes no pixel.
- `fill-rect-overflow` — A rectangle whose x + w overflows i32 clips to the surface without wrapping.
- `fill-rect-empty` — Zero or negative width or height draws nothing.
- `draw-rect` — draw_rect writes exactly the one-pixel perimeter.
- `line-axes` — Horizontal and vertical lines cover exactly their spans, endpoints included.
- `line-bresenham` — Shallow, steep and 45-degree lines write exactly the pixels in the contract's literal tables.
- `line-clip` — A line crossing the edges writes only the in-surface pixels of its unclipped path.
- `circle-fill` — fill_circle sets exactly the pixels the contract's x² + y² ≤ r² rule selects.
- `circle-fill-offscreen-left` — A circle partly or wholly off the left edge writes nothing outside the surface.
- `circle-outline` — draw_circle writes the midpoint-algorithm pixels listed in the contract for radii 0 to 40.
- `guard-pages` — Every primitive called with extreme coordinates leaves the guard pages untouched and the process alive.
- `extreme-work-bound` — Every primitive at coordinates near ±2³¹ takes no more loop steps than section 12.3's bound for its clipped rectangle, by libgfx's work counter, which counts rejected iterations too.
- `stride-padding` — Drawing into a surface whose stride exceeds width × bytes per pixel leaves every row's sentinel bytes intact.
- `surface-validate` — `Surface::new` and `Surface::from_raw` both reject a short stride, an overflowing size, an oversize dimension, a misaligned or null base with section 12.3's errors.
- `bpp3-bpp4` — The same scene on a 3-byte RGB surface and a 4-byte BGRX surface gives the same colours.
- `bgr-order` — The BGR flag swaps the channel order and nothing else.
- `x-byte` — Every primitive writes the X byte of a BGRX pixel as 0xFF.
- `copy-row-clip` — copy_row_from clips at the end of the row instead of wrapping into the next one.
- `premultiply` — Straight colour and alpha premultiply to div255(c × a) for all 65,536 pairs.
- `alpha-over-examples` — Source-over gives the contract's literal results for fractional source and destination alphas.
- `alpha-over-sweep` — Source-over matches the premultiplied formula for all 256 alphas over four backgrounds.
- `alpha-extremes` — Alpha 255 replaces the destination and alpha 0 leaves it unchanged.
- `alpha-output` — The output alpha over a translucent destination follows the contract's formula.
- `blit` — blit copies a source surface to a position exactly.
- `blit-clip` — A blit partly off the surface copies only the visible intersection.
- `blit-alpha` — A premultiplied blit blends each pixel by the formula.
- `blit-straight` — A straight-alpha source is premultiplied exactly once before blending.
- `blit-scale` — An integer-scale blit replicates each source pixel into an exact square.
- `clip-rect` — With a clip rectangle set, every primitive draws only inside it.
- `clip-stack` — Nested clips intersect, and popping one restores the outer clip.
- `clip-empty` — An empty clip draws nothing and marks no damage.
- `rounded-rect` — Rounded-rectangle corners match the contract's corner predicate.
- `gradient` — A vertical gradient's rows equal the contract's integer interpolation.
- `damage-exact` — Each primitive marks exactly the clipped bounding rectangle the contract's table gives.
- `damage-noop` — A draw clipped to nothing marks nothing, and a same-colour draw still marks its rectangle.
- `damage-sound` — Over a seeded random scene, every changed pixel lies inside the accumulated damage.
- `surface-owned` — An owned surface allocates, clears to a colour and frees its memory when dropped.

#### `text` — TrueType & bitmap text (37)
- `fonts-pinned` — The test fonts parse, and their SHA-256 values match the pins.
- `raster-identical` — A fixed set of rasterized glyphs hashes to the pinned value, the same on every target.
- `metrics-independent` — Advance widths, left bearings and bounding boxes of 50 glyphs equal the independently extracted table.
- `cmap-independent` — Codepoint-to-glyph mappings for 200 codepoints equal the independently extracted table.
- `bitmap-glyph` — The built-in 5x7 font draws 'A' exactly as its table defines it.
- `bitmap-scale` — Scale 3 turns each glyph pixel into a 3x3 block.
- `bitmap-unknown` — A byte without a glyph draws the contract's replacement box.
- `noto-ascii` — noto-sans-mono-bitmap draws every printable ASCII character matching its reference cell.
- `noto-coverage` — Bitmap coverage blends with the background by the alpha contract.
- `ttf-glyph` — DejaVu Sans Mono "Hg" at 16 px matches its reviewed reference.
- `ttf-sizes` — One string at 8, 12, 16, 24 and 48 px matches each size's reviewed reference.
- `ttf-advance` — A 200-character line ends where the literal sum of 26.6 advances says, rounded once.
- `ttf-kerning` — "AV" in DejaVu Sans is narrower than "A" plus "V" by the kern table's literal value.
- `ttf-width` — text_width equals the distance from the pen start to the end of the last advance.
- `ttf-clip-left` — Text starting at negative x draws its visible part.
- `ttf-clip-top` — Glyphs partly above row 0 draw their visible part.
- `ttf-subpixel` — LCD subpixel text matches its reviewed reference per channel.
- `ttf-notdef` — A codepoint the font lacks draws glyph 0.
- `utf8-text` — UTF-8 input draws "é" from the font's glyph for U+00E9.
- `utf8-invalid` — Invalid UTF-8 draws one U+FFFD per maximal invalid subpart and does not panic.
- `compound-glyph` — A compound glyph places its components at the literal offsets.
- `compound-transform` — Scaled and 2x2-transformed components match their reviewed reference.
- `line-metrics` — Ascent, descent and line gap equal the font's hhea values at each size.
- `size-range` — Sizes from 6 to 72 px render, and size 0 or above 1,024 px is rejected; at 6, 12, 24, 48 and 72 px every printable ASCII glyph of the pinned fonts, grayscale and LCD, stays within section 12.3's flattening and rasterization bounds by the work counter.
- `many-args-abi` — The text calls with ten or more parameters, including trailing f32 or 26.6 values, give the reference output on this target.
- `cache-hit` — A cached glyph renders identically to a fresh one, and the cache reports a hit.
- `cache-bound` — 2,000 distinct glyph and size pairs keep the cache within 256 entries and 4 MiB.
- `cache-evict-correct` — A glyph evicted and rasterized again renders identically.
- `fontwatch-reload` — Changing the configured font makes later text use newly rasterized glyphs of the new font.
- `fontwatch-size` — Changing the configured size changes the rasterized glyphs and their metrics.
- `fontwatch-missing` — A configured font file that is missing keeps the previous font and reports the error.
- `fontwatch-malformed-config` — A malformed fonts.conf keeps the previous fonts and reports the error.
- `fontwatch-partial` — A fonts.conf replaced by rename is seen whole, and a truncated one is rejected.
- `fontwatch-repeated` — Five changes in a row end with the last one in use.
- `fontwatch-independent` — Mono and display fonts change independently.
- `fontwatch-idle` — A window that receives no events, with fonts.conf changed at UI time *t*, draws the new font in its first commit at or after the watcher's first 250 ms check after *t*.
- `fontwatch-file-replaced` — Replacing the configured font file at the same path by rename, with fonts.conf unchanged, makes later text use newly rasterized glyphs of the new file by the next check.

#### `font-parser` — font parsing safety (22)
- `truncated` — A truncated TTF fails to parse with an error instead of panicking.
- `table-offsets` — Table offsets or lengths past the end of the file are rejected.
- `loca-range` — A loca entry past the glyf table is rejected for that glyph only.
- `loca-order` — A loca table that is not monotonic is rejected.
- `numglyphs` — A glyph count that disagrees between maxp and loca is rejected.
- `cmap4-segments` — A format 4 cmap whose segCountX2 is odd, or whose end codes are not sorted, is rejected.
- `cmap4-odd-segments` — A valid format 4 cmap with an odd number of segments (39, as in the OpenType specification's example) maps 200 codepoints as the independently extracted table says.
- `cmap4-range-offset` — A format 4 idRangeOffset that points outside the table is rejected.
- `cmap12-groups` — Overlapping or unsorted format 12 groups are rejected.
- `glyf-endpoints` — Contour end points that do not increase are rejected.
- `glyf-points` — A point count above maxp's maximum is rejected.
- `glyf-flags` — A flag repeat that runs past the glyph's data is rejected.
- `compound-depth` — A compound nested deeper than the limit is rejected for that glyph.
- `compound-cycle` — A compound glyph that refers to itself, directly or through another, is rejected within bounded work.
- `compound-missing` — A component index past the glyph count is rejected.
- `hmtx-compressed` — A valid hmtx with fewer long metrics than glyphs gives the remaining glyphs the last advance and their own left side bearings.
- `hmtx-truncated` — An hmtx shorter than 4 × numberOfHMetrics + 2 × (numGlyphs − numberOfHMetrics) bytes, or an hhea numberOfHMetrics of 0 or above the glyph count, is rejected.
- `units-per-em` — unitsPerEm of 0 or above 16,384 is rejected.
- `coords-extreme` — Glyphs with coordinates at the i16 limits either flatten and rasterize within section 12.3's bounds by the work counter, or are refused with `GlyphTooComplex` or `GlyphTooExpensive` before any raster loop runs, never after it.
- `work-bound` — Every corpus file loads, or fails to, within section 12.3's step bounds for a load and for each glyph's outline, flattening and rasterization; and the independent 1,024 × 1,024 square fixture rasterizes in at least 8,388,608 and at most 10,699,844 steps.
- `alloc-bound` — No corpus file makes the parser allocate more than section 12.3's limits.
- `corpus` — Each of the 64 malformed corpus files yields its documented error, never a panic.

#### `inflate` — clean-room inflate (21)
- `stored` — A stored block decodes to its bytes.
- `stored-length` — A stored block whose LEN and NLEN disagree is rejected.
- `fixed-huffman` — A fixed-Huffman block decodes to the literal expected bytes.
- `dynamic-huffman` — A dynamic-Huffman block decodes to the literal expected bytes.
- `block-sequence` — Stored, fixed and dynamic blocks in one stream decode, ending at the final block.
- `block-type-invalid` — Block type 3 is rejected.
- `code-oversubscribed` — An over-subscribed code-length set is rejected.
- `code-incomplete` — An incomplete code-length code is rejected, and so is an incomplete literal/length or distance code other than a single one-bit code.
- `code-single` — A literal/length code holding only end-of-block (code 256, one bit) decodes, and a distance code of one one-bit code decodes; the recheck's empty dynamic block decodes to empty output, as the independent decoder recorded.
- `no-distance-codes` — A block whose distance code lengths are all zero (RFC 1951 §3.2.7: no distance codes used) decodes its literals, and a length symbol in that block is rejected.
- `repeat-first` — A repeat code with no previous length is rejected.
- `symbols-invalid` — Literal/length symbols 286 and 287 and distance symbols 30 and 31 are rejected.
- `distance-too-far` — A distance reaching before the start of the output is rejected.
- `distance-max` — A distance of 32,768 within the window decodes.
- `length-258` — A match of length 258 decodes.
- `truncated` — A stream cut off at any byte is rejected.
- `zlib-header` — A bad check value, a preset dictionary, a method other than 8 and a window above 32 KiB are each rejected.
- `adler32` — A stream whose Adler-32 is wrong is rejected.
- `output-limit` — Output past the caller's limit is rejected without writing past it.
- `byte-split` — A stream fed in pieces split at every byte position decodes identically.
- `independent-fixtures` — Twelve fixtures made by an independent encoder at authoring time decode to their recorded bytes.

#### `bmp` — BMP images (9)
- `decode-fixtures` — Independent BMP fixtures decode to their literal pixels.
- `orientation` — Bottom-up and top-down (negative height) BMPs decode to the same pixels.
- `padding` — Widths 1 to 4 decode correctly across row padding.
- `encode-bytes` — encode_bmp_24 writes the literal expected header and rows.
- `roundtrip` — Encoding then decoding returns the original pixels.
- `truncated` — A BMP shorter than its header claims is rejected.
- `size-overflow` — Dimensions whose row or image size overflows are rejected before any allocation.
- `huge` — Dimensions beyond section 12.3's limits are rejected before any allocation.
- `unsupported` — An unsupported depth or compression is rejected with its error.

#### `png-decode` — PNG decoding (31)
- `gray-depths` — Grayscale at depths 1, 2, 4, 8 and 16 decodes exactly.
- `gray-alpha` — Grayscale with alpha at depths 8 and 16 decodes exactly.
- `rgb` — RGB at depths 8 and 16 decodes exactly.
- `rgba` — RGBA at depths 8 and 16 decodes to premultiplied pixels exactly.
- `palette-depths` — Palette images at depths 1, 2, 4 and 8 decode exactly.
- `trns-gray` — A grayscale tRNS key colour becomes transparent.
- `trns-rgb` — An RGB tRNS key colour becomes transparent.
- `trns-palette` — Palette tRNS alphas apply per entry.
- `invalid-combos` — Every illegal colour type and depth pair is rejected with its error.
- `rounding-16bit` — 16-bit samples convert to 8 bits by the contract's formula.
- `filters` — Rows using each of the five filter types decode exactly, at every bytes-per-pixel width.
- `filter-invalid` — A filter type above 4 is rejected.
- `interlace` — An Adam7 image decodes to the same pixels as its non-interlaced twin.
- `interlace-small` — Images from 1x1 to 9x9, where some passes are empty, decode correctly.
- `multi-idat` — Image data split across many IDAT chunks, including empty ones, decodes identically.
- `crc` — A chunk with a bad CRC is rejected.
- `chunk-order` — IDAT before IHDR, PLTE after IDAT and a missing IEND are each rejected.
- `plte-required` — A palette image without PLTE is rejected.
- `plte-size` — A PLTE whose length is not a multiple of 3 or exceeds the depth's entries is rejected.
- `palette-index` — A pixel index past the palette's end is rejected.
- `critical-unknown` — An unknown critical chunk is rejected.
- `ancillary-ignored` — gAMA, iCCP, sRGB and tEXt are ignored, as the contract states (no colour management).
- `ihdr-invalid` — Zero width or height and invalid compression, filter or interlace methods are rejected.
- `chunk-length` — A chunk length above 2³¹ - 1 is rejected.
- `huge` — Dimensions beyond section 12.3's limits are rejected before inflating.
- `bomb` — Data that inflates past the image's expected size is rejected.
- `truncated` — A PNG cut off inside IDAT is rejected.
- `signature` — A bad signature is rejected.
- `independent-fixtures` — The public PngSuite fixtures decode to the pixels an independent decoder recorded at authoring time.
- `blit` — A decoded RGBA image blitted into a surface gives the literal blended values.
- `from-disk` — A pinned PNG read from the ext2 disk decodes to its recorded pixels.

#### `png-encode` — PNG encoding (7)
- `decodes` — An encoded frame decodes to the same pixels.
- `stable` — Encoding the same frame twice gives identical bytes.
- `format` — The output is the documented format: RGB or RGBA at depth 8, filter 0, stored or fixed-Huffman blocks.
- `chunk-crc` — Every chunk's CRC matches the literal table's value.
- `zlib-adler` — The zlib stream's Adler-32 is correct.
- `sizes` — Frames of 1x1, odd widths and 1280x800 encode and decode.
- `limits` — A frame beyond the dimension limit is refused.

#### `widgets` — widgets (23)
All in-process: a widget is driven with explicit `InputState` values and timestamps and drawn into a surface.
- `button-draw` — A button draws its label and frame in the theme's literal colours and positions.
- `button-click` — Press and release inside a button report exactly one click.
- `button-cancel` — Press inside and release outside report no click.
- `button-disabled` — A disabled button draws dimmed and reports no click.
- `checkbox-toggle` — Clicking a checkbox toggles it and redraws the mark.
- `checkbox-space` — Space toggles a focused checkbox.
- `slider-drag` — Dragging a slider sets the value the contract maps from the pointer x.
- `slider-clamp` — Dragging past either end clamps to the minimum or maximum.
- `slider-keys` — Arrow keys step a focused slider by its step size.
- `label-clip` — A label wider than its bounds is clipped at the bounds with an ellipsis.
- `tabbar-select` — Clicking a tab selects it and reports its index.
- `tabbar-overflow` — More tabs than fit scroll, and every tab stays reachable.
- `scrollbar-drag` — Dragging the thumb sets the offset proportionally.
- `scrollbar-wheel` — A wheel step moves the offset by the configured amount.
- `scrollbar-thumb` — The thumb's length is proportional to visible over total.
- `menubar-open` — Clicking a menu title opens its menu below it.
- `menubar-shortcut` — match_shortcut returns the item bound to Ctrl+S.
- `menu-keys` — Arrows, Enter and Escape navigate, choose and close an open menu.
- `menu-disabled` — A disabled item draws dimmed and cannot be chosen.
- `menu-dismiss` — A click outside an open menu closes it without choosing.
- `layout-stack` — h_stack and v_stack place children at the contract's literal positions.
- `focus-ring` — Each focusable widget draws the theme's focus ring when focused and none otherwise.
- `no-change-ack` — An event that changes no widget state is reported as needing no redraw.

#### `text-entry` — text fields, focus & file picker (19)
- `insert` — Typed text is inserted at the cursor.
- `delete` — Backspace and Delete remove the character before and after the cursor.
- `move` — Left, Right, Home and End move the cursor as the contract says.
- `select` — Shift with an arrow selects, and typing replaces the selection.
- `utf8` — Multi-byte characters insert and delete as one character.
- `maxlen` — Input beyond the maximum length is refused.
- `scroll-to-cursor` — Text wider than the field scrolls to keep the cursor visible.
- `selection-text` — The selected text is returned exactly for a copy.
- `paste-insert` — Inserted paste text replaces the selection and moves the cursor past it.
- `focus-tab` — Tab moves focus through focusable widgets in order.
- `focus-shift-tab` — Shift+Tab moves focus in reverse.
- `focus-skip-disabled` — Focus traversal skips disabled widgets.
- `focus-click` — Clicking a focusable widget focuses it.
- `filepicker-list` — The file picker lists the given entries, directories first, in byte order.
- `filepicker-keys` — Arrows move the selection and Enter opens it.
- `filepicker-path` — Typing a path into the picker's text field navigates to it.
- `filepicker-mouse` — A click selects and a double click within the interval, by explicit timestamps, opens.
- `filepicker-scroll` — Lists longer than the view scroll by keys and wheel.
- `filepicker-errors` — A directory that cannot be read shows its error in the picker.

#### `icons` — animated icons (12)
All with explicit time values, integrated in 1 ms quanta.
- `states` — An icon goes idle, hover, pressed and released on the corresponding pointer edges.
- `spring-steps` — Spring positions after fixed time steps equal the contract's literal table.
- `step-invariance` — One step of 1,000 ms, a thousand steps of 1 ms and steps of 16 ms give identical states at equal times.
- `settles` — Every catalogue animation settles within 2,000 ms of UI time after its last input and never draws outside its box.
- `interrupt` — A hover interrupted by a press mid-animation follows the contract's state table.
- `reversal` — Leaving mid-hover reverses from the current position without a jump.
- `reset` — Reset after a resize returns the icon to idle at its new geometry.
- `bounded` — Over 10,000 steps of seeded random pointer input, the trajectory stays inside its box.
- `easing` — Each easing function matches the literal table at its sample points.
- `sin-approx` — sin_approx stays within 0.0012 of sin at 4,096 evenly spaced points of [−π, π].
- `particles-bounded` — Particle counts stay at or below the cap, and positions stay inside the box.
- `assets` — Each icon in each state draws to its reviewed reference.

#### `windows` — window lifecycle on every target (39)
Cases run against `kiosk_test` and read the registry.
- `create` — create_window returns a window id and buffers of the requested size.
- `create-zeroed` — A new window's buffers are all zero.
- `register-listed` — A registered window is listed with its title, size and owner.
- `title-limit` — A 64-byte title is kept whole, and a longer one is refused with EINVAL.
- `title-utf8` — A UTF-8 title is stored and listed byte for byte.
- `title-change` — Setting a new title updates the listing.
- `size-zero` — A zero width or height fails with EINVAL.
- `size-too-big` — A dimension over 4,096 fails with EINVAL.
- `limit` — The seventeenth window fails with the contract's errno, and the first sixteen are unaffected.
- `multi-window` — One process creates and draws four windows independently.
- `destroy` — The destroy op removes the window from the listing and unmaps its buffers.
- `drop-destroys` — Dropping a Breengel Window destroys it.
- `stale-id` — Every op on a destroyed window's id fails with ENOENT, and ids are never reused in a boot.
- `register-unwinds` — When registration fails, Window::new frees the buffers it created.
- `exit-cleanup` — A process's windows disappear when it exits.
- `exec-cleanup` — A process's windows disappear when it execs.
- `fork-no-mapping` — A forked child inherits no mapping of its parent's buffers.
- `fork-no-authority` — A forked child cannot commit, resize or destroy its parent's window.
- `commit-swaps` — After a commit, acquire returns a different buffer holding the committed pixels.
- `acquire-replays-damage` — With three buffers and alternating disjoint updates (A commits `[1,0]`, B commits `[1,2]` with only the second cell damaged), the next acquired buffer holds `[1,2]`, and every acquire over 300 such commits equals the newest front.
- `acquire-full-copy` — A buffer with no base, or whose missing commits have left the 8-entry damage log, is replayed in full by the kernel and equals the newest front, for a Breengel client and a raw client alike.
- `stale-base-commit` — Of two concurrently acquired buffers, committing the one with the older base gives exactly that buffer's pixels, with damage widened to cover every pixel that changed. With `acquire-stall` holding thread 1's replay copy from front A halfway, thread 2 commits C and then acquires: it does not get A until the copy ends, and thread 1's buffer equals exactly the commit its returned base names. A stale buffer committed after more than eight intervening commits gets whole-window damage. Every step compares whole pixels to the formula and checks that the damage covers every changed pixel.
- `commit-immutable` — Writing a committed buffer, or its tree page at any tree size, faults.
- `commit-busy` — Committing a buffer twice without acquiring it again fails with EBUSY.
- `thread-commit` — Two threads of the owner committing concurrently both succeed with distinct, ordered commit ids.
- `release-order` — A retired buffer returns to free only after its composite and any capture copy in progress are done.
- `acquire-timeout` — acquire with no free buffer returns ETIMEDOUT no earlier than its timeout and within 100 ms after it.
- `resize` — The owner's resize gives buffers of the new size holding the old contents in the intersection, also while another thread of the owner resizes the window.
- `resize-error` — A resize the kernel refuses is reported as an error, and no Resized event claims the new size.
- `resize-rlimit` — A resize over RLIMIT_AS fails with ENOMEM and leaves the old buffers intact.
- `resize-rollback` — A failure at each allocation step of create and resize leaves registry, mappings and accounting unchanged.
- `rlimit-accounting` — Window buffers count against RLIMIT_AS, and destroying the window returns them.
- `capture-across-destroy` — A capture in progress when its window is destroyed or resized returns the whole commit, with the tree page of that same commit when a tree destination is given; a capture begun after the destroy is ENOENT; and the buffer is freed when the copy ends.
- `two-buffer-capture-load` — A two-buffer client committing 300 frames while a reader captures it continuously and an evidence holder is registered never waits in acquire beyond one composite and one capture copy.
- `remnant-on-death` — When the owner of a window with an evidence holder is killed, the holder reads its last front and tree, and releasing the remnant returns memory to the baseline.
- `churn` — 100 create and destroy cycles return the registry, allocation counter and memory to the baseline.
- `distinct-content` — Sixteen live windows each composite their own pixels, never another window's.
- `raw-client` — A client using the raw libbreenix calls, without Breengel, creates, commits and destroys a window.
- `every-target` — Window creation works on this target's backend and reports the backend by name.

#### `isolation` — window authority & EFAULT safety (38)
Each denial follows a positive control in the same case: the authorized principal's same call succeeds.
- `other-register` — An unrelated process registering another's window fails with EPERM.
- `other-commit` — An unrelated process committing another's window fails with EPERM.
- `other-position` — A non-compositor setting a window's position fails with EPERM.
- `other-inject` — A non-compositor writing events into a window fails with EPERM.
- `other-read` — An unrelated non-root process capturing another's window fails with EPERM, and root succeeds.
- `other-map` — An unrelated process mapping another's buffers fails with EPERM.
- `other-resize` — An unrelated process resizing another's window fails with EPERM.
- `other-destroy` — An unrelated process destroying another's window fails with EPERM.
- `other-tree` — An unrelated non-root process mapping another's tree page fails with EPERM.
- `texture-map` — A non-compositor mapping the compositor texture fails with EPERM.
- `composite-denied` — A non-compositor compositing fails with EPERM.
- `claim-unprivileged` — A uid 1000 process claiming the compositor fails with EPERM, and root's claim succeeds.
- `claim-twice` — A second compositor claim fails with EBUSY while the first lives.
- `claim-after-death` — The compositor role can be claimed again once its holder dies, with a higher epoch.
- `compositor-authorized` — The compositor's position, inject, capture and composite calls succeed on every window.
- `pid-reuse` — A process that reuses a dead owner's PID number gains no authority over any window.
- `efault-create` — An unmapped output pointer for create fails with EFAULT and creates nothing.
- `efault-register` — An unmapped title pointer fails with EFAULT.
- `efault-list` — An unmapped list buffer fails with EFAULT.
- `efault-capture` — An unmapped capture destination fails with EFAULT.
- `efault-events` — An unmapped event-read buffer, or an argument structure made read-only while the read blocks, fails with EFAULT and consumes no event.
- `efault-inject` — An unmapped event pointer given to the compositor's write fails with EFAULT.
- `efault-composite` — An unmapped composite descriptor fails with EFAULT.
- `efault-mouse` — An unmapped pointer-position buffer fails with EFAULT.
- `efault-commit` — An unmapped commit metadata pointer fails with EFAULT, metadata with a nonzero reserved field fails with EINVAL, and neither commits anything.
- `efault-readback` — An unmapped readback destination fails with EFAULT.
- `efault-claim` — An unmapped claim argument fails with EFAULT and claims nothing.
- `efault-cross-page` — A buffer that runs from a mapped page into an unmapped one fails with EFAULT and has no side effect.
- `efault-overflow` — A pointer whose range overflows the address space fails with EFAULT.
- `efault-readonly` — A read-only output buffer fails with EFAULT and is left unchanged.
- `cow-output` — Output written into a copy-on-write page after fork lands only in the caller's copy.
- `destroy-during-commit` — Destroying a window while another thread commits it ends with one outcome and no fault.
- `test-hooks-absent` — The wait-stress ops fail with ENOSYS in the production kernel.
- `retired-ops` — The retired demo ops fail with ENOSYS.
- `unknown-op` — An unknown op fails with EINVAL.
- `killable` — SIGKILL ends a process blocked in acquire or wait_event within 250 ms.
- `quiet-errors` — 1,000 failing window calls on one thread add nothing to that thread's kernel-output counter or to the unattributed counter; with `emit-diagnostic(path)` armed, each writer path of section 12.1 (direct writers, nested wrappers, the logger, a notice written out by another thread, and `emergency_print` into the unattributed counter) adds exactly its bytes, once, to the right counter, and a second emitting thread moves only its own.
- `controls-latched` — In this suite boot the test controls are enabled because PID 1 is a suite binary, no op can clear or set the latch, and a child running `/sbin/init desktop` reports it is not PID 1 and makes no absence claim.

#### `composition` — commit, composition & present (27)
Cases run with `kiosk_test`. Expected screens are formulas: each test client fills its window with a pattern
derived from its id.
- `compose-position` — A committed pattern appears at the window's position in the composed frame, exactly.
- `compose-overlap` — Two overlapping windows compose in z order, exactly.
- `compose-offscreen` — A window partly off the screen is clipped at the screen edge.
- `compose-fullscreen` — A 1280x800 window at 0,0 fills the frame exactly.
- `compose-many` — Sixteen windows in a fixed stack compose to the formula, exactly.
- `generation` — Each commit advances the window's commit id by exactly one.
- `no-compositor` — A commit with no compositor returns within 250 ms and is composed once a compositor claims.
- `compositor-dies` — An acquire blocked when the compositor exits, and a blocking event read on another window, return within 250 ms, and the next commit succeeds.
- `eintr` — A signal interrupts a blocked acquire with EINTR.
- `damage-metadata` — A commit's damage rectangles are recorded exactly.
- `damage-work` — A composite processes exactly the pixels in its damage union, by the compositor's counter, and with `ignore-damage` armed the excess is reported by count.
- `damage-screen` — A partial composite gives the same frame as a full recomposition.
- `composite-record` — Each composite publishes its serial and the commit id and pixel generation of every presentation-required window it accounts for, a wholly occluded one included with zero composed pixels.
- `backend-error` — A forced backend failure leaves the commit uncomposited, is reported to the compositor, and the retry succeeds.
- `inflight-bound` — At most one composite per window is in flight.
- `no-change-commit` — commit_no_change publishes new metadata with empty damage and the same pixel generation, causes no composite, and is presented by the composite that already holds that generation.
- `cursor-hidden` — With the cursor hidden, no cursor pixel appears in the frame.
- `cursor-no-trail` — Moving the cursor over a static scene restores every pixel it covered.
- `cursor-shape` — Setting a cursor shape draws that shape at the pointer.
- `cursor-stationary-repaint` — Content committed under a stationary cursor is shown, with the cursor redrawn above it.
- `flush-irq-bound` — The longest IRQ-masked interval during full-screen damage stays within 500 µs, after `irq-hold` has shown the gauge reads a 2 ms hold.
- `present-rate` — One 256x256 window composes at least 60 commits a second.
- `takeover-giveback` — Display takeover and giveback restore the previous owner's frame exactly.
- `ownership-stack` — When a compositor dies, the display returns to the process that gave it, not to the runner.
- `fbinfo-geometry` — fbinfo's stride, bytes per pixel and pixel format agree with the mode.
- `fbmmap-pane` — fb_mmap gives the display owner the whole screen.
- `flush-clip` — FLUSH clips its rectangle to the screen.

#### `events` — window input events (32)
Events are written by `kiosk_test` through the compositor-only op and read through Breengel.
- `key-press-release` — A key press and its release arrive with the same keycode.
- `key-text` — A key event carries the UTF-8 text the compositor attached.
- `key-modifiers` — Shift, Ctrl, Alt and Super arrive as separate modifier bits.
- `key-repeat` — A repeat arrives marked as a repeat.
- `text-event` — A text event delivers its UTF-8 string whole.
- `mouse-move` — Pointer motion arrives in window-local coordinates.
- `mouse-buttons` — Left, right and middle press and release arrive with the right button.
- `scroll` — Vertical and horizontal scroll arrive with their signed amounts.
- `focus` — FocusGained and FocusLost arrive as sent.
- `close-requested` — CloseRequested arrives as sent.
- `resized` — Resized arrives only after the buffers have the new size, and says that size.
- `resize-failed` — A failed resize delivers ResizeFailed with the old size, never Resized.
- `font-changed` — FontChanged arrives when the configured font changes.
- `compositor-changed` — A new compositor claim delivers CompositorChanged with the new epoch.
- `unknown-type` — An unknown event type arrives as Unknown, not as a key press.
- `order` — 100 events arrive in the order sent.
- `serial` — Each event carries its input serial.
- `processed-serial` — After the app's handlers return and it acquires, draws and commits, the commit's processed serial is the highest serial whose handler had returned at acquire.
- `processed-not-polled` — An event polled but not yet handled when an animation frame is committed is not acknowledged by that commit.
- `processed-at-acquire` — An event whose handler returns after frame N was acquired is acknowledged by frame N + 1, not by frame N.
- `no-change-after-redraw` — commit_no_change does not acknowledge an event whose handler returned Redraw until a frame acquired after it is committed.
- `processed-ack` — commit_no_change publishes the processed serial without changing pixels.
- `overflow` — A full ring reports how many events were lost instead of dropping them silently.
- `wait-timeout` — wait_event with a timeout returns no earlier than the timeout and within 100 ms after it when nothing arrives.
- `wait-eintr` — A signal interrupts a blocked wait_event with EINTR.
- `wait-targeted` — An event for one window does not wake a reader blocked on another.
- `pollable` — A window's event descriptor reports POLLIN through poll when an event is queued.
- `per-window` — Two windows of one process each receive only their own events.
- `after-destroy` — Writing to a destroyed window fails with ENOENT.
- `abi-layout` — The event struct has the contract's literal size and field offsets.
- `no-overloaded-fields` — Resize size, button state and text travel in named fields, not borrowed ones.
- `malformed-write` — A compositor write with an invalid type or length fails with EINVAL and delivers nothing.

#### `scanout` — display observation on every target (15)
- `mode-set` — A GUI suite boot runs this target's display at 1280x800, and fbinfo agrees.
- `device-path` — Readback uses this backend's observation path from the contract and names it.
- `changing-scene` — Three successive known scenes each read back equal to their formulas.
- `full-destination` — Readback covers the whole destination, and a change in its last row is seen.
- `binding` — The record names the scanout resource or address the device is bound to, and it is the composed one.
- `completion` — The record carries answered completion for every damaged rectangle of its composite serial.
- `atomic-record` — The pixels and the record always belong to the same composite serial under continuous commits.
- `window-commits` — The record lists the commit id of each window composed.
- `format-canonical` — RGB and BGR destinations read back as the same canonical BGRX pixels.
- `root-only` — A non-root, non-compositor process cannot read back the scanout.
- `fault-skipped-transfer` — A skipped transfer or update is reported as missing completion, and the next composite recovers.
- `fault-wrong-resource` — A flush or scanout bound to the wrong resource is reported as a binding mismatch.
- `fault-stale-fence` — A fence answered for another serial is reported as a serial mismatch.
- `fault-outside-damage` — Corruption written outside any damage is found by comparing the full destination.
- `boundary-stated` — The record states what lies outside the guest's view for this backend.

#### `panel` — suite & probe panels (8)
- `counts` — Fixed pass, fail and count fixtures draw the expected digits in the expected colours.
- `fail-rows` — Failed cases are listed with their names in the failure rows.
- `layout` — The category bars and score occupy the contract's rectangles at this mode.
- `handoff` — A display-taking child under probe takes the display, gives it back, and the panel is restored exactly.
- `between-cases` — In a GUI suite the panel is drawn only between cases and restored exactly after each.
- `cursor-restore` — Moving the pointer over the panel leaves no cursor trail.
- `cursor-trail-detected` — With the panel cursor's save and restore disabled by a test control, the trail is detected.
- `kernel-screens` — The kernel's console screen redraws exactly after the display is given back.

## 16. Suite `window-manager`

Manifest: id `window-manager`, title "Window manager", area "Window manager", `"timingSensitive": true`, binary
`/sbin/suite-window-manager`, source `suite_window_manager.rs`. Summary: *"Keyboard and pointer events from
every target's devices reach bwm through the kernel's input core, and bwm composes, stacks, focuses, moves,
resizes and closes the windows of independent programs, routes input and the clipboard between them, gives
every event a recorded outcome, recovers from its own death, and stays live and fast when clients hang, crash
or misbehave, on every target."*

| Milestone | Title | Categories |
|---|---|---|
| `input-core` | input core & virtual devices | `input-core` |
| `input-devices` | keyboard & pointer drivers on every target | `input-devices` |
| `startup` | bwm starts and shows clients | `startup` |
| `composition` | composition, chrome & repaint | `composition` |
| `stacking` | stacking & minimize | `stacking` |
| `focus` | focus, keyboard routing & hotkeys | `focus` |
| `pointer` | pointer routing | `pointer` |
| `geometry` | move & resize | `geometry` |
| `lifecycle` | close, crash & launch | `lifecycle` |
| `recovery` | WM death, restart & supervision | `recovery` |
| `clipboard` | clipboard between programs | `clipboard` |
| `liveness` | idle, stalls & stress | `liveness` |
| `performance` | latency & frame rate | `performance` |

**Foundation work, in order:**
1. The input core, the event ABI, `/dev/uinput` with `EV_TEXT` and `time=injector`, the dispositions ledger,
   grabs and session device selection, derived modifier state, the console keymap consumer, input test
   controls, and on x86-64 the revived PCI virtio-input driver with `virtio-tablet-pci` on the gate.
2. bwm reads input events instead of stdin bytes, records a disposition for every serial, generates repeats and
   recognizes double clicks and the Super double tap on its clock (section 6.1), and hit-tests and rejects
   pointer events tagged with a stale scene version. It publishes focus, visibility and stacking to the kernel,
   and the compositor leaves out windows that are not visible. It sends configure requests and waits for the
   client's acknowledgment before it changes a frame's size. It reads `bwm.conf`, `hotkeys.conf` and its UI
   fonts on every target, ARM64 included. It acknowledges UI-clock epochs and its handled serials in its status
   page through `compositor_ack` (section 6.3), composing only when something it draws changed. It runs on the
   software compositor where there is no VirGL.
3. The defects the recon lists: focus cycling, focus notices naming the wrong window, minimize not hiding
   content, live focus when everything is minimized, disconnected modifier hotkeys, no pointer capture, stale
   drag and resize indices, negative resize sizes, ignored composite errors, and the stationary cursor shape.
   Each is fixed with the case that fails today.
4. `libs/desktop-supervisor`: starting, supervising (bounded restarts) and stopping bwm and the session's first
   clients. `init desktop` uses it, and so do the `startup` and `recovery` cases.
5. `wm-client_test`: a configurable client. It fills its window with an id-derived pattern and logs every
   event it receives to a shared ring the case reads. Its behaviour flags: ignore close, slow commit, hang,
   crash after N events, two windows, animate by frame counter, refuse resize, and never read events.
6. Expected chrome geometry is a literal table in `docs/gui/bwm.md`, typed into the test crate by hand. bwm's
   code never computes its own expectations.

#### `input-core` — input core & virtual devices (31)
- `uinput-create` — Root creates a virtual keyboard and pointer device with declared capabilities.
- `uinput-nonroot` — A non-root open of /dev/uinput fails with EACCES after root's open has succeeded.
- `capabilities` — An event outside the device's declared capabilities is rejected with EINVAL.
- `abi-layout` — The event struct has the contract's literal size and field offsets.
- `key-press-release` — A virtual key press and release reach the event stream as values 1 and 0 with Linux keycodes.
- `modifiers-derived` — Shift, Ctrl, Alt and Super state follows key events, the same for every device.
- `ctrl-super-distinct` — Ctrl and Super are distinct keys, and treating one as the other is an explicit setting.
- `pointer-absolute` — An absolute event maps to the screen coordinates the contract's formula gives.
- `pointer-relative` — Relative motion accumulates and clamps to the screen.
- `buttons` — Left, right and middle press and release arrive as sent.
- `wheel` — Vertical and horizontal wheel steps arrive with their signs.
- `syn-batches` — Events between SYN_REPORTs are delivered as one batch.
- `serials` — Every event has a strictly increasing serial across all devices.
- `time-real` — A real-device event carries CLOCK_MONOTONIC time.
- `time-injected` — An injector-timed device's events carry the injector's times, and a decreasing time is rejected.
- `two-readers` — Two readers of the event stream each receive every event.
- `syn-dropped` — A reader that falls behind receives SYN_DROPPED, and each dropped serial is recorded as lost.
- `poll` — The event device reports POLLIN through poll when events are queued.
- `grab` — While the compositor holds the grab, the console sees no key events.
- `grab-select` — Events from a device outside the session's selection reach no client and are recorded as not-selected.
- `grab-drain` — Taking the grab drains queued events and resets pressed state.
- `device-removal` — Closing a device with keys and buttons held delivers their releases.
- `text-event` — An EV_TEXT event of 1 to 256 valid UTF-8 bytes reaches the compositor whole.
- `text-invalid` — Invalid UTF-8, a truncated record and an oversize payload are each rejected whole.
- `malformed-batch` — A truncated batch or an unknown type is rejected with EINVAL, and nothing of it is published.
- `console-keymap` — With no compositor, key events become the console bytes the US keymap gives.
- `console-special-keys` — Arrows, Home, End, Delete, Page Up and Page Down become VT sequences on the console.
- `console-modifiers` — Ctrl+C on the console gives byte 3 from every device type.
- `ledger-complete` — Every serial of a mixed script ends with exactly one terminal disposition.
- `ledger-gap` — A serial left without a disposition is reported as the first missing serial.
- `console-after-compositor` — When the compositor dies, keys return to the console.

#### `input-devices` — keyboard & pointer drivers on every target (15)
- `hid-keyboard-decode` — Recorded USB boot-keyboard reports decode to the literal expected events.
- `hid-rollover` — Six-key rollover and phantom-state reports decode as the contract says.
- `hid-tablet-parallels` — Recorded Parallels tablet reports decode to the expected pointer events.
- `hid-tablet-vmware` — Recorded VMware tablet reports, including merged endpoints, decode correctly.
- `hid-relative` — Recorded relative-mouse reports decode to relative motion.
- `hid-wheel` — Recorded wheel reports decode to wheel events.
- `virtio-keyboard-decode` — Recorded virtio-input key events decode to the same core events.
- `virtio-tablet-decode` — Recorded virtio-input absolute, button and wheel events decode correctly.
- `ps2-scancodes` — Recorded PS/2 sequences, including E0- and E1-prefixed keys, decode correctly.
- `keyboard-completion-path` — A recorded report replayed through the production completion function of this target's keyboard driver (the coverage map's row: virtio-input, PS/2 or xHCI HID) reaches a reader with a disposition, wakes the compositor and leaves the device armed.
- `pointer-completion-path` — The same for this target's pointer driver (virtio-input or xHCI HID tablet).
- `replay-report-fault` — With the report call skipped by a test control, the completion-path case's event never arrives, and the case says so.
- `replay-wake-fault` — With the wake skipped, the idle compositor's wake is missing, and the case says so.
- `devices-bound` — This target binds exactly the keyboard and pointer drivers its row of the coverage map names, by name.
- `device-capabilities` — Each bound device reports the capabilities its driver declares.

#### `startup` — bwm starts and shows clients (12)
- `claims-display` — bwm claims the compositor and the display, and its status page shows the new epoch.
- `no-gpu-required` — bwm runs on this target's backend without exiting.
- `empty-desktop` — With no clients, the screen shows exactly the background, taskbar and appbar from the contract's table.
- `first-client` — A test client's committed pattern is composed at its assigned position, and the composite record lists its commit.
- `existing-windows` — Windows created before bwm starts are adopted and composed.
- `config-test` — The session bwm.conf sets pinned fonts, frozen clock text and no frame-rate output.
- `config-invalid` — A malformed bwm.conf falls back to defaults and shows its error on the desktop.
- `fonts-loaded` — bwm's configured UI fonts are used on every target.
- `hotkeys-loaded` — hotkeys.conf bindings are read on every target.
- `supervisor-starts` — The desktop supervisor starts bwm and a client and reports both running.
- `composited-marker` — bwm prints its composited-client marker only after a composite record lists a client commit.
- `epoch-ack` — bwm acknowledges each stepped UI-clock epoch in its status page, with `none-needed` and no composite when nothing it draws changed.

#### `composition` — composition, chrome & repaint (27)
- `client-position` — A client's pattern appears at the content origin bwm assigned, exactly.
- `chrome-geometry` — Titlebar, border and buttons occupy the contract table's rectangles in the theme's colours.
- `title-text` — The window's title matches the certified text renderer's drawing at the contract's position.
- `overlap` — Where B is over A the screen shows B, and A's titlebar under B is hidden.
- `cascade` — New windows cascade by the contract's offset.
- `shadow` — The drop shadow's pixels match the contract's formula.
- `background` — The desktop background shows wherever no window is.
- `appbar-entries` — The appbar lists one entry per window, in creation order.
- `chromeless` — A chromeless window has no frame, sits where the contract says and has the dimmer behind it.
- `sixteen-clients` — Sixteen clients each show their own pattern.
- `client-update` — A client's new commit reaches the screen in the next composite.
- `moving-client` — An animating client's every committed frame is composed in order as the case steps it.
- `frozen-client` — A hung client keeps its last frame on screen while the chrome stays live.
- `present-failure` — A forced composite failure is reported by bwm and retried, and is not counted as a frame.
- `visible-only` — A minimized window's content is left out of composition.
- `expose-close` — Closing the top window reveals the exact pixels beneath it.
- `expose-move` — Moving a window repaints the uncovered area exactly.
- `partial-equals-full` — A partial redraw gives the same screen as a full redraw.
- `damage-bounded` — Moving a small window composes exactly the union of its old and new outer painted bounds.
- `outer-bounds` — The outer painted bounds, shadow and cursor included, match the contract's table.
- `scene-identical` — A fixed three-window scene hashes to the pinned value, the same on every target.
- `resized-content` — After a resize, the content area shows the client's committed buffer at its new size.
- `cursor-over-window` — The cursor draws above windows and leaves no trail when it moves.
- `cursor-edge-shapes` — Hovering each resize edge and corner shows that edge's cursor on every target.
- `cursor-stationary-shape` — A cursor shape change with the pointer still is shown.
- `clock-test-config` — In the test configuration the taskbar clock shows the UI clock's wall time.
- `no-fps-overlay` — The test configuration draws no frame-rate text.

#### `stacking` — stacking & minimize (19)
Expected stacks come from an operation model in the test crate, and each step is checked against the screen,
the client event logs and the registry.
- `click-raises` — Clicking a lower window raises it to the top.
- `raise-stable-id` — Raising by window id raises that window even after others exit.
- `appbar-order` — Appbar order follows creation, not stacking.
- `minimize-hides` — A minimized window's content disappears from the screen, not just its chrome.
- `minimize-no-input` — Keys and clicks never reach a minimized window and are recorded as rejected.
- `restore` — Restoring repaints the window and raises and focuses it.
- `all-minimized` — With every window minimized, nothing has focus.
- `minimize-close` — Closing a minimized window removes its appbar entry.
- `fkeys-skip-minimized` — Function-key selection skips minimized windows.
- `chromeless-top` — A chromeless window stays above normal windows.
- `dimmer-only-with-chromeless` — The dimmer appears only while a chromeless window is shown.
- `launcher-toggle` — The launcher hotkey shows and hides a chromeless test client.
- `close-focus-next` — Closing the focused window focuses the next one in the stack.
- `new-on-top` — A new window opens on top and focused.
- `registry-agrees` — After each of 16 operations, the kernel's z order equals the model's.
- `exit-restacks` — When a middle window's client exits, the others keep their order.
- `model-sequence` — 50 seeded random raise, minimize, restore and close steps match the operation model at every step.
- `minimize-all-restore-all` — Minimizing and restoring every window returns the original stack and screen.
- `titlebar-drag-raises` — Starting a titlebar drag raises the window.

#### `focus` — focus, keyboard routing & hotkeys (30)
- `pair-click` — A click focus change sends exactly one FocusLost to the old window and one FocusGained to the new.
- `pair-appbar` — An appbar focus change sends exactly one correct pair.
- `pair-cycle` — A hotkey focus change sends exactly one correct pair.
- `initial-focus` — The first window receives FocusGained.
- `focus-after-close` — Closing the focused window sends FocusGained to its successor.
- `cycle-next` — Alt+Tab visits every eligible window once before repeating.
- `cycle-prev` — Alt+Shift+Tab visits them in reverse.
- `cycle-skips-minimized` — Cycling skips minimized windows.
- `keys-to-focused` — Typed keys reach only the focused window.
- `text-exact` — Typing "Hello, World!" delivers exactly that text.
- `releases` — Key releases are delivered.
- `release-follows-press` — A key release goes to the window that got the press, even after focus moved.
- `focus-change-mid-press` — A focus change between press and release sends no release to the new window.
- `repeat-delay` — A key held to just before the repeat delay gives no repeat, and at the delay gives one.
- `repeat-rate` — A key held for 1,000 ms on the UI clock gives exactly 11 repeats (500 ms delay, then every 50 ms, inclusive).
- `repeat-cancel` — Releasing the key or removing its device stops repeats at once.
- `repeat-frozen-clock` — With the UI clock frozen, a held key gives no repeats, and the case still ends by its real deadline.
- `ctrl-c` — Ctrl+C arrives as the C key with the Ctrl modifier.
- `navigation-keys` — Home, End, Delete, Page Up, Page Down and the arrows arrive as key events.
- `escape` — A lone Escape arrives at once.
- `super-return` — Super+Return runs its configured command.
- `alt-q` — Alt+Q sends CloseRequested to the focused window.
- `hotkey-consumed` — A hotkey is recorded as WM-handled and not delivered, and the same key without the modifier is delivered.
- `super-double-tap` — Two Super taps exactly at the interval toggle the launcher, and one tick over the interval does not.
- `super-single-tap` — A single Super tap does nothing.
- `hotkeys-config` — A binding in hotkeys.conf works on every target.
- `no-focus-no-delivery` — With no window, typed keys are recorded as rejected for no focus, after the same keys reached a focused window.
- `focus-new-window` — A newly opened window takes focus.
- `routing-after-exit` — After another window's client exits, keys still reach the focused window.
- `lost-then-delivered` — A key lost to a full ring followed by a delivered key leaves the lost serial recorded as lost.

#### `pointer` — pointer routing (21)
- `local-coords` — Motion inside a window arrives in content-local coordinates, exactly.
- `outside-not-delivered` — Motion outside a window is not delivered to it.
- `press-release-order` — Press, motion and release arrive in order.
- `capture` — A press inside, a drag outside and a release outside all go to the pressing window.
- `capture-target-closes` — When the captured window is destroyed, the capture ends and no other window gets the release.
- `right-middle` — Right and middle buttons are delivered.
- `scroll-under-pointer` — Scroll goes to the window under the pointer.
- `scroll-wakes` — Scroll on an idle desktop wakes the compositor and is delivered.
- `fast-click` — A press and release within one composite are both delivered.
- `double-click-threshold` — Two clicks exactly at the interval arrive with click counts 1 and 2, and one tick over gives 1 and 1.
- `double-click-distance` — Two clicks 4 pixels apart give click counts 1 and 2, and 5 pixels apart on either axis give 1 and 1.
- `chrome-click` — Clicks on chrome are recorded as WM-handled and not delivered.
- `enter-leave` — Crossing from one window to another delivers leave and enter.
- `cursor-position` — The cursor is drawn at the injected position.
- `modifier-click` — Shift+click arrives with the Shift modifier.
- `clamp` — Coordinates past the screen edge clamp to it.
- `minimized-none` — A minimized window receives no pointer events.
- `chromeless-click` — Clicks on a chromeless window reach it, and clicks outside it dismiss it.
- `press-raises-delivers` — A press on a lower window raises it and is delivered once.
- `hit-test` — bwm's hit test returns the topmost window at a point inside a partly covered window.
- `stale-scene` — A pointer event tagged with an older scene version is recorded as rejected for a stale scene when it starts a gesture, while a later batch of a gesture already under bwm's grab is delivered to the grabbing window even though the scene changed (section 5.5).

#### `geometry` — move & resize (24)
- `drag-move` — Dragging the titlebar moves the window by the drag distance.
- `drag-top-clamp` — A window cannot be dragged above the taskbar.
- `drag-keeps-titlebar` — A window dragged off a side keeps the contract's minimum titlebar strip on screen.
- `resize-right` — Dragging the right edge changes only the width.
- `resize-bottom` — Dragging the bottom edge changes only the height.
- `resize-left` — Dragging the left edge moves the left side and keeps the right side fixed.
- `resize-top` — Dragging the top edge moves the top side and keeps the bottom side fixed.
- `resize-corners` — Each corner resizes both dimensions, anchored at the opposite corner.
- `resize-minimum` — A resize stops at the minimum width and height.
- `resize-cross` — Dragging past the opposite edge clamps at the minimum and never gives a huge size.
- `resize-maximum` — A resize stops at the screen and buffer limits.
- `configure-ack` — The frame changes only after the client acknowledges the size with its configure serial.
- `buffer-matches` — After a resize, the client's buffer size equals the frame's content size.
- `coalesce` — A 60-step resize drag allocates client buffers no more often than configures are acknowledged.
- `client-refuses` — A client whose resize fails under RLIMIT_AS keeps its old geometry, and the frame returns to it.
- `exit-during-drag` — A client exiting mid-drag ends the drag, and no other window moves.
- `exit-during-resize` — A client exiting mid-resize ends the resize, and no other window changes.
- `client-resize` — A client-initiated resize is reconciled into the frame.
- `client-title` — A client's title change shows in its titlebar and appbar.
- `remembered-geometry` — Reopening a window with the same title restores its last position and size.
- `expose-during-drag` — Windows beneath repaint exactly at each step of a drag.
- `resize-repaints` — During a resize, the uncovered desktop and windows repaint exactly.
- `geometry-registry` — After every move and resize, the registry's geometry equals the model's.
- `chromeless-not-resizable` — A chromeless window has no resize edges.

#### `lifecycle` — close, crash & launch (20)
- `close-request` — The close button sends CloseRequested to that window and no signal.
- `close-one-of-two` — Closing one of a process's two windows leaves the other open.
- `close-unresponsive` — A second close on a window that ignored the first sends SIGTERM to its owner.
- `client-exit` — A client's exit removes its window and repaints beneath it.
- `client-crash` — A client killed by SIGSEGV is removed the same way.
- `client-exec` — A client that execs loses its windows.
- `launch-ack` — A hotkey bound to a test client starts it, and its first commit is composed within the 5,000 ms UI-time launch deadline.
- `launch-missing` — A hotkey bound to a missing program shows an error on the desktop and leaves no zombie.
- `launch-fork-fails` — A launch whose fork fails by test control shows an error, and the next launch succeeds.
- `launch-reaped` — Programs bwm launched are reaped when they exit.
- `launch-close-50` — 50 launch and close cycles return windows, processes and memory to the baseline.
- `inject-denied` — A client cannot write input into another window.
- `compositor-steal` — A client cannot claim the compositor while bwm holds it.
- `read-other-denied` — A non-root client cannot capture another window.
- `huge-window` — A client's 4096x4096 window is clamped on screen, and the desktop stays usable.
- `commit-flood` — A client committing as fast as it can does not delay input to other windows.
- `title-flood` — A client changing its title 10,000 times does not stall the desktop.
- `never-reads` — A client that never reads events loses only its own events, each recorded as lost.
- `normal-exit-gives-back` — bwm's normal exit gives the display back.
- `window-limit` — A seventeenth window is refused visibly, and the others are unaffected.

#### `recovery` — WM death, restart & supervision (15)
- `death-releases` — When bwm is killed, the display returns to the session supervisor, grabs and pressed state reset, and the epoch advances.
- `clients-notified` — Every client receives CompositorChanged.
- `blocked-acquire` — A client blocked in acquire when bwm dies returns within 250 ms.
- `blocked-wait` — A client blocked in wait_event when bwm dies returns within 250 ms.
- `death-at-composite` — bwm killed while composing a client's commit leaves that client able to commit to the next bwm.
- `death-mid-configure` — A configure pending at bwm's death is dropped, the client keeps its size, and the next bwm reconciles it.
- `death-mid-capture` — A pointer capture held at bwm's death ends, and the next bwm starts with no capture.
- `restart-adopts` — A restarted bwm composes the existing windows.
- `stack-restored` — The restarted bwm restores z order, minimized state and geometry from what the last bwm published.
- `focus-restored` — The restarted bwm restores focus to the window that had it.
- `clipboard-reset` — A restarted bwm starts with an empty clipboard, as the contract says.
- `interaction-after-restart` — The same surviving client receives typed text and clicks under the restarted bwm.
- `supervisor-restarts` — The desktop supervisor restarts bwm after SIGKILL.
- `supervisor-gives-up` — After five restarts within ten seconds the supervisor stops and reports the crash loop.
- `unexpected-crash-fails` — An app crash during a deliberate bwm restart fails the scenario, distinct from the expected compositor death.

#### `clipboard` — clipboard between programs (16)
- `set-get` — A program reads back the text it put on the clipboard.
- `cross-process` — Text one program sets is the text another gets.
- `utf8` — Multi-byte UTF-8 survives byte for byte.
- `empty` — Getting an empty clipboard returns empty, not an error.
- `large` — 1 MiB of text round-trips.
- `too-large` — Text over the 16 MiB limit is refused with E2BIG, and the clipboard keeps its previous text.
- `replace` — A second set replaces the first, and the first owner is told.
- `owner-exit` — The text stays after its owner exits.
- `change-notice` — Programs that asked are told when the clipboard changes.
- `concurrent-sets` — Of two concurrent sets, the one with the later clipboard serial wins, and both setters see that result.
- `mime-text-only` — A type other than text/plain;charset=utf-8 is refused.
- `window-required` — A process with no window on the desktop is refused.
- `unprivileged` — A uid 1000 client with a window can set and get.
- `textfield-copy` — Ctrl+C in a libbui text field copies its selection.
- `textfield-paste` — Ctrl+V in a libbui text field inserts the clipboard text.
- `nul-bytes` — Text containing NUL bytes is stored whole.

#### `liveness` — idle, stalls & stress (12)
Deterministic: counts of composites, epochs and events, on the stepped UI clock.
- `idle-no-composites` — Over 100 epochs with nothing changing, every epoch is acknowledged by bwm and the clients, and the composite counter does not move.
- `deadline-wake` — bwm wakes for its next clock deadline with no other work.
- `wake-on-registry` — A new window wakes an idle compositor.
- `compositor-wait-timeout` — compositor_wait returns no earlier than its timeout and within 100 ms after it when nothing happens.
- `slow-client` — A client committing every tenth epoch does not reduce other clients' frames per epoch.
- `hung-client` — A hung client does not stop the desktop or input.
- `stopped-client` — A client stopped with SIGSTOP and explicitly excluded does not stop the desktop, and stays stale.
- `stress-animation` — Eight animating clients plus typing for 300 epochs: every window advances every epoch, and every serial is processed.
- `stress-io` — Animation plus disk and loopback network traffic still processes input in order, with every disposition recorded.
- `progress-under-load` — Five seconds of multi-window composition raise the composite counter in every 100 ms window, and the soft-lockup counter stays unchanged.
- `memory-bounded` — After a 60-epoch warmup, 600 epochs of eight animating clients grow bwm's resident memory by at most 64 KiB.
- `cursor-shape-idle` — A cursor shape change on an idle desktop is shown.

#### `performance` — latency & frame rate (7)
Real time on `CLOCK_MONOTONIC`, with the workloads and bounds in section 12.2.
- `typing-latency` — Keys on an idle desktop reach a composited client commit within a median of 20 ms and a maximum of 50 ms.
- `pointer-latency` — Pointer motion reaches a composited client commit within a median of 20 ms and a maximum of 50 ms.
- `latency-under-animation` — With four clients animating, keys reach a composited commit within a median of 33 ms and a maximum of 100 ms.
- `present-latency` — Commit to composite stays within a median of 16 ms and a maximum of 33 ms.
- `frame-floor` — A single animating client gets at least 60 composites a second.
- `idle-cpu` — An idle desktop with three static clients uses at most 2% of one CPU.
- `clipboard-latency` — A 64 KiB copy and paste completes within 50 ms.

## 17. Suite `ui-testing`

Manifest: id `ui-testing`, title "UI test framework", area "UI test framework", `"timingSensitive": true`,
binary `/sbin/suite-ui-testing`, source `suite_ui_testing.rs`. Summary: *"Scripted keyboard, pointer and text
input reaches GUI programs through the window manager with a recorded outcome for every event; their trees and
committed frames are captured whole and compared to independent expectations at acknowledged clock epochs and
causal barriers; every planted defect is caught for its own reason; and every failure leaves its evidence, on
every target."*

| Milestone | Title | Categories |
|---|---|---|
| `runner` | result transport & proof executor | `runner` |
| `input` | scripted keyboard, pointer & text input | `input` |
| `tree` | semantic trees of windows & widgets | `tree` |
| `frames` | frame capture & comparison | `frames` |
| `clock` | UI time & stepping | `clock` |
| `settle` | causal barriers & settling | `settle` |
| `sessions` | sessions, state isolation & cleanup | `sessions` |
| `diagnosis` | saved evidence & failure messages | `diagnosis` |
| `proof` | proven on bwm & a test app | `proof` |

**This effort's product:** `libs/libuitest` (sessions and the session supervisor with its evidence arena,
scripted input with dispositions, tree queries, frame checks, the epoch-and-settle protocol, artifacts, and the
inner scenario executor); the tree pages and builder in Breengel, libbui and bwm, with layout versions; the
input serial chain through bwm and Breengel, with scene and layout tags; `Suite::gui()`'s mandatory result
transport, FRAME accounting, collector socket and collection handshake; the descendant-scoped sweep and the
transcript descriptor in the shared runner; `suite-runner-fixture_test`; the FRAME, TREE, INPUT and COLLECT
records;
`frameChecks` in the manifests, the validators, the verdict script and Vigil; `PR_SET_CHILD_SUBREAPER`;
`BREENIX_CONFIG_ROOT` support in every GUI program; `docs/suites/frames/` with its provenance file, the
installer and `tools/gfx-refgen`; `/var/uitest` artifacts and `scripts/uitest-artifacts.py`; and
`uitest-app_test`.

`uitest-app_test` is one window holding every widget kind, a text grid, a canvas and an animation. Its fault
flags each plant one defect: `off-by-one`, `wrong-colour`, `drain-no-render`, `stale-pixels`, `drop-key`,
`lie-bounds`, `lie-text`, `slow-commit`, `wallclock`, `hang`, `crash` and `trail`. `bwm-fault_test` is the real
bwm built with one named fault per run (`misroute-focus`, `chrome-offset`, `keymap-swap`, `drop-disposition`,
`ignore-selection`) and installed only in `/usr/local/test/bin`. The production bwm has none of them.

#### `runner` — result transport & proof executor (22)
- `transport-always` — Records reach the runner with the live strip turned off.
- `transport-blocking` — A writer that fills the pipe blocks, and all 256 records arrive.
- `transport-exit-race` — A record written immediately before the case exits still arrives.
- `transport-capacity` — A suite table declaring more checks for one case than the record limit is refused at suite start.
- `record-binding` — Under the real runner in the scoped fixture (section 8.4), a record naming another case or an undeclared check makes that runner report the case FAIL.
- `record-malformed` — Under the real runner in the fixture, a malformed record makes it report the case FAIL.
- `record-duplicate` — Under the real runner in the fixture, a second record for the same check makes it report the case FAIL.
- `forgery-exec` — An app the case execs cannot write records.
- `forgery-fork` — A helper created through `suite::spawn` that never execs cannot write records.
- `never-ran` — Under the real runner in the fixture, a case that declares a frame check and never runs it is reported FAIL, naming the check.
- `mismatch-is-fail` — Under the real runner in the fixture, a case that returns Ok after a mismatch record is reported FAIL.
- `outer-positive` — The fixture's clean case passes under the real runner, and the fixture's transcript ends in a consistent DONE.
- `outer-case-death` — A fixture case killed mid-scenario is reported FAIL by the real runner, its supervisor's COLLECT and `done` arrive before the sweep, and the evidence it names is on disk.
- `outer-collector-timeout` — A fixture supervisor that never answers is swept at the case limit, the runner writes `artifacts=none:collector-timeout`, and the case's primary failure stands; a fixture supervisor that dies instead makes the runner write `artifacts=none:supervisor-died` and reap the session's orphans.
- `sweep-descendants` — The real runner, run as a subreaper, kills and reaps every descendant of the fixture, a double-forked helper included, and nothing outside it.
- `inner-pass` — A clean scenario run in the inner executor returns Pass.
- `inner-expected-failure` — A scenario with a planted pixel fault returns pixel-mismatch with its check and first pixel.
- `inner-wrong-class` — An outer case expecting pixel-mismatch fails when the scenario fails another way.
- `inner-crash-not-detection` — A scenario whose app crashes is not accepted as detecting a pixel fault.
- `inner-teardown-scoped` — Inner teardown kills only the scenario's descendants, and the outer case's helpers survive.
- `inner-records-contained` — An inner scenario's records never appear in the outer suite's records.
- `subreaper` — In a boot session, an inner scenario and the real-runner fixture alike, a double-forked descendant that exits with status 7 after its parent has died is reparented to that session's supervisor (its parent pid read from `/proc/<pid>/stat` is the supervisor's) and reaped there, and the supervisor reports its pid and status 7 to the case; when the supervisor is killed first, the orphan goes to the runner, the runner writes `artifacts=none:supervisor-died`, and the case fails naming the dead supervisor.

#### `input` — scripted keyboard, pointer & text input (29)
- `type-ascii` — type_text("hello world") puts exactly that text in the focused text field.
- `type-shifted` — Characters that need Shift arrive correctly.
- `type-keymap-all` — Every printable ASCII character, typed through libuitest's own literal key table, arrives exactly.
- `type-utf8` — Characters outside the keymap arrive through a text event.
- `type-control` — Newline and Tab arrive as the Enter and Tab keys.
- `chord` — chord(Ctrl, A) arrives as A with the Ctrl modifier.
- `hold-repeat` — Holding a key for 600 ms of UI time gives exactly 3 repeats.
- `click-node` — click(node) lands on a visible point of the node, and the delivery names that node's window.
- `click-partly-covered` — Clicking a node partly covered by another window lands on its exposed part, confirmed by hit test.
- `click-covered-fails` — Clicking a node fully covered by another window fails the step by name.
- `click-clipped` — Clicking a node clipped by a scrolled container uses its visible part.
- `click-zero-area-fails` — Clicking a node with no visible area fails the step by name.
- `click-stale-scene` — When the window moves between the query and the click, the click is rejected for a stale scene, re-resolved and lands correctly.
- `click-stale-layout` — When the node's list scrolls inside an unmoved window between the query and the click, Breengel rejects the unstarted click for a stale layout, and the re-resolved click lands on the node; when the press itself expands the list, the release still reaches the pressed node through capture, the click activates exactly once, nothing is retried, and no node is left pressed; when the pressed node is removed before the release, the gesture is cancelled, its release is `client-rejected(gesture-cancelled)`, and the step fails by name without a retry.
- `double-click` — double_click delivers click counts 1 and 2.
- `drag-steps` — A ten-step drag delivers ten motions in order between a press and a release; when the layout changes on the press and again mid-drag, and when the window moves mid-drag, every later motion and the release still reach the captured node through bwm's grab, the drag ends in exactly one completed gesture, and no button, pressed flag or capture is held at the end.
- `scroll-node` — scroll(node, dy) delivers the wheel amount to the window under the node.
- `focus-then-type` — Typing into a node focuses it first, and a node that cannot take focus fails by name.
- `delivered` — delivered(serial) returns only after the target window processed that serial.
- `undelivered-fails` — An event sent to a hung app is reported unprocessed when the 2 s settle bound passes, naming the serial and the window.
- `dispositions-all` — Every serial of a script ends with a terminal disposition.
- `trace-hops` — The input log records each event at the core, the WM and the client.
- `synthetic-tagged` — Injected events are tagged synthetic in the trace.
- `nonroot-denied` — A non-root session cannot inject, after a root session has.
- `teardown-releases` — Ending a session releases every held key and button.
- `input-during-animation` — Input arrives in order while the app animates.
- `two-windows` — Input addressed to two windows of one app arrives at each.
- `wm-hotkey-scripted` — A scripted Super double tap opens the launcher through bwm's real hotkey path.
- `real-input-excluded` — Events from a device outside the session's selection change nothing in the session.

#### `tree` — semantic trees of windows & widgets (23)
- `roles-keys` — The test app's tree lists every widget with its expected role and key.
- `names-values` — Names and values match the widgets' states set by the script.
- `role-invariants` — Menu items lie inside their menu, tabs inside their tablist, and thumbs inside their track.
- `focus-flag` — The focused flag moves with Tab.
- `focus-ring-pixels` — The focused widget draws the theme's focus ring at its bounds.
- `enabled-flag` — A disabled button is flagged not enabled.
- `state-flags` — Checked, selected and pressed flags follow interaction.
- `textfield-value` — A text field's value equals the typed text.
- `terminal-grid` — A terminal node's rows, cell attributes and cursor equal what its grid holds.
- `commit-coupled` — A tree and its pixels always come from the same commit.
- `rapid-commits` — Over 1,000 rapid commits, every read gives a tree and pixels from one commit.
- `truncation-flagged` — A tree larger than its page is flagged truncated, not corrupted.
- `wm-tree` — bwm's tree lists each window's title, bounds, z order, focus and minimized state.
- `wm-tree-vs-model` — bwm's tree agrees with the operation model and the client logs after every step of a script.
- `find-unique` — A query that matches exactly one node returns it.
- `find-ambiguous-fails` — A query that matches two nodes fails, naming both.
- `find-none-fails` — A query that matches nothing fails, naming the query.
- `hidden-nodes` — Nodes scrolled out or covered are flagged not visible, with their clip.
- `list-scroll` — Scrolling a list changes which items are flagged visible.
- `permission` — An unrelated non-root process cannot map another app's tree.
- `no-reader-no-serialize` — With no reader attached the tree is never serialized, and with one it is serialized once per commit.
- `dump-roundtrip` — The text dump of a tree parses back to the same tree.
- `text-pixels` — A label whose tree text is "PASS" is checked against the certified renderer's "PASS" at its bounds.

#### `frames` — frame capture & comparison (32)
- `window-capture` — A window capture equals the committed buffer of that commit.
- `capture-while-drawing` — Capturing commit N while the app draws N+1 returns N whole.
- `capture-two-readers` — Two readers capturing the same window each get whole commits.
- `capture-resize-race` — A resize during a capture gives the whole old commit or ENOENT, never a mixture.
- `capture-destroy-race` — A destroy during a capture gives the whole commit, and the buffer is freed afterwards.
- `capture-threads` — Commits from two producer threads are each captured whole.
- `screen-capture` — A screen capture shows the window at its position, with its observation record.
- `capture-commits` — A screen capture lists the commit of each window it contains.
- `exact-match` — An identical frame matches with zero differing pixels.
- `one-pixel-fails` — A frame with one changed pixel fails as pixel-mismatch, naming that pixel and both colours.
- `channel-tolerance` — A difference of 2 passes at tolerance 2 and fails at tolerance 1.
- `max-differing` — The differing-pixel cap passes at its limit and fails one pixel past it.
- `regions-same-frame` — All regions of a check must pass in the same frame.
- `mask` — A masked region is excluded, and a mask without a reason fails validation.
- `mask-cap` — Masks that leave less than 75% of a region fail validation; the 100x100 region whose four one-pixel border strips are masked (396 pixels, 96.04% still compared) passes this region rule, so `structural-vacuous` must be what rejects it.
- `unknown-field` — A check carrying a field schema version 1 does not define, `edgeRadius` included, fails validation.
- `size-mismatch` — Frames of different sizes fail without comparing.
- `reference-pin` — A reference whose SHA-256 differs from its pin fails before comparing.
- `reference-missing` — A missing reference fails, naming the path.
- `formula` — A formula check compares like a stored one.
- `structural-uniform` — A uniform-region check passes on a flat fill and fails on one stray pixel.
- `structural-outside` — A no-colour-outside check catches a pixel leaking past its rectangle.
- `structural-count` — A colour-count check catches a blank frame.
- `structural-vacuous` — Each predicate's effective-domain rule is enforced after masks, before any comparison: a uniform check without a colour, a colour count below 2 or above the compared pixels, overlapping equal regions, equal regions with fewer than 2 compared pairs, `minDiffering` above the domain, an operand that names no declared region, and a region of one pixel each fail validation; and an `outside` check on a 100x100 region with a strictly interior 98x98 `rect` and its four border strips masked fails validation for an empty outside domain, while the same check unmasked catches one forbidden-colour pixel planted in the border.
- `formula-params` — A formula check with a missing, extra, mistyped or out-of-range parameter fails validation.
- `schema-version` — A check with an unknown schema version fails validation.
- `schema-binding` — A check bound to a window key or checkpoint the scenario never reaches fails, naming it.
- `png-reference` — A reference PNG decodes to the pixels that were encoded.
- `cursor-excluded` — Captures taken with the cursor hidden contain no cursor pixels.
- `cross-target` — The test app's canonical scene hashes to the pinned value on this target.
- `frame-record` — Every check writes one well-formed FRAME record with its definition hash.
- `compare-cost` — Comparing two 1280x800 frames takes at most 50 ms.

#### `clock` — UI time & stepping (19)
- `clock-frozen` — An app reading the UI clock sees it stand still between steps.
- `clock-step` — step(16) advances the app's clock by exactly 16 ms and the epoch by one.
- `epochs-acked` — Each step is acknowledged by every required participant in its commit metadata.
- `equal-deadlines` — Two timers due at the same time fire in registration order.
- `multiple-expiry` — A periodic timer whose period passes ten times in one step fires once and reports nine missed.
- `zero-step` — step(0) advances the epoch, fires only timers due now, and is acknowledged without new frames.
- `large-step` — One step of 1,000 ms leaves an animation in the same state as a thousand steps of 1 ms.
- `stamps` — Each commit carries its epoch and its UI time in milliseconds.
- `timers-virtual` — An app's one-second timer fires on step(1000), not on real time.
- `wall-time-fixed` — Wall-clock time shown by apps and bwm is the fixed instant plus elapsed UI time.
- `wm-clock` — bwm's taskbar clock and repeat timer read the UI clock.
- `shared-clock` — Two apps in one session step together.
- `stopped-unacked-fails` — A stopped participant makes step fail naming it, unless the case excluded it.
- `backpressure-stepped` — Stepping never deadlocks against buffer back-pressure.
- `continuous-animation` — An app that animates forever still lets every step complete.
- `real-clock-counter` — An app's real-clock read counter stays 0 in a session and rises when the test app reads the real clock on purpose.
- `input-time` — Injected events carry UI time, and bwm's double-click threshold uses it.
- `watchdog-real` — With the UI clock frozen, the case is still ended by its real deadline.
- `no-real-clock` — With a UI clock set, Breengel's clock functions never read the real clocks.

#### `settle` — causal barriers & settling (13)
- `idle` — settle returns once nothing changes.
- `processed-not-consumed` — settle waits until the target window commits with the serial processed, not until it reads the event.
- `drained-not-rendered` — An app that drains events and never commits makes settle fail, naming the window and the serial.
- `no-change-ack` — An event that changes nothing settles through commit_no_change.
- `composited` — settle waits until a completed composite lists every visible window at its newest pixel generation.
- `no-change-no-composite` — Settling after a commit_no_change completes through the composite that already holds the window's pixels, with no new composite.
- `bwm-ack-no-composite` — An idle step settles through bwm's `none-needed` acknowledgment, with the composite counter unchanged.
- `wm-handled` — A hotkey settles with no client involved, through its WM-handled disposition.
- `async-predicate` — settle waits for a named tree predicate on an asynchronous result.
- `timeout-names` — A settle that cannot complete fails, naming the window, serial or predicate it waited for.
- `animation-allowed` — settle returns while an app has a future animation deadline.
- `minimized-acks` — A minimized window still acknowledges epochs and processes its serials, settle waits for those, and settle does not wait for its pixels; once restored, the next settle waits for its current generation.
- `repeated-settle` — Repeated settle calls with nothing changing cause no extra frames.

#### `sessions` — sessions, state isolation & cleanup (23)
- `start-desktop` — A desktop session with one app reaches its first composited commit within 300 ms.
- `start-kiosk` — A kiosk session with one app reaches its first composited commit within 300 ms.
- `environment` — Apps in a session receive its fonts, clock, seed, mode, time zone, locale and config root.
- `config-root` — bwm, Breengel's font configuration and every app read and write under the session's config root.
- `global-untouched` — A session that changes every setting leaves the global configuration files unchanged.
- `restore-after-crash` — A case killed while changing settings is followed by a case that sees the defaults.
- `order-independent` — Two settings scenarios give the same results in either order.
- `devices-reset` — After a case its virtual devices are gone and no key or button is held.
- `teardown` — After a case, no session process remains and the runner owns the display.
- `teardown-hung` — A hung app is killed and reaped at teardown.
- `panel-not-in-case` — A screen capture during a case contains no panel pixels.
- `panel-after-case` — The panel is redrawn after the case, with that case's result.
- `ownership-stack` — When a session's compositor dies, the display goes to the session supervisor, then to the runner at case end.
- `case-timeout` — A scenario that runs out of time fails, and its evidence is saved inside the reserve.
- `app-crash` — An app crashing during a scenario fails it, naming the app and the signal.
- `app-stderr-capped` — An app's stderr is saved, capped at 64 KiB.
- `unprivileged-app` — An app launched as uid 1000 runs in the session while the harness keeps root observation.
- `twenty-sessions` — Twenty sessions in a row leave windows, processes and memory at the baseline.
- `wait-window` — wait_window(title) returns after that window's first composited commit.
- `wait-window-timeout` — wait_window for a window that never appears fails, naming the title.
- `launch-argv` — launch passes argv and the environment exactly.
- `sequence-compatible` — A GUI suite runs inside a suite sequence and hands off at READY.
- `fixtures-pinned` — The session's fixture tree, fonts and locale match their pinned hashes.

#### `diagnosis` — saved evidence & failure messages (19)
- `artifacts-on-mismatch` — A mismatch saves the got image, diff image, tree, input log and screen.
- `nothing-on-pass` — A passing case saves nothing unless saving is turned on.
- `save-all` — With saving turned on, a passing case saves its frames.
- `diff-image` — The diff image marks exactly the differing pixels.
- `got-image` — The saved got image decodes to the captured frame.
- `tree-at-commit` — The saved tree is the one from the compared commit: under the real runner in the fixture, a case whose window check fails is killed before it writes any artifact, after the app has committed 30 more frames with different trees, and the app is then killed; the saved tree matches the compared commit's id and tree hash from the arena record, not the app's last commit or its remnant.
- `input-log` — The saved input log lists every serial with its disposition.
- `screen-saved` — The saved screen is the readback at the failing step, with its record.
- `cap` — Past the artifact cap, a failure records artifacts=none:cap and keeps its primary message.
- `io-error` — An artifact write that fails records artifacts=none:io, and the primary failure stands.
- `path-in-record` — The FRAME record says what was saved and where.
- `synced` — Artifacts are on disk before the CASE line is written.
- `message-pixel` — A frame failure's message names the check, the first pixel and both colours.
- `message-tree` — A tree failure's message names the node key and the field.
- `message-input` — A delivery failure's message names the serial and its disposition.
- `case-death` — When the case process is killed, the runner sends `end collect`, waits for the supervisor's `done` before sweeping, and the evidence is on disk.
- `failed-screen-retained` — The screen, frame and tree saved for a failing check are the ones captured at the failure, after 30 later commits and composites, the case's death before it wrote artifacts, and the window owner's death; a slot left `writing` by the killed case is recorded `partial:case-died` and never saved as evidence.
- `app-death` — An app that dies right after a commit still has that commit and tree saved, from its remnant.
- `reserve` — Evidence collection finishes inside the reserve, before the runner's limit.

#### `proof` — proven on bwm & a test app (26)
Each fault case runs its scenario in the inner executor twice: clean, which must pass, then with the fault,
which must fail with the stated class for the stated reason.
- `clean-passes` — The test app with no fault passes every check of a full script.
- `fault-off-by-one` — A frame drawn one pixel off fails as pixel-mismatch at the first shifted pixel.
- `fault-wrong-colour` — A widget drawn in the wrong colour fails as pixel-mismatch with both colours.
- `fault-drain-no-render` — An app that drains a key and never commits fails as unprocessed-serial for that key.
- `fault-stale-pixels` — An app that updates its tree but not its pixels fails as pixel-mismatch.
- `fault-drop-key` — An app that drops one key fails as tree-mismatch on the text field's value.
- `fault-misroute` — bwm-fault_test sending keys to the wrong window fails as pixel-mismatch and tree-mismatch on the intended window.
- `fault-chrome-offset` — bwm-fault_test drawing chrome one pixel off fails as pixel-mismatch against the contract table.
- `fault-keymap-swap` — bwm-fault_test with two keys swapped fails as tree-mismatch on typed text.
- `fault-lie-bounds` — A tree whose bounds disagree with the drawn widget fails as tree-mismatch through a role invariant.
- `fault-lie-text` — A label drawn "FAIL" whose tree says "PASS", with valid ink and bounds, fails as pixel-mismatch.
- `fault-slow-commit` — An app that never settles fails as settle-timeout, naming it.
- `fault-wallclock` — An app animating on the real clock fails as clock-violation on every run.
- `fault-hang` — A hung app fails as app-hung when the 2 s step bound passes, instead of hanging the suite.
- `fault-crash` — A crashing app fails as app-crashed, naming the signal.
- `fault-trail` — A cursor trail fails as pixel-mismatch in a screen check.
- `fault-lost-disposition` — bwm-fault_test leaving a serial without a disposition fails as missing-disposition.
- `fault-real-input` — bwm-fault_test that ignores the session's device selection fails when a non-selected device types.
- `form-scenario` — Filling the test app's form (text field, checkbox, slider, submit) gives the expected tree and frame.
- `menu-scenario` — Opening a menu and choosing an item gives the expected tree and frame.
- `list-scenario` — Scrolling and selecting in a list gives the expected tree and frame.
- `animation-scenario` — Stepping the test app's animation gives its reference frames at epochs 1, 10 and 60.
- `wm-focus-scenario` — A scripted focus scenario on bwm gives the expected trees and screens.
- `wm-stack-scenario` — A scripted stacking scenario on bwm gives the expected trees and screens.
- `wm-resize-scenario` — A scripted resize of the test app gives the expected trees and screens.
- `wm-clipboard-scenario` — Copying between two test apps through bwm gives the expected text fields.

## 18. Suite `gui-apps`

Manifest: id `gui-apps`, title "GUI applications", area "GUI applications", `"timingSensitive": true`, binary
`/sbin/suite-gui-apps`, source `suite_gui_apps.rs`. Summary: *"bterm, blauncher, bfontpicker, blog with
blogd, Gus Kit and bcheck, and the graphics demos, do what their users rely on (commands run, programs launch,
fonts change, logs follow, drawings save and synchronize, checks report truthfully), driven by real input on a
real desktop, on every target."*

The effort is deliberately larger than a typical one. It keeps one milestone per application, separate
milestones for terminal emulation and for Gus Kit's collaboration, and integration last. Splitting
applications into separate efforts was considered and rejected by the operator's ruling. Runtime is handled
by gate sizing (section 19), not by splitting.

| Milestone | Title | Categories |
|---|---|---|
| `terminal-emulation` | bterm terminal emulation | `terminal-emulation` |
| `bterm` | bterm runs shell sessions | `bterm` |
| `blauncher` | blauncher finds & launches programs | `blauncher` |
| `bfontpicker` | bfontpicker sets system fonts | `bfontpicker` |
| `blog` | blog & blogd show the logs | `blog` |
| `guskit` | Gus Kit paints & saves | `guskit` |
| `guskit-collab` | Gus Kit collaboration | `guskit-collab` |
| `bcheck` | bcheck reports truthfully | `bcheck` |
| `demos` | graphics demos in windows | `demos` |
| `desktop` | the whole desktop together | `desktop` |

Not GUI applications, and not planned here (the applications recon): btop and bless are terminal workloads that
bterm's cases run; burl, counter, spinner, heartbeat, resolution, tones and fart stay in their utility, audio
or diagnostic domains; `system_fonts.rs` is a stale helper to delete. Probe and the suite panels are covered by
`breengel/panel`.

**Work this effort needs first, with the decisions it settles:**

- **All apps:** libuitest throughout; `BREENIX_CONFIG_ROOT`; deterministic startup arguments (`bterm -e PROG
  ARGS`, `blog FILE...`, `guskit --canvas WxH`, `bfontpicker`, `bcheck --catalog FILE`); semantic keys for
  custom-drawn content (bterm's grid as a `terminal` node, Gus Kit's canvas, palette and status, blog's lines
  and status, bcheck's rows); `commit_no_change` for events that change nothing; handling of
  `CompositorChanged`. The recon's app defects are each fixed with the case that fails today.
- **Fixtures** in `/usr/local/test/share/gui-apps/` (BMP and PNG images including malformed ones, valid and
  malformed fonts, log files of set sizes, launcher catalogs) and these helpers:
  - `pty-job_test`: acknowledges exec, its pid, process group, session and whether it is in the
    foreground, on a side channel; records each signal it receives; prints sequence-numbered lines on command;
  - `vt-script_test`: writes a named escape-sequence script to its terminal in a given chunking;
  - `ack-app_test`: records its argv, working directory and environment, then opens a window or exits;
  - `log-producer_test`: writes uniquely numbered records to `/dev/kmsg` or to a file;
  - `bcheck-fixture_test`: children that pass, exit 1, exit 127, die by a signal, hang, flood output, print
    forged PASS or SUITE text, or leave grandchildren;
  - `collab-peer_test`: a raw protocol peer that writes exact bytes and can stop reading.
- **bterm.** Its contract is `docs/gui/bterm.md`. It installs a pinned terminfo entry `bterm` and sets
  `TERM=bterm`. The advertised capabilities are exactly: `am`, `xenl`, `cols`, `lines`, `colors#256`, `bold`,
  `rev`, `smul`, `rmul`, `sgr0`, `sgr`, `setaf`, `setab`, `op`, `cup`, `home`, `clear`, `cuu`, `cud`,
  `cuf`, `cub`, `cuu1`, `cud1`, `cuf1`, `cub1`, `el`, `el1`, `ed`, `csr`, `ind`, `ri`, `il`, `dl`, `il1`,
  `dl1`, `ich`, `dch`, `ech`, `sc`, `rc`, `civis`, `cnorm`, `smcup`, `rmcup`, `smkx`, `rmkx`, `kcuu1`,
  `kcud1`, `kcuf1`, `kcub1`, `khome`, `kend`, `kdch1`, `kpp`, `knp`, `kbs`, `kf1` to `kf4`, `hts`, `tbc`, `ht`,
  `cr`, `nel`, `bel`. Each one is exercised through the PTY by `terminal-emulation/terminfo-caps`. Character
  cells: each printable codepoint takes one cell; combining marks (general categories Mn and Me) are not
  displayed; wide characters are drawn in one cell. These are stated limits, and nothing in the terminfo entry
  claims otherwise. Parser bounds: at most 16 CSI parameters, each clamped to 9,999; a sequence longer than
  256 bytes or an OSC longer than 512 bytes is discarded; CAN and SUB abort a sequence; invalid UTF-8 becomes
  one U+FFFD per maximal invalid subpart. Scrollback keeps 1,000 lines. Mouse selection copies through the
  bwm clipboard. A tab is either a **shell tab** (Ctrl+T, and the default first tab, so a new window shows a
  prompt) or an **exec tab** (`-e`). An exec tab shows its child's exit status and stays until closed.
  Closing a tab sends SIGHUP to its session. btop is no longer the first tab. bterm propagates its size with
  `TIOCSWINSZ`, which sends SIGWINCH to the foreground process group. **Streaming witnesses:** the `terminal`
  node publishes, in each commit, `stream-digest`, a running SHA-256 over every byte bterm has consumed from the
  PTY in order, and `rows-retired` with `rows-digest`, a running SHA-256 over the text of each row as it leaves
  the top of the screen into scrollback (trailing spaces removed, each row followed by a newline). Both are
  updated by the code that consumes and parses, and are published only through commits. The throughput case
  computes the expected values independently from the generator's definition of the 1 MiB, at any byte count.
  SHA-256 is libpixcheck's, certified by its known-answer tests (section 15, foundation item 5).
- **blauncher.** Entries are `.app` files in `/usr/share/applications/`, owned and specified by this effort
  (`docs/gui/app-entry.md`). Fields: `name`, `description`, `exec` (an argv array), `cwd`, `mode` (`gui` or
  `terminal`), `hold` (keep a terminal tab after the command exits) and `icon`. The built-in catalog becomes
  installed entries. A malformed, duplicate or stale entry is listed in an errors section and the rest still
  load. This effort also adds an `--app-entry` option to the SDK's install tool. That is new work measured
  here: the completed SDK effort's cases do not depend on it.
- **Saving, for bfontpicker and Gus Kit alike.** A save writes a temporary file in the same directory in full
  (retrying short writes), `fsync`s it, `rename`s it over the destination, then `fsync`s the directory. The
  `rename` is the commit point, and the contract states what holds on each side of it:
  - **Before `rename` succeeds** (the write, a short write, the file `fsync` or the `rename` itself fails): the
    destination still names the old file, untouched. The app removes the temporary file, keeps its unsaved
    state, and shows the error.
  - **After `rename` succeeds and the directory `fsync` fails:** the destination names the new file, and the old
    contents are gone. Nothing is rolled back, because a rollback would be a second unsynced rename. The app
    shows "saved, but not confirmed on disk" with the error, keeps the document marked unsaved (so quitting
    still asks), and retries the directory `fsync` on the next save. bfontpicker shows the new font as applied
    but unconfirmed.
  - **Interrupted** (the process killed between any two steps): after a restart the destination is the whole
    old file or the whole new file, never a mixture, and the app removes any temporary file it left behind at
    startup.
- **bfontpicker.** It saves fonts.conf by the protocol above and keeps settings it does not know. It reads font
  categories from the `post` table's `isFixedPitch`. Mono and display choices are independent.
- **blog and blogd.** The kernel gains a sequence-aware log reader with `/dev/kmsg` semantics: each record has
  a sequence number, each reader descriptor has its own cursor, and a reader overtaken by the ring gets EPIPE
  once and then continues from the oldest record, which reports exactly how many it lost. A root `write` to
  `/dev/kmsg` appends a record. The kernel exposes a boot id, a random 128-bit value chosen at boot, at
  `/proc/sys/kernel/random/boot_id` (added if absent). blog's Serial tab is removed until a real producer
  exists. Search uses `/`, `n` and `N`.
  **blogd's append protocol** makes each record appear exactly once, without a separate cursor file whose update
  could disagree with the log:
  - Each line of `kernel.log` starts with the boot id and the record's sequence number. The log's last complete
    line is the cursor.
  - On start, and again before retrying after any failed write or `fsync`, blogd reads the tail of
    `kernel.log`, truncates a partial last line (one with no newline) back to the last complete line, and
    `fsync`s. If the last complete line carries this boot's id, it resumes from the following sequence number.
    Otherwise it starts from the oldest record in the ring.
  - It appends whole lines in sequence order, retries short writes, and `fsync`s after each batch. An overrun
    is logged as its own line, `<boot id> <seq> lost=<n>`.
  - Death at any point therefore leaves either a complete line, which recovery skips, or a partial one, which
    recovery truncates. No record is duplicated, and no record is lost unless the ring overran it, in which case
    the loss is logged with its count.
  - blogd sets SIGXFSZ to `SIG_IGN` at startup, so a file-size limit surfaces as EFBIG (section 11).
- **Gus Kit.** Undo and Redo become real, with 32 steps. Save As uses a text field. PNG open and save are
  added. Images larger than the canvas are cropped with a visible notice. Saves follow the saving protocol
  above, and Gus Kit sets SIGXFSZ to `SIG_IGN` at startup. The save directory is under the config root, and file
  names never collide across sessions. **Collaboration:** the host sequences every operation. A peer sends an
  operation with its id (peer id and local sequence); the host either admits it, giving it a global sequence,
  broadcasting it and sending the sender `ack(local sequence, global sequence)`, or refuses it with
  `refused(local sequence, reason)`. Every peer, the sender included, applies operations in global order. Each
  peer publishes in its tree every id it submitted and the ack or refusal each got, so an operation lost before
  admission shows up as a submitted id with no answer. Undo reverts the issuing peer's newest operation by
  replaying the admitted history without it, and a peer cannot undo another peer's operation. Each peer's
  output queue is bounded at 1 MiB. A full queue disconnects that peer with a stated reason, and the others
  continue. A reconnecting peer gets a full resync, and its unanswered ids are answered `refused(disconnected)`.
  The host publishes its operation log, queue sizes and its `partial-writes` and `eagain` counts in its tree.
- **bcheck.** The child supervision core (fork, deadline, process-group kill, wait-status decoding,
  exec-error pipe, bounded output capture) is extracted from `libbreenix::suite` into a shared crate used by
  both the suite runner and bcheck. bcheck keeps its own UI and catalog format, has no SKIP, polls input while
  tests run, and exits non-zero when any test failed. The production catalog's network test reads its URL from
  the catalog file, which the suite points at a loopback HTTP fixture in the guest.
- **Demos.** demo, particles and confetti are ported to Breengel windows, which retires the half-screen
  `fb_mmap` pane. Bounce and Rectangles drop their legacy fallbacks under a compositor. Bounce handles
  CloseRequested, and its speed factor saturates at 1/4 and 4 (section 12.3).
- **Saving and persistence.** These cases prove process-restart persistence and the exact syscall sequence,
  with forced failures at each step. That the bytes survive a reboot after a successful `fsync` is the
  `files-io` effort's contract, and no GUI case re-proves it with host disk extraction.

#### `terminal-emulation` — bterm terminal emulation (44)
Each case writes a fixed byte sequence through `vt-script_test` and compares the `terminal` node's cell text
and attributes with an independent literal expectation, plus selected pixels where they matter. Each case runs
the sequence three ways: whole, byte by byte, and in seeded random chunks.
- `crlf` — CR returns to column 0 and LF moves down one row.
- `backspace` — BS moves left one column without erasing.
- `tab-stops` — HT moves to the next stop, and HTS and TBC set and clear stops.
- `autowrap` — Text past the last column wraps to the next row.
- `pending-wrap` — Writing the last column leaves the cursor pending there, and a CR or cursor move cancels the wrap.
- `autowrap-off` — With DECAWM off, text past the last column overwrites it.
- `cursor-moves` — CUU, CUD, CUF and CUB move by their counts and stop at the margins.
- `cup` — CUP places the cursor at the 1-based row and column.
- `erase-line` — EL 0, 1 and 2 erase to the end, to the start and the whole line.
- `erase-display` — ED 0, 1 and 2 erase below, above and the whole screen.
- `scroll-at-bottom` — LF on the last row scrolls the screen up one line.
- `sgr-bold` — SGR 1 sets bold, and SGR 22 ends it.
- `sgr-16` — SGR 30-37, 40-47, 90-97 and 100-107 select the 16 colours.
- `sgr-256` — SGR 38;5 and 48;5 select from the 256-colour table.
- `sgr-truecolor` — SGR 38;2 and 48;2 select exact RGB.
- `sgr-inverse` — SGR 7 swaps foreground and background, and SGR 27 swaps them back.
- `sgr-underline` — SGR 4 underlines, and SGR 24 ends it.
- `sgr-reset` — SGR 0 resets every attribute.
- `cells-attributes` — A scripted screen's every cell text and attribute equals the literal expectation.
- `private-modes` — CSI ? sequences are parsed, so "ESC[?25l" prints nothing.
- `cursor-visibility` — DECTCEM hides and shows the cursor.
- `alt-screen` — Mode 1049 switches to a blank screen and back to the saved one.
- `alt-screen-resize` — A resize while on the alternate screen resizes both screens, and leaving it restores the main one.
- `scroll-region` — DECSTBM confines scrolling to the region.
- `index` — IND and RI scroll at the region's edges.
- `insert-delete-line` — IL and DL insert and delete lines inside the region.
- `insert-delete-char` — ICH and DCH insert and delete characters in the line.
- `erase-char` — ECH erases characters without moving the cursor.
- `save-restore` — DECSC and DECRC, and CSI s and u, restore the cursor and attributes.
- `dsr` — CSI 6n replies with the cursor position.
- `da` — CSI c replies with the documented device attributes.
- `osc-title` — OSC 0 sets the tab's title.
- `osc-bounded` — An OSC longer than 512 bytes is discarded, and the terminal recovers.
- `csi-bounded` — Too many or too large CSI parameters, or an overlong sequence, are handled as the contract says, and the terminal recovers.
- `can-sub` — CAN and SUB abort a sequence in progress.
- `unknown-ignored` — An unknown sequence is consumed without printing.
- `utf8-split` — A UTF-8 character split at any byte boundary prints once.
- `utf8-invalid` — Invalid UTF-8 prints one U+FFFD per maximal invalid subpart.
- `combining-policy` — Combining marks are not displayed, and the base character keeps its cell.
- `bracketed-paste` — With mode 2004 on, a paste is wrapped in the bracket sequences.
- `cursor-keys-mode` — Arrow keys send CSI sequences in normal mode and SS3 sequences in application mode.
- `key-encoding` — Home, End, Delete, Page Up, Page Down, F1 to F4 and Ctrl with a letter send what the terminfo entry says.
- `terminfo-caps` — Every capability in the installed bterm entry, written through the PTY, has its documented effect.
- `attribute-pixels` — Bold, inverse, underline and coloured cells match the certified renderer's pixels.

#### `bterm` — bterm runs shell sessions (37)
- `prompt` — A new bterm window shows the shell's fixed prompt in its first tab.
- `command-output` — Typing "echo hello" and Enter shows "hello" on the next row.
- `output-pixels` — The grid after a fixed command sequence matches its reviewed reference.
- `exit-status` — "false; echo $?" shows 1.
- `ctrl-c` — Ctrl+C ends a foreground pty-job_test that acknowledged it was running, and the shell reports status 130.
- `ctrl-z-fg` — Ctrl+Z stops the foreground job, seen stopped in /proc, and fg resumes it, acknowledged by the job.
- `ctrl-d` — Ctrl+D at the prompt ends the shell, and the tab shows that its child exited with status 0.
- `child-reaped` — An exited child leaves no zombie.
- `exec-tab-exit` — An exec tab whose program exits shows its exit status and stays open.
- `exec-tab-fail` — An exec tab whose program cannot run shows the error and status 127.
- `close-hups-descendants` — Closing a tab sends SIGHUP to its session, and a surviving descendant acknowledges it.
- `winsize-initial` — stty size reports the grid's rows and columns.
- `winsize-resize` — After a window resize, the foreground job acknowledges SIGWINCH and reads the new size.
- `resize-keeps-text` — A resize keeps the text in the surviving rows and columns.
- `new-tab` — Ctrl+T opens a tab running a new shell.
- `close-tab` — Ctrl+W closes the tab and ends its process group.
- `tab-isolation` — Output in one tab never appears in another.
- `tab-switch-mouse` — Clicking a tab shows that tab's grid.
- `tab-switch-keys` — Ctrl+Page Down and Ctrl+Page Up switch tabs.
- `tab-churn` — 50 tab open and close cycles return memory, descriptors and processes to the baseline.
- `font-size-keys` — Ctrl++ enlarges the cells, shrinks the grid and updates the size the PTY reports.
- `font-change-live` — A fonts.conf change redraws the grid with newly rasterized glyphs of the new font.
- `history` — Up recalls the previous command line.
- `paste` — Ctrl+Shift+V writes the clipboard text to the PTY.
- `select-copy` — A mouse selection and Ctrl+Shift+C put that text on the clipboard.
- `scrollback` — After 200 lines, scrolling up shows the earlier lines.
- `scrollback-limit` — Scrollback keeps the last 1,000 lines and drops older ones.
- `throughput` — For 1 MiB of numbered lines, the commit with `pty-consumed` = 1 MiB carries the independently computed stream and row digests, at least sixteen intermediate commits carry the matching prefix digests for their byte and row counts, and the final screen and scrollback hold the last lines.
- `throughput-fault-detected` — `bterm_test` built to drop one byte at 512 KiB, run in the inner executor, fails as tree-mismatch on `stream-digest` at the first sampled commit past that offset, after the clean bterm passes the same scenario.
- `throughput-time` — 1 MiB through the PTY is consumed and committed within 8 s.
- `unfocused-cursor` — The cursor draws hollow while the window is unfocused.
- `close-hups` — Closing the window sends SIGHUP to every tab's session.
- `term-env` — Children see TERM=bterm, and its terminfo entry is installed.
- `btop` — btop in an exec tab lists the fixture helpers by name, with values within tolerance of independent samples.
- `bless-pages` — bless pages through a file with Space and b.
- `bless-search` — bless finds text with / and n.
- `bless-quit` — q leaves bless and restores the shell tab's screen.

#### `blauncher` — blauncher finds & launches programs (30)
- `open` — A double tap of Super opens the launcher with the search field focused.
- `close-escape` — Escape closes it.
- `close-focus-loss` — Clicking another window closes it.
- `toggle` — A second double tap closes it.
- `all-reachable` — Every catalog entry can be reached by scrolling.
- `filter` — Typing filters entries by case-insensitive substring of the name.
- `filter-description` — Typing matches descriptions too.
- `no-results` — A query with no matches shows "No matches".
- `backspace` — Backspace widens the filter again.
- `query-limit` — The query takes 63 characters and refuses the 64th.
- `selection-keys` — Up and Down move the selection and stop at the ends.
- `launch-gui` — Enter on a GUI entry opens a window owned by the launched process and closes the launcher.
- `launch-argv` — An entry starts ack-app_test with exactly the entry's argv, working directory and environment.
- `launch-terminal` — A terminal entry runs that command in a bterm tab, shown by the command's acknowledgment and output.
- `launch-short-lived` — A terminal entry whose command exits at once shows its output and exit status in a held tab.
- `launch-missing` — An entry whose program is missing shows an error, the launcher stays open, and no zombie is left.
- `launch-no-window` — A GUI entry whose program never opens a window shows an error in the first epoch at or after the 5,000 ms UI-time launch deadline.
- `no-double-launch` — Two quick Enters launch one program.
- `double-click` — Double-clicking an entry launches it.
- `hover` — Hovering an entry selects it.
- `entry-format` — A well-formed .app entry is listed with its name, description and mode.
- `entry-malformed` — Malformed, duplicate and stale entries are listed as errors, and the rest load.
- `empty-catalog` — An empty catalog shows an empty state.
- `discovers-installed` — An app installed with the SDK's --app-entry option appears in the list and launches.
- `catalog-valid` — Every shipped entry's program exists and its mode matches the program.
- `border-animation` — The border animates by the stepped clock and matches its reviewed frames at epochs 1 and 30.
- `font-relayout` — A font change relays out the entries.
- `resize-relayout` — A resize relays out the entries.
- `reaped` — Programs the launcher started are reaped when they exit.
- `paste-query` — Ctrl+V pastes the clipboard into the query.

#### `bfontpicker` — bfontpicker sets system fonts (28)
- `lists-fonts` — Every TTF in the font directory is listed.
- `mono-by-metadata` — Fonts are sorted into mono and display by the post table's isFixedPitch, not the file name.
- `select-keys` — Up and Down move the selection and skip headings.
- `select-mouse` — A click selects a font.
- `apply-enter` — Enter applies the selected font.
- `apply-double-click` — A double click applies a font.
- `persists` — An applied choice is in fonts.conf with the expected bytes, and a restarted picker shows it.
- `atomic-write` — A reader during the write sees the old fonts.conf or the new one, never part of one.
- `keeps-unknown` — Settings the picker does not know survive its rewrite.
- `size-bounds` — Sizes stop at 6 and 48.
- `size-persists` — A size change is in fonts.conf after a restart.
- `unwritable` — Run as uid 1000 against a root-owned config, the picker shows an error and does not show the choice as applied.
- `write-failure-preserves` — A failure forced at the write, the file fsync or the rename leaves the old fonts.conf untouched, removes the temporary file and shows the error.
- `dirsync-failure` — A failure forced at the directory fsync leaves the new fonts.conf in place and in use, shown as applied but not confirmed on disk, and the next save retries the directory fsync.
- `write-interrupted` — The picker killed by `kill-after` after each step of a save leaves fonts.conf whole (old or new, never mixed), and the restarted picker removes the leftover temporary file.
- `bad-font` — A malformed TTF is listed as unusable with its error, and the picker keeps running.
- `missing-font` — A configured font whose file was removed is shown as missing, and apps keep their last valid font.
- `empty-dir` — An empty font directory shows an empty state.
- `preview` — The preview is drawn in the selected font and matches its reviewed reference.
- `live-bterm` — Applying a mono font makes bterm draw newly rasterized glyphs of that font.
- `live-blog` — Applying a mono font makes blog draw newly rasterized glyphs of that font.
- `live-guskit` — Applying a display font changes Gus Kit's menus and status.
- `live-chrome` — Applying a display font changes bwm's titles on every target.
- `independent-categories` — Changing the mono font leaves the display font unchanged, and the reverse.
- `repeated-changes` — Five changes in a row leave every app on the last one.
- `resize` — A resize relays out the list and preview.
- `wheel` — The wheel scrolls the list.
- `quit` — q and Escape quit.

#### `blog` — blog & blogd show the logs (35)
- `tail` — Lines a producer appends appear in order, exactly.
- `follow` — While following, new lines scroll into view.
- `follow-off` — With follow off, the view stays put as lines arrive.
- `partial-line` — An unterminated line is redrawn when it is completed.
- `same-count-change` — A change that keeps the line count is still redrawn.
- `cap-crossing` — A file growing past 256 KiB shows its newest lines and keeps reading from the right offset.
- `large-open` — Opening a 1 MiB file shows its last 256 KiB.
- `truncate` — A truncated file is reloaded from the start.
- `rotation` — A log replaced by rename is followed to the new file.
- `missing-file` — A missing file shows a waiting state and opens when it appears.
- `read-error` — A file the uid 1000 viewer cannot read shows its error in the status line.
- `tabs-independent` — Each tab keeps its own scroll and follow state.
- `tab-switch` — Tab and clicks switch tabs.
- `navigation` — j, k, Page Up, Page Down, Home, End, g and G move as documented.
- `line-numbers` — Line numbers match the file's lines.
- `long-lines` — A line longer than the view is cut with a marker.
- `wheel` — The wheel scrolls.
- `search` — / finds the next match and highlights it.
- `search-next` — n and N move between matches.
- `font-change` — A font change redraws with newly rasterized glyphs and recomputes the visible rows.
- `resize` — A resize changes the visible row count.
- `file-args` — blog FILE opens a tab for that file.
- `serial-tab-absent` — There is no Serial tab.
- `kmsg-cursor` — A /dev/kmsg reader gets each record once, in sequence order.
- `kmsg-overrun` — A reader overtaken by the ring gets EPIPE once, then continues and reports exactly how many records it lost.
- `kmsg-write` — A root write to /dev/kmsg appears as a record with the next sequence number.
- `blogd-kmsg` — blogd appends log-producer_test's numbered records to kernel.log in order, each exactly once, each line carrying the boot id and sequence number.
- `blogd-ring-wrap` — After the ring wraps, blogd still captures new records and logs the overrun count.
- `blogd-short-write` — After the case has proven the RLIMIT_FSIZE mechanism on its own file (section 11), a limit that a record crosses gives blogd's write the exact short count and its retry EFBIG, and blogd, which ignores SIGXFSZ, reports the error and is still the same running process. With the limit lifted, that process's next retry truncates the partial line and logs every record exactly once, and a restart afterwards duplicates none.
- `blogd-write-failure` — An append failure forced at the write or the fsync is reported, and the retry, after tail recovery, adds no duplicate line and no partial line.
- `blogd-restart` — A cleanly stopped and restarted blogd resumes after the log's last complete line with no loss and no duplicate.
- `blogd-killed` — blogd killed by `kill-after` after its data write, after its fsync, and after a write that RLIMIT_FSIZE cut short inside a line, then restarted each time, leaves every record exactly once and no partial line.
- `blogd-foreign-boot` — Started on a kernel.log whose last line carries another boot id, blogd starts from the ring's oldest record and duplicates nothing of this boot.
- `blogd-desktop` — In a desktop session, blogd's startup records reach kernel.log and blog's view.
- `close` — q and the close button quit.

#### `guskit` — Gus Kit paints & saves (38)
Expected canvases are computed by the test crate from the contract's literal tool rules and the certified
libgfx primitives, never by Gus Kit's own code.
- `pencil` — The pencil draws a one-pixel stroke along the dragged path.
- `brush-sizes` — Brush sizes 1, 2 and 3 draw their documented widths.
- `line` — The line tool draws a Bresenham line from press to release.
- `rectangle` — The rectangle tool draws its outline from press to release.
- `circle` — The circle tool draws a circle centred at the press.
- `fill` — Flood fill fills exactly the connected region.
- `fill-boundary` — Flood fill stops at a one-pixel boundary, with diagonals treated as documented.
- `eraser` — The eraser paints the background colour.
- `colour-picker` — The HSV controls set the documented colour.
- `recent-colours` — Used colours enter the recent list, most recent first.
- `preview-cancel` — Escape during a shape preview leaves the canvas unchanged.
- `focus-loss-cancel` — Losing focus mid-stroke ends the stroke cleanly.
- `stroke-clip` — Strokes leaving the canvas are clipped at its edge.
- `undo` — Undo reverts the last operation exactly.
- `redo` — Redo reapplies it exactly.
- `undo-limit` — Undo goes back at most 32 steps.
- `menu-undo-enabled` — Undo and Redo are enabled exactly when there is something to undo or redo.
- `new` — New clears the canvas after confirming unsaved changes.
- `save` — Save writes a BMP whose bytes equal the independently expected encoding of the canvas.
- `save-png` — Saving with a .png name writes a PNG that the certified decoder reads back as the canvas.
- `save-as` — Save As writes to the typed name.
- `save-unique` — Saves in a new session never overwrite an earlier session's file.
- `save-fail-before-rename` — A failure forced at the write, the file fsync or the rename shows the error, leaves the old file untouched, removes the temporary file and keeps the unsaved state.
- `save-fail-after-rename` — A failure forced at the directory fsync leaves the new file in place, shows it as saved but not confirmed on disk, keeps the document marked unsaved, and the next save retries the directory fsync.
- `save-interrupted` — Gus Kit killed by `kill-after` after each step of a save leaves the file whole (old or new, never mixed), and the restarted Gus Kit opens it and removes the leftover temporary file.
- `save-short-write` — After the case has proven the RLIMIT_FSIZE mechanism on its own file (section 11), a short write and then EFBIG are reported as a failed save before the rename, the old file is intact, and Gus Kit, which ignores SIGXFSZ, keeps running; with the limit lifted, the same process's next save completes and the file decodes to the canvas.
- `save-error-permission` — Run as uid 1000 against a read-only directory, saving shows an error in the window.
- `restart-persistence` — A file saved by one Gus Kit process opens in a new one as the same canvas.
- `open-bmp` — Opening a BMP shows its pixels.
- `open-png` — Opening a PNG shows its pixels.
- `open-malformed` — Opening a malformed image shows an error and keeps the canvas.
- `open-oversize` — Opening an image larger than the canvas crops it and says so.
- `shortcuts-file` — Ctrl+N, Ctrl+O, Ctrl+S and Ctrl+Q do their menu actions.
- `shortcuts-tools` — P, B, L, R, C, F, E and 1, 2, 3 select tools and sizes.
- `picker-blocks-shortcuts` — Tool shortcuts do nothing while the file picker is open.
- `picker-keys` — The file picker is driven by arrows, Enter and typed paths.
- `resize-keeps-canvas` — Shrinking and regrowing the window loses no canvas pixels.
- `quit-unsaved` — Quitting with unsaved changes asks first.

#### `guskit-collab` — Gus Kit collaboration (31)
Peers are Gus Kit instances in the same guest on 127.0.0.1. Every peer's canvas is compared with an expected
canvas the test crate replays from the operations **the script submitted**, in the global order the host
reported. Every submitted id must appear exactly once, admitted in the host's log or refused with a reason, so an
operation lost before admission cannot drop out of the expectation.
- `join` — A second instance joins a host.
- `initial-sync` — A joiner receives the host's canvas exactly.
- `stroke` — A stroke on one peer appears on the other, exactly.
- `global-order` — Operations from two peers released at a barrier are applied by every peer in the host's sequence order.
- `no-loss-no-dup` — Every submitted operation is answered, and every admitted one is applied exactly once on every peer.
- `submitted-acked` — Each peer's tree lists every id it submitted with its ack and global sequence or its refusal reason, and the lists equal the script's submissions.
- `lost-before-admission-detected` — `guskit_test` built with `drop-before-admit`, run in the inner executor, fails naming the submitted id that was never answered, after the clean Gus Kit passes the same scenario.
- `three-peers` — Three peers converge to the replayed canvas.
- `late-join` — A peer joining after drawing receives the current canvas.
- `clear` — New on one peer clears every peer.
- `open` — Opening an image on one peer replaces every peer's canvas.
- `undo` — Undo on one peer reverts that peer's newest operation on every peer.
- `undo-ownership` — A peer cannot undo another peer's operation.
- `remote-cursor` — Each peer shows the others' cursors at their positions.
- `peer-leaves` — When a peer leaves, the host keeps running and the others are told.
- `host-leaves` — When the host leaves, the peers say they are disconnected.
- `short-send` — With `short-send` forcing one partial send, the host's `partial-writes` count rises by exactly 1, sends the rest, and the peer receives the operation whole and once.
- `eagain` — With `eagain` forcing *n* refused sends, the host's `eagain` count rises by exactly *n*, and the queued operations arrive in order after POLLOUT.
- `queue-limit` — A peer that never reads is disconnected at the 1 MiB queue limit with a reason, and the others continue.
- `fragmented-frames` — Messages delivered one byte at a time are parsed correctly.
- `coalesced-frames` — Several messages in one read are parsed correctly.
- `invalid-length` — A message with an invalid length disconnects its sender, and the session continues.
- `invalid-type` — A message with an unknown type disconnects its sender, and the session continues.
- `invalid-coords` — An operation with out-of-range coordinates is refused without changing any canvas.
- `sequence-gap` — A gap or duplicate in a peer's sequence disconnects it.
- `disconnect-during-sync` — A joiner that disconnects during the initial sync leaves the host and others unaffected.
- `reconnect-resync` — A peer that reconnects receives a full resync.
- `version-mismatch` — A peer with another protocol version is refused.
- `client-limit` — A sixteenth client is refused.
- `large-sync` — Syncing a 1024x768 canvas finishes within 2 s.
- `canvas-fixed` — Resizing a window in a session keeps the shared canvas size.

#### `bcheck` — bcheck reports truthfully (24)
- `all-pass` — Fixture children that pass are shown passing.
- `nonzero-exit` — A child exiting with status 1 is shown failing.
- `signal` — A child killed by a signal is shown failing, naming the signal.
- `exec-failure` — A child whose program cannot run is shown as an exec failure.
- `exit-127` — A child that itself exits with status 127 is shown as exit 127, not as an exec failure.
- `spawn-failure` — A fork failure forced by test control is shown failing, never skipped.
- `hang` — A child that hangs fails at its deadline and is killed and reaped.
- `descendants` — A test's grandchildren are killed and reaped when the test ends.
- `output-capped` — A child flooding output has its capture bounded at 64 KiB, and the window stays responsive.
- `forged-output` — A child printing PASS or SUITE text does not change its result.
- `responsive` — Scrolling and closing work while a test runs.
- `cancel` — Closing during a run ends and reaps the running child.
- `totals` — The summary's totals equal the rows.
- `exit-status` — Closed through real input after a run with a failure, bcheck exits non-zero.
- `serial-agrees` — The serial summary equals the window's summary.
- `headless-agrees` — A headless run gives the same results as the window and the independent expected set.
- `wheel` — The wheel scrolls the results.
- `scrollbar` — Dragging the scrollbar scrolls the results.
- `resize` — A resize changes the visible rows.
- `progress` — The progress bar advances with each finished test.
- `failure-output` — A failed test's row shows its last output line.
- `catalog-fixture` — --catalog runs exactly the fixture catalog's tests.
- `production-catalog` — The production catalog, with its network test pointed at the loopback fixture, passes every test.
- `rerun` — Running again gives the same results.

#### `demos` — graphics demos in windows (24)
- `bounce-frames` — Bounce's frames at epochs 1, 30 and 120 match their reviewed references.
- `bounce-close` — Bounce exits on CloseRequested.
- `bounce-click-sphere` — Clicking a sphere changes only that sphere's velocity.
- `bounce-click-empty` — Clicking empty space gives every sphere the documented impulse.
- `bounce-speed-keys` — + and - double and halve the speed factor and saturate at 4 and 1/4 without overflow.
- `bounce-speed-wheel` — Each wheel step doubles or halves the speed factor within the same limits.
- `bounce-resize` — After a resize, spheres stay inside the new bounds.
- `bounce-invariants` — Over 600 epochs spheres stay inside the window and never overlap past the contract's tolerance.
- `rectangles-window-path` — Rectangles uses the window path, and its frame at epoch 30 matches its reference.
- `rectangles-events` — Rectangles exits on close and adapts to a resize.
- `biconkit-scene` — biconkit's first frame matches its reviewed reference.
- `biconkit-hover` — Hovering and pressing an icon in biconkit changes its state as libicon specifies.
- `biconkit-keys` — Comma and period move biconkit's selection.
- `biconkit-resize` — biconkit relays out on a resize.
- `biconkit-idle` — biconkit stops committing when nothing animates.
- `demo-window` — demo runs in a window, and its frame at epoch 60 matches its reference.
- `particles-window` — particles runs in a window, and its frame at epoch 60 matches its reference.
- `particles-bounded` — No particle leaves the window in 600 epochs.
- `confetti-window` — confetti runs in a window, and a pointer move bursts at the pointer.
- `confetti-bounded` — The particle count never exceeds 200.
- `demos-close` — Every demo exits on CloseRequested.
- `no-takeover` — No demo takes over the display.
- `no-fallback` — Under a compositor no demo calls fb_mmap, by the kernel's per-process counter.
- `launcher-entries` — Every demo's launcher entry opens its window.

#### `desktop` — the whole desktop together (16)
- `init-desktop` — The real /sbin/init desktop, run as a child, puts bwm and a bterm prompt on screen, and reports that as a child it skipped the PID-1 test-control check.
- `launch-each` — Every launcher entry gives its entry-specific outcome: a window for GUI entries, an acknowledged command for terminal entries.
- `all-open` — With every app open at once, each still processes input and commits.
- `font-everywhere` — A font applied in bfontpicker makes bterm, blog, Gus Kit, blauncher and the chrome draw newly rasterized glyphs.
- `copy-terminal-to-launcher` — Text copied in bterm pastes into the launcher's query.
- `copy-field-to-terminal` — Text copied from a text field pastes into bterm.
- `close-all-baseline` — Closing every app returns windows, processes and memory to the baseline.
- `wm-restart` — After a bwm restart with every app open, each app receives input and commits.
- `crash-isolated` — One app crashing leaves the others running and responsive.
- `focus-cycle-all` — Alt+Tab visits every open app.
- `resize-every-app` — Every app handles a resize, and its tree fits its window.
- `alt-q-every-app` — Every app exits on Alt+Q.
- `tree-every-app` — Every app publishes a root window node and its focusable controls.
- `logs-show-desktop` — blog shows the known records of the desktop's own start.
- `save-survives` — A drawing saved in Gus Kit opens again after Gus Kit restarts.
- `idle-all-apps` — With every app open and idle, the session uses at most 5% of one CPU.

## 19. Gate sizing and runtime

- **Case limits.** The default is 10 s. A case whose workload is longer gets a **named allowance** in the suite
  table and the manifest: the workload plus startup plus the 1.5 s collection reserve, rounded up. The runner's
  stall detection uses the same per-case limit. Planned allowances:

  | Suite | Case | Allowance |
  |---|---|---|
  | `breengel` | `font-parser/corpus`, `inflate/byte-split`, `png-decode/independent-fixtures`, `windows/churn`, `composition/present-rate` | 20 s |
  | `window-manager` | `performance/*`, `liveness/progress-under-load`, `lifecycle/title-flood` | 20 s |
  | `window-manager` | `liveness/stress-animation`, `liveness/stress-io` | 30 s |
  | `window-manager` | `liveness/memory-bounded`, `lifecycle/launch-close-50` | 40 s |
  | `ui-testing` | every `proof/*` case (two scenario runs each) | 20 s |
  | `ui-testing` | `sessions/twenty-sessions` | 60 s |
  | `gui-apps` | `bterm/throughput`, `bterm/throughput-time`, `guskit-collab/large-sync`, `guskit-collab/lost-before-admission-detected`, `demos/bounce-invariants` | 20 s |
  | `gui-apps` | `bterm/throughput-fault-detected` (two 1 MiB scenario runs) | 30 s |
  | `gui-apps` | every `desktop/*` case | 30 s |
  | `gui-apps` | `bterm/tab-churn` | 40 s |
  | `gui-apps` | `bcheck/production-catalog` | 60 s |

  An allowance is added only when the plan names the workload that needs it, and it never stands in for a
  fix to a slow path.
- **Gate timeouts.** breenix-runs stores x86-64 evidence only when a suite finishes inside its gate timeout,
  and `--gate-timeout` already overrides the 420 s default (files-io runs with 1800 s). Each GUI suite's
  timeout is set from measurement: once `ui-testing/sessions/start-desktop` and one typical case of each
  suite exist, their cost is measured on x86-64 and ARM64 QEMU. The timeout is then twice the projected full
  suite duration, rounded up to the next 300 s, and is recorded in the suite's run command in
  `docs/suites/README.md`. Planning figures until then: `breengel` 900 s, `window-manager` 1800 s,
  `ui-testing` 1800 s, `gui-apps` 3600 s. The same timeout applies on every target.
- **One full suite per effort.** Each milestone's evidence is a full boot of its suite, the protocol every
  suite uses. Vigil keeps the newest result per suite and platform, so category-filtered boots are not an
  evidence path. If a suite ever has to be sharded, the plan first defines coverage, definition identity and
  aggregation, and a passing subset never completes a category it omitted. Splitting the applications effort
  to save time is not an option: the operator ruled it one effort.

## 20. Risks, and what to check first

### 20.1 Per-target display risks

| Target | Risk | Mitigation, and what to check first |
|---|---|---|
| Parallels | Composition happens in host GPU memory, and the driver has never issued `TRANSFER_FROM_HOST_3D`. #454 (soft lockups) and #457 (second-window corruption) are open but unreproduced at this snapshot. | **The entry gate (section 4.2) decides first.** If device readback fails, Parallels moves to the software compositor; the risk then becomes CPU composition cost, held to the `performance` bounds like every target. #454 and #457 need fresh specimens before anyone attributes a new failure to them. |
| VMware | Client pixels are never composed (#1047). The mode was observed at 1920x1200 against a vmx asking for 1280x960. There is no host capture. | The software compositor fills the screen target's surface. **Check first:** the 1280x800 mode set and `READBACK_GB_SURFACE`, which pick the observation row. |
| ARM64 QEMU | No compositor backend today; the kernel cursor leaves trails on the panels (#1338). | The software compositor writes the 2D resource, and the panel cursor gets save-and-restore (`breengel/panel`). Low risk. |
| x86-64 | No window interface at all; `copy_to_display` masks IRQs across a whole-screen copy (#1299); the pixel format may be RGB or BGR; userspace is soft-float. | The interface becomes arch-neutral; the compositor copies damaged bands within 500 µs of IRQ masking; readback converts to canonical BGRX. **Check first:** 1280x800 through `virtio-vga,xres=1280,yres=800`, and that adding `virtio-tablet-pci` leaves every existing x86 suite's verdict unchanged. The Tier-1 edit to `kernel/src/syscall/handler.rs`, if the dispatch needs it, follows the Tier-1 policy. |

### 20.2 Performance of headless rendering

- **Soft-float x86 userspace.** libfont rasterizes in f32, which runs in software on x86-64. If
  `text/raster-identical` or the `performance` bounds fail, the rasterizer moves to 26.6 fixed point, which
  also makes cross-target identity a property of integer arithmetic. The bounds are not relaxed instead.
- **Frame work in the guest.** Comparing a 1280x800 frame reads 4 MB, and decoding references is the slowest
  step. References are kept at window size, decoded references are cached within a case, and
  `frames/compare-cost` holds the cost to 50 ms.
- **Software composition** of a full 1280x800 frame costs a few milliseconds of copying on HVF. Damage-bounded
  composition keeps steady state far below that, and `performance/frame-floor` is the line to watch on x86-64.
- **Host contention and App Nap.** The UI clock takes timing out of every functional case. Only the
  `performance` cases are exposed, and they run under the shared-host rules like every timing-sensitive suite.

### 20.3 Decisions

- **Suite-mode test controls: decided.** The operator chose the suite-mode gate (section 3.4). Kernel fault
  points are runtime controls latched on by a PID-1 suite binary, every case runs the production kernel, and no
  fault kernel is built.
- **The glyph rasterization ceiling** (section 12.3) refuses a glyph whose computed bound exceeds 2²⁸ steps.
  It is a safety limit, estimated at about a hundred times what a 72 px glyph needs, and `text/size-range` checks the sizes the apps use by counter. If it ever refuses a glyph an app needs, a
  rasterizer with a smaller bound comes to the operator as a ruling; the ceiling is not raised quietly.
- **Session start at 300 ms** (section 12.2) is the bound the first design adopted. It stands until a measurement
  and a ruling say otherwise; the plan does not relax it on its own.
- **The case count** rises from 803 to 1,142. The increase is the coverage the critique and the re-check found
  missing: parser and codec corpora, observation faults, dispositions, clock boundaries, buffer replay, outer
  runner proofs, recovery and app failure paths. Library cases cost milliseconds each. Gate sizing (section 19)
  carries the rest.

### 20.4 Fonts and licensing

- *(checked against `fonts/LICENSE-*`)* The repo's fonts are under OFL-1.1 (the Google Fonts set, Cascadia
  Code, Inconsolata and JetBrains Mono), the Ubuntu Font Licence (Ubuntu, Ubuntu Mono), DejaVu's Bitstream Vera
  licence, and an MIT notice for Hack. All allow redistribution on the disk image. Rendered
  reference PNGs are documents, not font software. **Fix first:** `fonts/LICENSE-GoogleFonts` describes Hack as
  plain MIT, but Hack's licence also carries the Bitstream Vera terms for its derived glyphs.
- Tests depend only on DejaVu Sans Mono, DejaVu Sans, the built-in 5x7 font and noto-sans-mono-bitmap, all
  pinned by hash. A font update is a deliberate reference refresh.
- PNG and inflate are clean-room from RFCs 1950, 1951 and 2083; no zlib, miniz or lodepng source may be read
  or adapted. Fixture *files* produced by an independent encoder at authoring time, and the public PngSuite
  images, are data, not source. PngSuite's redistribution terms are recorded in the provenance file before the
  images are committed.

### 20.5 Blockers and triage

1. **The SDK effort.** At this snapshot `path.json` has the `sdk` effort, but there is no `docs/suites/sdk.json`
   or `suite_sdk.rs`, and SDK PRs were open. These efforts append after it either way.
   `blauncher/discovers-installed` builds on the SDK install tool's actual output.
2. **The four-target bar** is unreachable for window cases until the window interface and the software
   compositor land. Until then those cases are recorded FAILs on the targets that lack them, never SKIPs.
3. **Old issues to re-triage before work starts:** #424, #429, #431, #432, #441, #444, #447, #454, #455, #456,
   #457, #482 and #487. Each is reproduced against current main or closed with evidence. #989 is a close-out
   candidate after one confirming x86 boot. #930 and #917 stop being oracles, because host capture leaves pass
   or fail; they close once `scanout` passes on Parallels. #1047, #1299 and #1338 close with the `composition`,
   `composition/flush-irq-bound` and `panel` cases.
4. **Clean-main matrix.** Desktop mode and each GUI suite become rows as each exists, so main-health catches
   GUI regressions like any other.

## 21. What happens first

1. **Zero skips in the shared acceptance policy** (section 2.2), with its host-test fixtures, after checking
   that every existing suite's newest run has `skipped=0` on every target.
2. **The four entry spikes** (section 4.2), one boot each. Their results fill in section 4.1, including
   Parallels' production compositor.
3. **`docs/gui/breengel.md`**, the contract every `breengel` case cites.
4. **Two tracks in parallel:** the library track starting with `raster` (which fixes the `fill_rect` and
   `fill_circle` defects, the second of which writes out of bounds today), and the kernel track starting with
   the arch-neutral window interface and the commit protocol.
5. **The issue re-triage** in section 20.5, alongside.

## 22. JSON for `docs/efforts/path.json`

Append these four objects to `efforts`, after the `sdk` effort, in this order. *(checked)* Appended to a copy
of today's `path.json`, they parse, keep every effort id unique, use only the measure shapes the file already
uses (`{"suite", "categories"}`), name only the effort's own suite, and name only categories this plan
defines.

```json
[
  {
    "id": "breengel",
    "title": "BreenGL graphics library",
    "outcome": "Programs draw with BreenGL's 2D library (clipped and alpha-blended shapes and blits, TrueType and bitmap text, PNG and BMP images, widgets and animated icons) exactly to its written contract, and the windows they commit through an authority-checked kernel interface are composed and observed on the display pixel-exact on every target.",
    "suite": "breengel",
    "subsystems": [
      17,
      22,
      25
    ],
    "milestones": [
      {
        "id": "raster",
        "title": "shapes, blits, clipping & alpha",
        "measure": {
          "suite": "breengel",
          "categories": [
            "raster"
          ]
        }
      },
      {
        "id": "text",
        "title": "TrueType & bitmap text",
        "measure": {
          "suite": "breengel",
          "categories": [
            "text"
          ]
        }
      },
      {
        "id": "font-parser",
        "title": "font parsing safety",
        "measure": {
          "suite": "breengel",
          "categories": [
            "font-parser"
          ]
        }
      },
      {
        "id": "inflate",
        "title": "clean-room inflate",
        "measure": {
          "suite": "breengel",
          "categories": [
            "inflate"
          ]
        }
      },
      {
        "id": "images",
        "title": "BMP & PNG images",
        "measure": {
          "suite": "breengel",
          "categories": [
            "bmp",
            "png-decode",
            "png-encode"
          ]
        }
      },
      {
        "id": "widgets",
        "title": "widgets",
        "measure": {
          "suite": "breengel",
          "categories": [
            "widgets"
          ]
        }
      },
      {
        "id": "text-entry",
        "title": "text fields, focus & file picker",
        "measure": {
          "suite": "breengel",
          "categories": [
            "text-entry"
          ]
        }
      },
      {
        "id": "icons",
        "title": "animated icons",
        "measure": {
          "suite": "breengel",
          "categories": [
            "icons"
          ]
        }
      },
      {
        "id": "windows",
        "title": "window lifecycle on every target",
        "measure": {
          "suite": "breengel",
          "categories": [
            "windows"
          ]
        }
      },
      {
        "id": "isolation",
        "title": "window authority & EFAULT safety",
        "measure": {
          "suite": "breengel",
          "categories": [
            "isolation"
          ]
        }
      },
      {
        "id": "composition",
        "title": "commit, composition & present",
        "measure": {
          "suite": "breengel",
          "categories": [
            "composition"
          ]
        }
      },
      {
        "id": "events",
        "title": "window input events",
        "measure": {
          "suite": "breengel",
          "categories": [
            "events"
          ]
        }
      },
      {
        "id": "scanout",
        "title": "display observation on every target",
        "measure": {
          "suite": "breengel",
          "categories": [
            "scanout"
          ]
        }
      },
      {
        "id": "panel",
        "title": "suite & probe panels",
        "measure": {
          "suite": "breengel",
          "categories": [
            "panel"
          ]
        }
      }
    ]
  },
  {
    "id": "window-manager",
    "title": "Window manager",
    "outcome": "Independent programs share one desktop: keyboard and pointer events from every target's devices reach bwm through the kernel's input core, and bwm composes, stacks, focuses, moves, resizes and closes their windows, routes input and the clipboard between them with a recorded outcome for every event, and recovers from hung, crashed or misbehaving clients and from its own death, on every target.",
    "suite": "window-manager",
    "subsystems": [
      17,
      18,
      25
    ],
    "milestones": [
      {
        "id": "input-core",
        "title": "input core & virtual devices",
        "measure": {
          "suite": "window-manager",
          "categories": [
            "input-core"
          ]
        }
      },
      {
        "id": "input-devices",
        "title": "keyboard & pointer drivers on every target",
        "measure": {
          "suite": "window-manager",
          "categories": [
            "input-devices"
          ]
        }
      },
      {
        "id": "startup",
        "title": "bwm starts and shows clients",
        "measure": {
          "suite": "window-manager",
          "categories": [
            "startup"
          ]
        }
      },
      {
        "id": "composition",
        "title": "composition, chrome & repaint",
        "measure": {
          "suite": "window-manager",
          "categories": [
            "composition"
          ]
        }
      },
      {
        "id": "stacking",
        "title": "stacking & minimize",
        "measure": {
          "suite": "window-manager",
          "categories": [
            "stacking"
          ]
        }
      },
      {
        "id": "focus",
        "title": "focus, keyboard routing & hotkeys",
        "measure": {
          "suite": "window-manager",
          "categories": [
            "focus"
          ]
        }
      },
      {
        "id": "pointer",
        "title": "pointer routing",
        "measure": {
          "suite": "window-manager",
          "categories": [
            "pointer"
          ]
        }
      },
      {
        "id": "geometry",
        "title": "move & resize",
        "measure": {
          "suite": "window-manager",
          "categories": [
            "geometry"
          ]
        }
      },
      {
        "id": "lifecycle",
        "title": "close, crash & launch",
        "measure": {
          "suite": "window-manager",
          "categories": [
            "lifecycle"
          ]
        }
      },
      {
        "id": "recovery",
        "title": "WM death, restart & supervision",
        "measure": {
          "suite": "window-manager",
          "categories": [
            "recovery"
          ]
        }
      },
      {
        "id": "clipboard",
        "title": "clipboard between programs",
        "measure": {
          "suite": "window-manager",
          "categories": [
            "clipboard"
          ]
        }
      },
      {
        "id": "liveness",
        "title": "idle, stalls & stress",
        "measure": {
          "suite": "window-manager",
          "categories": [
            "liveness"
          ]
        }
      },
      {
        "id": "performance",
        "title": "latency & frame rate",
        "measure": {
          "suite": "window-manager",
          "categories": [
            "performance"
          ]
        }
      }
    ]
  },
  {
    "id": "ui-testing",
    "title": "UI test framework",
    "outcome": "Any GUI program can be driven by scripted keyboard, pointer and text input from inside the guest and judged by its committed frames and semantic tree at acknowledged clock epochs and causal barriers, deterministically and identically on every target, with every check shown able to fail and every failure leaving its evidence.",
    "suite": "ui-testing",
    "subsystems": [
      18,
      21,
      25
    ],
    "milestones": [
      {
        "id": "runner",
        "title": "result transport & proof executor",
        "measure": {
          "suite": "ui-testing",
          "categories": [
            "runner"
          ]
        }
      },
      {
        "id": "input",
        "title": "scripted keyboard, pointer & text input",
        "measure": {
          "suite": "ui-testing",
          "categories": [
            "input"
          ]
        }
      },
      {
        "id": "tree",
        "title": "semantic trees of windows & widgets",
        "measure": {
          "suite": "ui-testing",
          "categories": [
            "tree"
          ]
        }
      },
      {
        "id": "frames",
        "title": "frame capture & comparison",
        "measure": {
          "suite": "ui-testing",
          "categories": [
            "frames"
          ]
        }
      },
      {
        "id": "clock",
        "title": "UI time & stepping",
        "measure": {
          "suite": "ui-testing",
          "categories": [
            "clock"
          ]
        }
      },
      {
        "id": "settle",
        "title": "causal barriers & settling",
        "measure": {
          "suite": "ui-testing",
          "categories": [
            "settle"
          ]
        }
      },
      {
        "id": "sessions",
        "title": "sessions, state isolation & cleanup",
        "measure": {
          "suite": "ui-testing",
          "categories": [
            "sessions"
          ]
        }
      },
      {
        "id": "diagnosis",
        "title": "saved evidence & failure messages",
        "measure": {
          "suite": "ui-testing",
          "categories": [
            "diagnosis"
          ]
        }
      },
      {
        "id": "proof",
        "title": "proven on bwm & a test app",
        "measure": {
          "suite": "ui-testing",
          "categories": [
            "proof"
          ]
        }
      }
    ]
  },
  {
    "id": "gui-apps",
    "title": "GUI applications",
    "outcome": "Breenix's desktop applications (bterm, blauncher, bfontpicker, blog with blogd, Gus Kit and bcheck, with the graphics demos) do their jobs correctly through real input on a real desktop, on every target.",
    "suite": "gui-apps",
    "subsystems": [
      24,
      25
    ],
    "milestones": [
      {
        "id": "terminal-emulation",
        "title": "bterm terminal emulation",
        "measure": {
          "suite": "gui-apps",
          "categories": [
            "terminal-emulation"
          ]
        }
      },
      {
        "id": "bterm",
        "title": "bterm runs shell sessions",
        "measure": {
          "suite": "gui-apps",
          "categories": [
            "bterm"
          ]
        }
      },
      {
        "id": "blauncher",
        "title": "blauncher finds & launches programs",
        "measure": {
          "suite": "gui-apps",
          "categories": [
            "blauncher"
          ]
        }
      },
      {
        "id": "bfontpicker",
        "title": "bfontpicker sets system fonts",
        "measure": {
          "suite": "gui-apps",
          "categories": [
            "bfontpicker"
          ]
        }
      },
      {
        "id": "blog",
        "title": "blog & blogd show the logs",
        "measure": {
          "suite": "gui-apps",
          "categories": [
            "blog"
          ]
        }
      },
      {
        "id": "guskit",
        "title": "Gus Kit paints & saves",
        "measure": {
          "suite": "gui-apps",
          "categories": [
            "guskit"
          ]
        }
      },
      {
        "id": "guskit-collab",
        "title": "Gus Kit collaboration",
        "measure": {
          "suite": "gui-apps",
          "categories": [
            "guskit-collab"
          ]
        }
      },
      {
        "id": "bcheck",
        "title": "bcheck reports truthfully",
        "measure": {
          "suite": "gui-apps",
          "categories": [
            "bcheck"
          ]
        }
      },
      {
        "id": "demos",
        "title": "graphics demos in windows",
        "measure": {
          "suite": "gui-apps",
          "categories": [
            "demos"
          ]
        }
      },
      {
        "id": "desktop",
        "title": "the whole desktop together",
        "measure": {
          "suite": "gui-apps",
          "categories": [
            "desktop"
          ]
        }
      }
    ]
  }
]
```

Vigil treats the last entry in `efforts` as the destination, so with these appended **the path's destination
moves from `sdk` to `gui-apps`.** The path's title and outcome still describe a path ending at the SDK. Replace
them in the same commit:

```json
"title": "Path to a POSIX kernel with a userspace and a desktop you can build on",
"outcome": "Breenix runs real programs unmodified on ARM64 and x86-64: every POSIX system interface backed and behaving to the standard, a C library and Rust std on top, POSIX utilities, an SDK for building our own applications, and a graphical desktop (BreenGL, the window manager and its applications) tested to the same bar."
```

No `bootPath` milestone is added (section 14.1), so `docs/boot-path.json` needs no new milestone for these
efforts. Its desktop-mode stages change as health evidence in the same work as `window-manager/startup`.

## 23. Open contracts by effort

A contract here is a specified rule with the milestone and case that will prove it, not a passing test. Each
later effort turns its contracts into its written contract document and cases when it is authored. It may
sharpen them, but it may not weaken a stated expectation without an operator ruling.

**`breengel`: nothing open.** Everything this effort delivers is settled in this plan. Its contract
document (`docs/gui/breengel.md`) transcribes: the kernel replay inside `acquire` under a replay pin, and the
whole-window fallback for stale commits (section 3.1; `windows/acquire-replays-damage`, `acquire-full-copy`,
`stale-base-commit`); `capture_window`'s tree destination (`windows/capture-across-destroy`); the processed
watermark and `dispatch` (section 6.4; `events/processed-not-polled`, `processed-at-acquire`,
`no-change-after-redraw`); pixel generations and `commit_no_change` (`composition/no-change-commit`); no pins
between syscalls, and remnants (`windows/two-buffer-capture-load`, `remnant-on-death`); the glyph flattening and
rasterization bounds with the independent square (section 12.3; `font-parser/work-bound`, `coords-extreme`,
`text/size-range`); one output-accounting boundary per byte, with its structure test
(section 12.1; `isolation/quiet-errors`); the 250 ms font watcher (`text/fontwatch-idle`,
`fontwatch-file-replaced`); `suite::spawn` with the direct-fork validator fixture (section 8.5); and the
operator's suite-mode gate (section 3.4; `isolation/controls-latched`).

**`window-manager` must settle:**
1. **bwm's per-gesture pointer grab** (section 5.5): the stale-scene check on a gesture's first batch only,
   later batches delivered to the grabbing window, and the grab ended on window loss or compositor death.
   Closed by `pointer/stale-scene`, `capture`, `capture-target-closes`, and in integration by
   `ui-testing/input/drag-steps`.
2. **bwm's epoch acknowledgment** through `compositor_ack` with `presented` or `none-needed` (section 6.3).
   Closed by `startup/epoch-ack` and `liveness/idle-no-composites`.
3. **The per-target driver coverage map**, confirmed against the bound devices once the entry spikes have run,
   and the Tier-2 factoring of the PS/2 handler (section 5.2). Closed by `input-devices/devices-bound`,
   `keyboard-completion-path`, `pointer-completion-path`, `replay-report-fault`, `replay-wake-fault`.

**`ui-testing` must settle:**
1. **Gesture identity** (section 5.5): gesture ids in `SYN_SCENE`, Breengel's pointer capture, `PointerCancel`
   and per-gesture `started`/`completed`/`cancelled` metadata, and libuitest retrying only unstarted gestures.
   Closed by `input/click-stale-scene`, `click-stale-layout`, `drag-steps`.
2. **Effective-domain non-vacuity** (section 7.5): predicate operands named by declared region, domains computed
   after masks, and an empty or under-sized domain rejected before comparison. Closed by
   `frames/structural-vacuous` and `mask-cap`, with the masked-border fixture.
3. **Historical-tree retention** (section 10.3): the arena's tree slot, filled by `capture_window` with the
   compared commit's tree, and the `writing`/`final` publication order. Closed by `diagnosis/tree-at-commit`
   and `failed-screen-retained`.
4. **Supervisor subreaping** (section 10.1): every supervisor a subreaper before its first launch, and the
   runner's `supervisor-died` fallback. Closed by `runner/subreaper` and `outer-collector-timeout`.
5. **The runner fixture, collection handshake and result transport** (sections 8.2–8.4, 10.2), as specified.
   Closed by `runner/never-ran`, `mismatch-is-fail`, `record-malformed`, `record-duplicate`, `record-binding`,
   `outer-positive`, `outer-case-death`, `sweep-descendants`, `forgery-fork`, `forgery-exec` and
   `diagnosis/case-death`.
6. **Settle over the watermark and presentation** (section 6.4). Closed by `settle/drained-not-rendered`,
   `minimized-acks`, `no-change-no-composite`, `bwm-ack-no-composite`.

**`gui-apps` must settle:**
1. **The save protocol on each side of the rename** (section 18). Closed by `guskit/save-fail-before-rename`,
   `save-fail-after-rename`, `save-interrupted`, and `bfontpicker/write-failure-preserves`, `dirsync-failure`,
   `write-interrupted`.
2. **The RLIMIT_FSIZE witnesses**, specified in full in section 11 (the mechanism proved in the case's own
   process, with signal generation shown by a blocked SIGXFSZ and by a counting handler; the app judged only on
   short count, EFBIG and survival; recovery by the same pid once `prlimit64` lifts the limit). Closed by
   `guskit/save-short-write` and `blog/blogd-short-write`.
3. **blogd's log-as-cursor append and the sequence-aware `/dev/kmsg` reader with a boot id** (section 18).
   Closed by `blog/blogd-kmsg`, `blogd-killed`, `blogd-write-failure`, `blogd-restart`, `blogd-foreign-boot`,
   `blogd-ring-wrap`.
4. **Collaboration acknowledgments and the socket forcing controls** (sections 11 and 18). Closed by
   `guskit-collab/submitted-acked`, `lost-before-admission-detected`, `no-loss-no-dup`, `short-send`,
   `eagain`.
5. **bterm's streaming witnesses** (section 18). Closed by `bterm/throughput` and
   `throughput-fault-detected`.

## Appendix A. Findings and what changed

A.1 lists the critique's 43 findings. The re-check rated the first revision 21 fixed, 5
answered acceptably and 17 open. The second re-check reviewed those 17 and the re-check's ten new
problems: 5 fixed, 16 explicit contracts and 6 open (findings 13, 14, 15, 19 and 23, and N1). Its "—" means
the row was settled before the second re-check and not re-examined. Rows keep their earlier text, with a note
where a later revision touched them. This third revision repairs in the plan itself the four defects the second
re-check said must be fixed first: the acquire/replay lifetime and history (N11, which closes N1), the
glyph-work expectation (N12, which closes 19 and 23), exactly-once output accounting (N13, which closes 14) and
the SIGXFSZ witness (N14, which closes 15). It gives the other four new problems explicit contracts with their
closing milestones and cases (N15 to N18; N18 closes 13). A.2 lists the re-check's ten new problems, and A.3 the
second re-check's eight. After this revision no finding is open. Every remaining contract belongs to
`window-manager`, `ui-testing` or `gui-apps`, and section 23 lists them by effort; `breengel` has none.

### A.1 The critique's findings

| # | Finding | Re-check | Second re-check | What changed |
|---|---|---|---|---|
| 1 | Parallels fallback weakens the oracle | fixed | — | Fallback removed. A fenced device-readback entry gate decides Parallels' production compositor: VirGL with device readback, or the software compositor (§4.2). A reconstructed image never counts. |
| 2 | "Scanout" captures read source memory | answered | — | A per-backend observation contract names what is read, the completion that must be present, the serial binding and the boundary. The full destination is read, and four planted faults must fail (§4.1, `scanout/*`). |
| 3 | Equal generations are not an atomic frame | fixed | — | Commit protocol: immutable committed buffers, tree in the commit (§3.1). Capture race cases in `frames/capture-*`. Second revision: capture pins last only inside a capture call, and the incremental-acquire defect is repaired (N1, N3). Third revision: the replay copy moves into the kernel, inside `acquire` under a replay pin, and stale commits fall back to whole-window damage when their history is gone (N11). |
| 4 | Consumption is not rendering | open | contract | §6.4: a serial is handled only when its `dispatch` handler returns; `commit(buffer)` acknowledges only serials handled before that buffer was acquired; `commit_no_change` cannot pass a `Redraw` event. Proved in `breengel/events` by `processed-serial`, `processed-not-polled`, `processed-at-acquire`, `no-change-after-redraw`, and in `ui-testing` by `settle/drained-not-rendered` and `proof/fault-drain-no-render`, `fault-stale-pixels`. |
| 5 | WM-handled input has no consumer serial | fixed | — | Every serial gets one terminal disposition in a gap-aware ledger; derived ids for repeats and notices; positive twins (§5.3). Second revision: bwm acknowledges handled serials in its status page (§6.3), and `client-rejected(stale-layout)` joins the table. |
| 6 | Virtual clock does not control asserted timers | fixed | — | Repeat, clicks and gestures move to bwm on the UI clock and event timestamps; uinput `time=injector`; a time-domain table (§5.1, §6.1). Second revision: the repeat, click and tap numbers are in §12.3. |
| 7 | step and settle need a scheduling protocol | open | contract | §§6.3–6.4: acknowledgment is metadata and never needs a composite; bwm acknowledges through `compositor_ack` with `presented` or `none-needed`; presentation is required only of visible, non-excluded windows and is tracked by pixel generation, so a `commit_no_change` is presented by the composite already holding its pixels. Proved by `ui-testing/settle/minimized-acks`, `no-change-no-composite`, `bwm-ack-no-composite`, `breengel/composition/no-change-commit`, `window-manager/startup/epoch-ack` and `liveness/idle-no-composites`. |
| 8 | External state and real input uncontrolled | fixed | — | Session device selection and grab (§5.4); one config root for every participant; pinned TZ, locale and ordering; independent sampling for system data (§9). |
| 9 | Semantic cross-check cannot catch lying text | fixed | — | The tree is not an oracle; text is checked against certified-renderer pixels; role-specific invariants; WM policy against an operation model (§7.1, §7.3). `proof/fault-lie-text`, `tree/text-pixels`. |
| 10 | Shared code makes both sides wrong | fixed | — | Independence rules (§2.5): literal tables in `docs/gui/bwm.md`, libuitest's own key table, `bwm-fault_test` chrome-offset and keymap-swap proofs. |
| 11 | Self-tests conflict with the outer runner | open | contract | §8.4: the real `Suite::gui()` runner runs as a scoped fixture (`suite-runner-fixture_test`) with a transcript descriptor and a descendant-scoped sweep, and the outer case checks the exact CASE and COLLECT records that runner produced. Proved by `ui-testing/runner/never-ran`, `mismatch-is-fail`, `record-malformed`, `record-duplicate`, `record-binding`, `outer-positive`, `outer-case-death`, `outer-collector-timeout`, `sweep-descendants`. The host scorer keeps its own fixtures (§2.2). Third revision: the fixture gains a supervisor that dies, and the runner's `supervisor-died` fallback is proved by `outer-collector-timeout` and `runner/subreaper` (N17). |
| 12 | Assertions on an optional, lossy pipe | fixed | — | A mandatory result transport, separate from the live strip (§8.2). Second revision: the supervisor reports on its own collector socket, never the result pipe (finding 42). |
| 13 | `frameChecks` lacks a complete schema | open | open → contract (N18) | §7.5: every field typed with its range; formulas declare typed signatures that `params` must match; each structural predicate has its parameter list and non-vacuity rule; schema version 1 has no edge exemption and rejects `edgeRadius`. Proved by `ui-testing/frames/unknown-field`, `formula-params`, `structural-vacuous`, `mask-cap`, `channel-tolerance`, `max-differing`, `schema-version`. Third revision: non-vacuity is checked on each predicate's effective domain after masks, operands are bound by name to declared regions, and an empty domain is a validation failure, never a match (N18). Contract in `ui-testing/frames`, closed by `structural-vacuous` (with the masked-border fixture) and `mask-cap`. |
| 14 | Cases without a guest observable | open | open → repaired (N13) | §12.1: every observable has a positive control and every measured claim a negative twin in its case. Kernel output is counted per thread at every emission entry point on both architectures (fatal paths into an unattributed counter), enforced by a host structure test, so direct `serial_println!` is covered and other threads' logging is not counted. Proved by `breengel/isolation/quiet-errors` with `emit-diagnostic`, `composition/damage-work` with `ignore-damage`, `flush-irq-bound` with `irq-hold`. Third revision: kernel output is counted at exactly one accounting boundary per byte (x86-64 `queue_byte`, the COM2 log ring's `append` and the emergency port writer; ARM64 `uart_owner::write`), wrappers and drains never count, deferred notices count when posted, and the structure test proves each writer reaches one boundary once (N13). Proved by `breengel/isolation/quiet-errors` with a subcase per writer path, nested wrappers included, on both architectures. |
| 15 | Negative cases cannot force their failure | open | open → repaired (N14) | §11: socket test controls `short-send` and `eagain` force each condition exactly and the host's counters must move by exactly 1 and *n*; RLIMIT_FSIZE forcing states POSIX's short count then EFBIG with SIGXFSZ, the apps ignore SIGXFSZ by contract, and the case proves the mechanism on its own file first; `kill-after` forces death between save steps. Proved by `guskit-collab/short-send`, `eagain`, `guskit/save-short-write`, `blog/blogd-short-write`, `save-interrupted`, `blogd-killed`. Third revision: the RLIMIT_FSIZE witness no longer asks an ignored SIGXFSZ to be pending. The mechanism is proved in the case's own process (short count; EFBIG with SIGXFSZ blocked and accepted by `sigtimedwait`; a counting handler's count rising by 1), the app is judged on short count, EFBIG and survival, and recovery by the same pid follows `prlimit64` lifting the limit (N14). Proved by `gui-apps/guskit/save-short-write` and `blog/blogd-short-write`. |
| 16 | Early acceptance depends on effort three | open | contract | §§8.1–8.2, 14.3: the descriptor rule lives in `libbreenix::suite::spawn`, delivered with `Suite::gui()` in `breengel`; every GUI suite, including the first two that do not use libuitest, creates processes through it, and the validator rejects direct `fork` from `breengel` on, so the later result pipe is closed by the same code and the early suites need no rewrite. Proved by `ui-testing/runner/forgery-fork`, `forgery-exec` and the validator's fixtures. Third revision: the direct-fork validator fixture is listed in §8.5 and lands with `Suite::gui()` in `breengel`. |
| 17 | Desktop boot-path evidence-routing gap | answered | — | No `bootPath` milestone; desktop start measured by suite cases; desktop-mode stages repaired as health (§14.1). Second revision: the child-init and suite-latch conflict is resolved (N9). |
| 18 | Deadlines and suite sizing | fixed | — | Named allowances, gate timeouts set from measurement, one full suite per effort, no filtered-boot evidence (§19). Second revision: allowances added for the new two-run fault cases. |
| 19 | Thresholds are placeholders | open | open → repaired (N12) | §12.3 gives every remaining number: blocked-call and timed-wait deadlines, step, settle and reserve bounds, repeat, click and tap timing, the 250 ms font watcher, the 5,000 ms launch deadline, icon settling and `sin_approx` error, Bounce's speed range, and parser, inflate, image and raster work and allocation bounds. Session start is restored to the adopted 300 ms (N8). Every case that said "the contract's bound" now cites its number. Third revision: the glyph rasterization bound now has its output-area term, `E + H × (10C + 8E × (3 + ⌈log₂(E + 1)⌉) + 17) + 64`, derived from the existing rasterizer's loops with a counted merge sort and a flattening row of its own, and enforced up front by a 2²⁸-step refusal (N12). Proved by `breengel/font-parser/work-bound` (the independent 1,024 × 1,024 square: at least 8,388,608 and at most 10,699,844 steps), `coords-extreme` and `text/size-range`. |
| 20 | Source-over double-applies alpha | fixed | — | Premultiplied formula with exact `div255` and literal examples (§7.4). |
| 21 | Dirty-rectangle contract undefined | fixed | — | Written-not-changed dirtiness, exact per-primitive rectangles, covering damage, WM outer painted bounds (§7.4). |
| 22 | Invalid PNG formats and a hidden codec | fixed | — | Legal combination matrix and `invalid-combos`; separate `inflate` milestone; separate decode and encode categories. Second revision: the inflate incompleteness rule is corrected (N5). |
| 23 | Font and surface safety too shallow | open | open → repaired (N12) | §12.3: always-on `work` counters count every loop iteration, rejected ones included; numeric step and allocation bounds for font load, glyph outline and rasterization, inflate and raster primitives; image and surface limits; `Surface::from_raw` checks every numeric rule and leaves only memory validity to its caller. Proved by `breengel/raster/extreme-work-bound`, `surface-validate`, `font-parser/work-bound`, `alloc-bound`, `coords-extreme`. The two invalid font expectations are corrected (N4). Third revision: the raster half is repaired as for finding 19 (N12): flattening and rasterization each have step and allocation bounds that count every loop, and an over-bound glyph is refused before any raster loop runs. |
| 24 | Authority and lifetimes under-specified | fixed | — | Authority and error table (§3.2). Second revision: a row for evidence holders and remnants. |
| 25 | Decoder replay does not prove the driver path | open | contract | §5.2: a per-target coverage map names each target's bound keyboard and pointer drivers; replays run only through bound instances; a host structure test fails any `input::report` caller no row covers. Proved by `window-manager/input-devices/devices-bound`, `keyboard-completion-path`, `pointer-completion-path`, `replay-report-fault`, `replay-wake-fault`. |
| 26 | Semantic clicks ignore occlusion and races | open | contract | §5.5: per-window layout versions, bumped by libbui and `layout_changed()`; tags carry scene and layout versions; bwm rejects stale scenes and Breengel rejects stale layouts at dispatch as `client-rejected(stale-layout)`. Proved by `ui-testing/input/click-stale-scene` and `click-stale-layout`. Third revision: gestures carry ids; both stale checks apply only to a gesture's first batch, later events follow bwm's grab and Breengel's pointer capture, and only an unstarted gesture is retried (N16). Contract closed by `ui-testing/input/click-stale-layout`, `drag-steps` and `window-manager/pointer/stale-scene`. |
| 27 | WM death and restart unspecified | fixed | — | Compositor epochs, an ownership stack, commits that never block (§3.3); a `recovery` milestone; a shared desktop supervisor. |
| 28 | Cleanup leaves files and devices | fixed | — | Per-case config root, runner hash-and-restore of global files, device reset, uid 1000 with asserted preconditions (§9). Second revision: device reset and config-root removal happen in the collection handshake (§10.2). |
| 29 | Same-boot reopen does not prove saving | open | contract | §18: the save protocol states what holds before the rename (old file untouched), after it (new file, shown unconfirmed, still unsaved, directory `fsync` retried) and on interruption (whole old or whole new file). Proved by `gui-apps/guskit/save-fail-before-rename`, `save-fail-after-rename`, `save-interrupted`, `bfontpicker/write-failure-preserves`, `dirsync-failure`, `write-interrupted`. Reboot durability stays with `files-io`. |
| 30 | bterm needs a capability contract | answered | — | A pinned terminfo capability list, cell-width policy, parser bounds and three chunkings per case. |
| 31 | Terminal readiness and completion | open | contract | §18: bterm's terminal node publishes `stream-digest` over consumed bytes and `rows-digest` over retired rows, only through commits; the case computes both independently at any byte count. Proved by `gui-apps/bterm/throughput` (final and sixteen intermediate prefixes) and `throughput-fault-detected` (a `bterm_test` that drops one byte fails). |
| 32 | Launcher entries contradict the CLI catalog | fixed | — | `.app` format owned by `gui-apps`, entry-specific outcomes, `ack-app_test`, SDK `--app-entry` as new work measured here. |
| 33 | Font settings miss invalid config and idle reload | open | contract | §12.3: the watcher checks fonts.conf and every configured font file every 250 ms by inode, size and modification time. Proved by `breengel/text/fontwatch-idle` (in use at the first check after the change) and `fontwatch-file-replaced` (same-path font replacement by rename), plus the existing `fontwatch-*` and `bfontpicker` cases. |
| 34 | blogd ring wrap needs a producer interface | open | contract | §18: every line carries the boot id and sequence number, and the log's last complete line is the cursor; recovery truncates a partial line and resumes after the last complete one, on start and before every retry. Proved by `gui-apps/blog/blogd-kmsg`, `blogd-write-failure`, `blogd-short-write`, `blogd-restart`, `blogd-killed`, `blogd-foreign-boot`, `blogd-ring-wrap`. |
| 35 | Collaboration convergence can hide loss | open | contract | §18: every submitted operation is acknowledged with its global sequence or refused with a reason, and each peer publishes its submitted ids and answers; expectations come from the script's submissions, not the host's admitted log; short sends and EAGAIN are forced by socket test controls. Proved by `gui-apps/guskit-collab/submitted-acked`, `lost-before-admission-detected`, `no-loss-no-dup`, `short-send`, `eagain`. |
| 36 | bcheck truthfulness | fixed | — | A shared supervision core; descendants, capped and forged output, exit 127 against exec failure, close through input, fixture and loopback production catalogs. |
| 37 | Probe and panel graphics unmeasured | fixed | — | New `breengel/panel` milestone; kernel primitives replaced by libgfx; cursor-trail detection by test control. |
| 38 | libicon and demo coverage narrow | fixed | — | New `icons` milestone with explicit time and literal tables; demo interactions and invariants. Second revision: settling and `sin_approx` numbers in §12.3. |
| 39 | Milestones bundle large deliverables | answered | — | Smaller milestones (14, 13, 9 and 10); terminal emulation before bterm; input before WM interaction; dependency map and size estimates (§14); applications stay one effort. |
| 40 | Path compatibility is not schema support | fixed | — | Manifest `area` values, underscore source names, validator and Vigil extensions with regression fixtures (§8.5). The four effort objects in §22 are unchanged by this revision; no category was added or removed. |
| 41 | The verdict script does not enforce zero skips | fixed | — | The shared policy requires `skipped == 0`, with fixtures; GUI sources cannot call `skip` (§2.2). |
| 42 | Crash and timeout artifacts depend on the dead process | open | contract | §10: the runner starts the session supervisor beside the case; session processes are the supervisor's descendants; the runner kills only the case at *L − R*, sends `end collect`, and sweeps only after `done` or at *L*; the failed check's screen and frame are kept in an evidence arena marked final at the failure, and dead apps' last frames in kernel remnants, with no pins on live buffers. Proved by `ui-testing/runner/outer-case-death`, `outer-collector-timeout`, `diagnosis/case-death`, `failed-screen-retained`, `app-death`, `reserve`, and `breengel/windows/remnant-on-death`. Third revision: the arena gains a tree slot filled by `capture_window` with the compared commit's tree and a `writing`/`final` publication order (N15), and every supervisor is a subreaper before its first launch, with the runner as the fallback when it dies (N17). Contracts closed by `diagnosis/tree-at-commit`, `failed-screen-retained`, `runner/subreaper`, `outer-collector-timeout`. |
| 43 | Repeating a script misses wall-clock animation | answered | — | A `real_clock_reads` counter makes `proof/fault-wallclock` deterministic (§6.2). Second revision: the detector's scope (reads through `breengel::clock`) is stated, and the fault reads the real clock through that provider. |

### A.2 The re-check's new problems

| # | Problem | Second re-check | What changed |
|---|---|---|---|
| N1 | Incremental acquire can erase committed pixels | open → repaired (N11) | §3.1: each buffer has a base; the kernel keeps an 8-commit damage log and returns a replay set (all damage since the buffer's base, or `full`); a commit from an older base is accepted whole with its damage widened. Proved by `breengel/windows/acquire-replays-damage` (the `[1,0]`, `[1,2]` counterexample over three buffers), `acquire-full-copy`, `stale-base-commit`. Third revision: the replay is copied by the kernel inside `acquire` from a source held by a replay pin, so another thread cannot recycle it mid-copy, and stale commits get whole-window damage when their history has left the log (N11). |
| N2 | Settle contradicts invisible and metadata-only commits | contract | §§6.3–6.4: acknowledgment never needs a composite; presentation is required only of visible, non-excluded windows, by pixel generation; `commit_no_change` keeps the pixel generation; bwm acknowledges separately with `presented` or `none-needed`. Cases as for finding 7. |
| N3 | Evidence pins can exhaust the buffer pool | contract | §§3.1, 10.3: no pin survives a syscall; evidence is kept by copies in the supervisor's arena and by kernel remnants made only at owner death, counted against the holder and capped at 16; the no-compositor release rule is qualified. Proved by `breengel/windows/two-buffer-capture-load`, `capture-across-destroy`, `remnant-on-death`. |
| N4 | Invalid font corpus expectations | fixed | `font-parser/cmap4-segments` now rejects an odd segCountX2, and `cmap4-odd-segments` accepts the specification's 39-segment layout; `hmtx-short` became `hmtx-compressed` (valid) and `hmtx-truncated` (the byte-length rule). |
| N5 | The incompleteness rule rejects a valid empty stream | fixed | `inflate/code-incomplete` now separates the three codes; new `code-single` (the re-check's empty-block fixture decodes to empty output) and `no-distance-codes` (RFC 1951 §3.2.7). |
| N6 | Save rollback crosses the rename commit point | fixed | §18's save protocol separates before-rename preservation, after-rename unconfirmed durability with the document still unsaved, and interruption on each side of the rename. Cases as for finding 29. |
| N7 | blogd's exactly-once append across a non-atomic cursor | contract | §18: the log is its own cursor (boot id and sequence on every line), and tail recovery before every start and retry removes the window between append and cursor. Cases as for finding 34. |
| N8 | Startup expectations weakened without a ruling | fixed | §12.2: both session-start cases are back at the adopted 300 ms; a change needs a measurement and an operator ruling, and the case keeps failing until then. |
| N9 | Child init and the suite-control latch disagree | fixed | §3.4: the absence check runs only in PID-1 init and is required by the production-profile boot gate and the clean-main desktop row; a child init skips it and says so. Proved by `breengel/isolation/controls-latched` and `gui-apps/desktop/init-desktop`. |
| N10 | Surviving collection has no outer-runner handshake | contract | §10.2: runner-owned supervisor, kill of the case only at *L − R*, `end collect` and `done` before the sweep, COLLECT on a separate collector socket, `collector-timeout` at *L*. Cases as for finding 42; the order is proved through the real runner (§8.4). |

### A.3 The second re-check's new problems

| # | Problem | Effort | What changed |
|---|---|---|---|
| N11 | Replay has no protected source, and stale-commit damage outlives its history | `breengel` (repaired) | §3.1: `acquire` computes the replay set under the window's commit lock, takes a replay pin on the newest front, copies inside the syscall without the lock, then drops the pin and sets the buffer's base to the front it copied from. A pinned buffer cannot return to free, so the copy's source cannot be recycled, and the buffer always equals the commit its returned base names. Breengel and raw clients copy nothing. A stale commit whose history has left the 8-entry log, or whose base is none, gets whole-window damage. A new window test control, `acquire-stall`, forces the overlap (§11). Proved by `breengel/windows/stale-base-commit` (the recheck's three-buffer interleaving held open by `acquire-stall`, and more than eight intervening commits, each step checking whole pixels and damage coverage), `acquire-replays-damage` and `acquire-full-copy`. |
| N12 | The glyph-work bound omits bitmap-area work | `breengel` (repaired) | §12.3: a flattening row (depth at most 9, 512 segments per curve, 65,536 per glyph, 1,028 steps per point) and a rasterization row whose bound `E + H × (10C + 8E × (3 + ⌈log₂(E + 1)⌉) + 17) + 64` is derived loop by loop from the existing rasterizer, with an allocation bound and a 2²⁸-step refusal computed before any raster loop. Crossings are sorted by libfont's own counted merge sort, and no loop is excluded. The independent 1,024 × 1,024 four-edge square must take between 8,388,608 and 10,699,844 steps, so a counter that skips the span fill fails. Proved by `breengel/font-parser/work-bound`, `coords-extreme` and `text/size-range`. |
| N13 | "Count at every entry point" counts the same bytes twice | `breengel` (repaired) | §12.1: exactly one accounting boundary per byte, named per architecture from the source (x86-64 `queue_byte`, the COM2 log ring's `append`, the emergency port writer; ARM64 `uart_owner::write`; the lock-free raw trace writers). Wrappers and drains never count. Notices count to the posting thread when posted and pass the boundary marked already counted. Lead newlines are not payload. The two fatal raw-port sites are listed exceptions. The structure test proves each public writer reaches one boundary once. Proved by `breengel/isolation/quiet-errors` on both architectures, with a subcase per writer path (direct, nested, logger, notice written by another thread, `emergency_print` into the unattributed counter) and a second emitting thread. |
| N14 | An ignored SIGXFSZ is not a portable pending-signal witness | `gui-apps` (repaired) | §11: signal generation is proved in the case's own process with SIGXFSZ blocked and accepted by `sigtimedwait`, and with a counting handler; the apps, which ignore SIGXFSZ, are judged on exact short count, EFBIG, the error shown and survival; recovery is the same pid's next save or append once `prlimit64` lifts the limit. Proved by `gui-apps/guskit/save-short-write` and `blog/blogd-short-write`. |
| N15 | The evidence arena loses the historical tree | `ui-testing` (contract) | §§3.1, 10.3, 10.4: `capture_window` takes a tree destination and copies the same commit's tree page under the same pin (settled in `breengel`, `windows/capture-across-destroy`). The supervisor-owned arena gains a 1 MiB tree slot, and slots publish in a `writing`/`final` order with hashes in the slot record. A slot left `writing` by a dead case is `partial:case-died`. Closed by `ui-testing/diagnosis/tree-at-commit` and `failed-screen-retained` (30 later commits, the case killed before writing artifacts, then the app killed). |
| N16 | Retrying a stale click can repeat a partially executed gesture | `ui-testing`, `window-manager` (contract) | §5.5: gesture ids; stale-scene and stale-layout checks on a gesture's first batch only; bwm's grab and Breengel's pointer capture carry the rest; cancellation by `PointerCancel` and `client-rejected(gesture-cancelled)`; libuitest retries only gestures that never started. Closed by `ui-testing/input/click-stale-layout` and `drag-steps` (layout changes on the press and mid-drag, one activation, nothing held) and `window-manager/pointer/stale-scene`. |
| N17 | The supervisor is not made the subreaper its case expects | `ui-testing` (contract) | §10.1: every session supervisor calls `PR_SET_CHILD_SUBREAPER` before its first launch and refuses to start a session without it. One supervisor code path serves boot sessions, inner scenarios and the fixture. When the supervisor dies, its orphans go to the runner, which writes `artifacts=none:supervisor-died`. Closed by `ui-testing/runner/subreaper` (the orphan's parent pid and its status 7, in all three settings, plus supervisor death) and `outer-collector-timeout`. |
| N18 | Structural non-vacuity is checked before the effective mask | `ui-testing` (contract) | §7.5: predicate operands are names of declared regions; each predicate's effective domain is computed from compared pixels after masks; `outside` needs at least one unmasked outside pixel and 75% of the outside area; equality needs compared pairs; count and difference thresholds must be achievable; an empty domain is a validation failure before comparison. Closed by `ui-testing/frames/structural-vacuous` (the masked-border fixture rejected, and the same check unmasked catching a planted border pixel) and `mask-cap`. |
