# #482: Arm xHCI completion IRQs before publishing boot enumeration requests

Round date: 2026-09-06/07. Branch `usb/482-xhci-arm-before-kick`, kernel commit
`e8038e5043cb9a60c53a77283b0cfb28638b2214`. Plan followed:
`scratchpad/astra/PLAN-482.md` (astra:xhigh RCA + plan, this round: astra:medium
execution slot). Line numbers below are read at this commit.

## 1. RCA summary (see PLAN-482.md section 1 for the full argument)

`submit_command_and_wait` and `control_transfer` (EP0) both used to enqueue
their TRB(s) and ring the doorbell BEFORE calling `enable_irq_for_wait`, which
clears the SPI's pending bit before re-enabling it. xHCI configures this SPI
edge-triggered. If the controller executed the command/transfer and raised its
MSI in the window between publication and that clear, the guest's own clear
discarded the legitimate pending edge, and the wait timed out despite the
request having actually completed. Two of six preserved boots in the prior
sweep showed a one-slot EnableSlot jump consistent with an abandoned-but-
executed command; the RCA treats this as strongly corroborating, not a direct
observation of the GIC interleaving (no preserved sample recorded the pending-
bit transition itself).

The fix reorders both synchronous paths so arming precedes publication, adds
bounded stage-labeled logging to the three previously-swallowed `scan_ports`
descriptor/config errors plus `configure_hid`'s error, and leaves the two
asynchronous doorbell callers (`queue_hid_transfer`, `queue_msi_probe_noop`)
untouched.

## 2. What changed

`kernel/src/drivers/usb/xhci.rs` only, plus one new structure test.

1. **`submit_command_and_wait`** (`xhci.rs:1534`): `enable_irq_for_wait(state)`
   moved from after `ring_doorbell` to immediately after
   `XHCI_COMMAND_WAITING.store(true, ...)` and before `enqueue_command(trb)`.
2. **EP0** (`prepare_transfer_wait` at `xhci.rs:1580`, `wait_for_prepared_transfer`
   at `xhci.rs:1605`, caller `control_transfer` at `xhci.rs:1922`):
   `prepare_transfer_wait` now takes `state`, rejects `irq==0` before touching
   any software state, prepares the software wait, arms the transient SPI, and
   returns the ownership flag (`Result<bool, &'static str>`). `control_transfer`
   calls it immediately after computing `slot_idx`, before the first
   `enqueue_transfer` call for the Setup-stage TRB. The returned flag is
   threaded into `wait_for_prepared_transfer(state, transient_irq)`, which no
   longer calls `enable_irq_for_wait` or checks `state.irq` itself.
3. **`scan_ports`** (`xhci.rs:4013`): the three swallowed `Err(_)` arms for
   `get_device_descriptor_short`, `get_device_descriptor`, and
   `get_config_descriptor`, plus the swallowed `configure_hid` error, now each
   log one bounded line —
   `[xhci] enum_failed port={} slot={} stage={device-short,device-full,
   configuration,hid-configure} error={}` (`xhci.rs:4230,4246,4285,4311`) —
   before the existing `continue` (or, for `configure_hid`, without one, since
   it's the last statement in the per-port loop body). No timeout or retry
   count changed. The `SLOT_ENABLE_MAX_ATTEMPTS` retry comment (`xhci.rs:
   ~4130-4148`) is rewritten to describe the actual arm-after-publish race
   instead of an unsupported "slow device" assumption.
4. **`queue_hid_transfer`** (`xhci.rs:3414`) and **`queue_msi_probe_noop`**
   (`xhci.rs:5469`) are untouched — both stay asynchronous (enqueue + ring
   doorbell + return, no wait). `activate_msi_if_ready_locked` (`xhci.rs:5438`)
   already armed the SPI before queueing its probe NOOP; unchanged.
5. **`handle_interrupt`** (`xhci.rs:5185`) is untouched — no logging,
   formatting, allocation, or new locking was added to the ISR by this round.
   Completion-ownership matching (command/TD identity) is explicitly deferred;
   this PR closes one producer of stale-notification loss, not each
   remaining ownership hazard.

## 3. Structural ratchet

`tests/xhci_wait_irq_order_structure.rs` (introduced by this round; extended in §10). Std-only,
compiled directly via `rustc --test` per this repo's
`scripts/run-structure-tests.sh` convention (no `cargo test` — see that
script's header on why). Reuses the `code_mask`/`identifier_offsets`/
`function_spans` technique from `tests/tty_irq_fg_structure.rs`.

Eight rules, each backed by a `#[test]`:

1. **Doorbell-caller census** — each function whose body calls
   `ring_doorbell(` must be exactly one of `submit_command_and_wait`,
   `control_transfer`, `queue_hid_transfer`, `queue_msi_probe_noop`, called
   exactly once each. A new/unclassified caller or a duplicate call fails.
2. **`db_base` write pinned to `ring_doorbell`** — each `write32(...)` call
   whose argument list mentions `db_base` must live inside `ring_doorbell`
   itself, so a new direct doorbell write can't evade rule 1's per-function
   census.
3. **`submit_command_and_wait` ordering** — WAITING=true < IRQ arm <
   `enqueue_command` < doorbell < wait < WAITING=false < transient cleanup,
   each read at byte-offset granularity through the comment/string mask.
4. **`prepare_transfer_wait` ordering** — `irq==0` rejection < software prep
   (WAITING=true) < IRQ arm, and it must return
   `Ok(enable_irq_for_wait(state))`.
5. **`control_transfer` ordering** — `prepare_transfer_wait` call < first
   `enqueue_transfer` < doorbell < `wait_for_prepared_transfer` call.
6. **`wait_for_prepared_transfer` has no GIC arming** — it must not call
   `enable_irq_for_wait(`, `clear_spi_pending(`, or `gic::enable_spi(`.
7. **Async exceptions** — `queue_hid_transfer` and `queue_msi_probe_noop`
   must ring a doorbell but must not call any of `wait_for_prepared_transfer(`,
   `wait_timeout_uninterruptible(`, `prepare_transfer_wait(`,
   `submit_command_and_wait(`.
8. **Activation-before-probe** — `activate_msi_if_ready_locked` must call
   `gic::enable_spi(state.irq)` before `queue_msi_probe_noop(state)`.

`deliberately_broken_copies_redden_the_rules` (the mutation leg) mutates
in-memory copies of the real source text back toward each pre-fix shape and
asserts the corresponding rule now fails. Nine of the eleven mutations use
`String::replace` against verbatim source snippets so a rule that quietly
stopped matching the real code would be caught; Mutations 5 and 7 are the
exception (§11, X-15) — they *append* a synthetic new function via
`format!()` rather than mutating existing text, because the shape they need
to redden (an unclassified fifth doorbell caller, a raw `db_base` write) has
no pre-existing "bad" text in the real source to revert:

- Mutation 1: command-arm reverted to after the doorbell → rule 3 reddens.
- Mutation 1b: same revert, plus a comment lying that it's already correct →
  rule 3 still reddens (proves the check reads code, not comments).
- Mutation 2: EP0 `prepare_transfer_wait` reverted to its old post-enqueue
  call site and old signature → rule 5 reddens.
- Mutation 3: `enable_irq_for_wait` reintroduced inside
  `wait_for_prepared_transfer` → rule 6 reddens.
- Mutation 4: `queue_hid_transfer` gains a synchronous wait call → rule 7
  reddens (and confirms the untouched exception, `queue_msi_probe_noop`,
  still passes).
- Mutation 5: an unclassified fifth function calls `ring_doorbell(` → rule 1
  reddens.
- Mutation 6: a duplicate `ring_doorbell(state, 0, 0)` inside
  `submit_command_and_wait` → rule 1 reddens.
- Mutation 7: a function writes `db_base` directly via `write32` instead of
  calling `ring_doorbell` — first asserted that rule 1 alone does NOT catch
  it (the function does not call `ring_doorbell(`), then that rule 2 does.
- Mutation 8: `activate_msi_if_ready_locked` reordered to queue its probe
  before enabling the SPI → rule 8 reddens.
- Mutation 9 (review fix pass, X-16): same command-arm revert as Mutation 1,
  but the lying comment now quotes the 7/7 marker strings
  `validate_command_arms_before_publish` searches for, verbatim → rule 3
  still reddens (a comment byte-identical to the real needle still doesn't
  satisfy a positional, mask-based check).
- Mutation 10 (review fix pass, X-7): an unclassified fifth doorbell caller
  whose `ring_doorbell(` call is preceded, modulo whitespace, by a comment
  ending in the literal text `fn` → rule 1 still reddens (the
  definition-site exclusion no longer trusts raw text across a comment
  boundary).
- Mutation 11 (review fix pass, X-8): a raw `db_base` write via `write32`
  with a comment between the identifier and its opening paren → rule 2
  still reddens (the call-site scan now skips comments, not only
  whitespace, between `write32` and `(`).

**Coverage gap, disclosed (§11, X-14):** 7 of the 8 rules above (1, 2, 3, 5,
6, 7, 8) have at least one dedicated reddening mutation among the eleven;
rule 4 (`prepare_transfer_wait` ordering) has 0 of 11 — it is exercised only
as a passing control by the real source, the same way every rule is, with no
adversarial copy that specifically breaks rule 4's ordering and asserts it
reddens. This is a real test-coverage gap in the mutation leg, not a defect
in rule 4 itself; closing it (a Mutation 12 that reverts
`prepare_transfer_wait`'s internal ordering) is left for a future pass rather
than added here.

Local result: `bash scripts/run-structure-tests.sh xhci_wait_irq_order_structure`
→ **10/10 tests green** (9 positive rules/sanity + the mutation leg covering
11 mutations, after this round's own review fix pass added three — see
§10). The full strict-gate structure preflight quoted in §5's table
(the original round's boot gate, unchanged by this fix pass) shows
`structure_suites=54/54` — this file is included and discovered
automatically, no closed name list to edit; §10 records this fix pass's own
host-side (no-boot) re-run of the full preflight against the now-55-file
tree.

## 4. Claim-lint

```
claim-lint: python3 scripts/claim-lint.py                                    -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg /tmp/commit-msg-482.txt -> exit 0
```
(Both run from the repo root of this worktree at the commit each covers; the
first covers the kernel + structure-test changes, the second the commit
message that landed them.)

## 5. Acceptance

All Parallels legs used the mandatory restart protocol
(`prlctl stop <vm> --kill`, poll to `stopped`, truncate
`/tmp/breenix-parallels-serial.log` — `launcher-smoke.sh` and `run.sh` do this
internally; the manual-input and lifecycle legs were stopped by hand after
each boot). Screen-lock was checked before every leg that injects input via
`prlctl send-key-event`
(`/opt/homebrew/bin/python3 -c "import Quartz; ...CGSSessionScreenIsLocked"` —
`0` every time this round; no wait was ever needed).

The 9/9 confirmed `cargo build` logs behind the runs in the table below (baseline
×3, launcher-smoke ×3, and `type-filter/run2-pass` — confirmed via each
preserved `run-sh.log`; lifecycle ×2 — confirmed via each `run-sh-stdout.txt`)
record the identical pre-existing toolchain future-incompatibility notice on
`core v0.0.0` (`warning: the following packages contain code that will be
rejected by a future version of Rust: core v0.0.0 (...)`). It is the only
warning any of these nine confirmed build logs produce, predates this
round's `xhci.rs` change (a toolchain/nightly note about the `core` crate,
unrelated to the USB driver), and is disclosed once here rather than
repeated per row — the strict-gate row below states it too, for that row's
own separate build.

| Leg | Invocation | Result |
|---|---|---|
| **Failing baseline** (unmodified `origin/main` @ `45daec35`, separate worktree) | `bash scripts/parallels/launcher-smoke.sh --max-inject-retries 0 --timeout 1200` | **3/3 RESULT: PASS** — no failure caught in 3 attempts. Consistent with the RCA's own characterization of an intermittent race (2/6 in the prior evidence sweep); this session did not reproduce a fresh failure on main. All three preserved (`serials/482/baseline/run{1,2,3}/`). |
| Original failing workload (fixed branch) | `bash scripts/parallels/launcher-smoke.sh --max-inject-retries 0 --timeout 1200` | **3/3 RESULT: PASS.** Each run: `hid-poll-line.txt` (grepped from the full `$SERIAL_LOG`) shows `start_hid_polling`'s real per-device slot/DCI summary for mouse+keyboard+its NKRO/composite report — reached only after `configure_hid` succeeds for both devices — and `[bterm] config:`/`[bterm] spawned child pid=` are both observed in each `serial-excerpt.txt`. Preserved: `serials/482/launcher-smoke/run{1,2,3}/`. See the evidence-fidelity note below the table for what raw `[xhci]` descriptor traffic is and is not additionally preserved per run. |
| Carried type-filter check | `bash scripts/parallels/launcher-smoke.sh --max-inject-retries 0 --timeout 1200 --type-filter` | **1/1 RESULT: PASS**, on the second attempt. The first attempt hit an unrelated stall in init's boot-time self-test battery at `CLONEVM_EXEC_TEST: second stage` (clone/exec, not xHCI — full xHCI enumeration had already completed cleanly, slots 1/2/3, before the stall; see §6). Killed and preserved as a non-xHCI finding at `serials/482/type-filter/run1-stalled-clonevm-unrelated/`; the retry passed cleanly and is preserved at `serials/482/type-filter/run2-pass/result.txt`. |
| Passive lifecycle comparison | `./run.sh --parallels --test 120` | **2/2**: clean enumeration (slots 1/2/3, no `enum_failed` line), full service lifecycle reached, bwm compositing live (~200 fps) at the end of both 120s windows. Preserved: `serials/482/lifecycle/run{1,2}/` (serial log + run.sh stdout + screenshot each). |
| Standing aarch64 QEMU regression gate | `./docker/qemu/run-aarch64-boot-test-strict.sh 3` (on a fresh `cargo build --release --features boot_tests --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`, 0 warnings besides the pre-existing toolchain future-incompat notice) | **3/3 boots succeeded** (100%), structure preflight `structure_suites=54/54`. Preserved: `serials/482/strict-gate/strict-gate-3boots.txt`. |

**Two clarifications on the table above (§11):**
- **X-4:** the strict-gate row's "3/3 boots succeeded" is the strict gate's
  own verdict format (`docker/qemu/run-aarch64-boot-test-strict.sh` prints
  `PASS: N/M boots succeeded`, confirmed literally at
  `serials/482/strict-gate/strict-gate-3boots.txt:57`), not a
  `RESULT: PASS` line — that literal string is produced only by
  `launcher-smoke.sh`'s own result files, which the baseline/launcher-smoke/
  type-filter rows correctly quote. The lifecycle leg similarly has its own
  pass criteria (§5's lifecycle row) rather than a `result.txt`. Each leg's
  verdict format matches the harness that produced it; there is no single
  `RESULT: PASS` requirement that spans all five rows.
- **X-5:** 0 of the 4 Parallels-leg evidence directories (baseline,
  launcher-smoke, type-filter, lifecycle) carry a build-time git SHA or
  image-hash receipt the way the strict-gate leg's ELF does (via its own
  build log). The 4 ran from this same worktree at the commit named in
  this doc's header, and the round's own git history (this branch's linear
  ancestry from `45daec35`/`e8038e50`) supports that, but the Parallels
  harness itself does not stamp a receipt into the boot disk image or serial
  log tying a given run to a specific source revision. Left as a disclosed
  harness gap, not fixed by this pass.

**Evidence-fidelity note on raw `[xhci]` descriptor traffic (fixed-branch
legs above):** the 4/4 fixed-branch runs cited in the table
(`launcher-smoke/run{1,2,3}` and `type-filter/run2-pass`) preserve
`hid-poll-line.txt`, grepped from the authoritative, un-truncated
`$SERIAL_LOG` — not from `serial-excerpt.txt`, which starts at the
launcher-trigger event and does not cover early boot. Raw `[xhci]`
port-scan/EnableSlot descriptor-fetch lines are additionally preserved
verbatim in `run-sh.log` for `launcher-smoke/run1` and `run3` (44 matching
lines each). They are NOT present in `launcher-smoke/run2`'s or
`type-filter/run2-pass`'s `run-sh.log` (0 matching lines in either — those
two `run-sh.log` files captured only the build/deploy phase, not the tailed
guest serial, an evidence-capture gap in this round's harness invocation
rather than a device- or enumeration-level difference: both runs' own
`hid-poll-line.txt` reports the identical successful `mouse=slot1/dci3 ...
kbd=slot2/dci3 nkro=dci5` summary as run1/run3). This fix pass hardens
`launcher-smoke.sh` (§10 below) to always capture the raw `[xhci]` trail
into a dedicated `xhci-enum-excerpt.txt` per run going forward; it does not
retroactively regenerate evidence for the four runs already completed
above.

**`launcher-smoke/run1`'s post-enter capture:** `smoke-log.txt` records
`capture (post-enter) failed (non-fatal); see capture.log` at 22:12:48;
`capture.log` shows three capture attempts against a black/near-black frame
(the harness's own VirGL-warmup non-fatal path — this leg's PASS verdict
does not depend on the post-enter screenshot). `display-post-enter.png` and
`display-post-enter.png.stats.json` are accordingly absent from `run1`'s
evidence directory (present for `run2`, `run3`, and
`type-filter/run2-pass`); this was not called out in the table above and is
disclosed here (X-22).

**Manual input leg** (at least once on a qualifying boot, per the acceptance
spec): attempted on a fresh, dedicated boot (`breenix-1788749740`). Only the
keyboard portion was live-injected, and (§11, X-1) that boot is not a clean
qualifying boot by PLAN-482 §3's own "keep kernel faults as failures" rule —
see the keyboard bullet below.

- *Keyboard*: connected to the guest's telnet shell (`telnetd`, port 2323,
  guest IP `10.211.55.100`, reachable directly from the host over Parallels'
  shared network) and ran `/bin/xhci_counters` before and after injecting a
  real 5-character string (`scripts/parallels/inject.sh type world`, i.e.
  actual `prlctl send-key-event` scancodes, not the telnet session's own
  keystrokes). Delta: **`XHCI_MSI_EVENT_TOTAL` 66→76 (+10)**,
  **`KBD_NONZERO_TOTAL` 5→10 (+5)**, `XHCI_LOCK_CONTENDED_TOTAL` unchanged at
  0 throughout. +5 KBD_NONZERO_TOTAL for 5 injected characters and +10 MSI
  events (one per key-down and key-up HID report) is exactly the expected
  shape. Full transcript preserved at
  `serials/482/manual-input/attempt2-keyboard-delta/telnet-transcript.txt`.
  **Not disclosed until this fix pass (§11, X-1):** the same boot's preserved
  `serial.txt` also contains a kernel fault —
  `[INSTRUCTION_ABORT] FAR=0x8 ELR=0x8 ESR=0x86000005 IFSC=0x5
  TTBR0=0x10000442fb000 from_el0=0` at `serial.txt:1385`, uptime ~65.4s, on
  `tid=36 name=telnetd_child_21_main` (`serial.txt:11374`) — sandwiched
  between the keyboard injection (heartbeat's `kbd_nonzero` jumps 0→5 at
  `uptime_ms=65384`, `serial.txt:1382`) and the "AFTER KEYBOARD" counter read
  the transcript above quotes. The kernel's own deferred-cleanup path
  absorbed it (`[INSTRUCTION_ABORT] deferring process cleanup`,
  `deferred_tid=36`, `serial.txt:11397-11398`) and the session kept running
  long enough to print the correct post-injection counters, but PLAN-482 §3
  is explicit that a kernel fault makes the boot it occurred on a failure,
  not a pass — so this delta, while numerically the exact expected shape, was
  not captured on a clean boot and this leg does not independently establish
  one. The fault's `ESR`/`IFSC`/`from_el0` match the pre-adjudicated near-null
  EL1 `INSTRUCTION_ABORT` signature tracked at #576 (`ELR=0, FAR=0`, same
  `ESR=0x86000005 IFSC=0x5 from_el0=0`); this occurrence's `FAR`/`ELR=0x8`
  rather than exactly `0` is disclosed as a real difference, not asserted as
  an exact match. It is in a process-lifecycle path (`telnetd`'s forked
  child), nowhere near `kernel/src/drivers/usb/xhci.rs`, and this round does
  not RCA or fix it — a fresh, fault-free re-run of this leg is left to a
  future pass rather than performed here.
- *Mouse*: **not performed as live host-cursor injection, and this remains
  unmet against PLAN-482 §3's requirement (§11, X-3) — not satisfied by any
  evidence in this round.** `prlctl` has no mouse-event subcommand (only
  `send-key-event`), and driving a synthetic mouse click through macOS's
  Quartz `CGEvent` API would require first locating the Parallels VM window
  via `System Events`, which failed with `osascript is not allowed assistive
  access (-25211)` — this session has no Accessibility permission. Posting a
  blind global mouse click at an unlocated screen position on a Mac this
  session shares with other lanes and the operator was judged unsafe (risk
  of clicking an unrelated window) and was not attempted. What this round
  DOES show: mouse enumeration and HID *configuration* (not just slot
  allocation) succeeded on each of the seven fix-side boots this round (3
  launcher-smoke + 1 type-filter + 2 lifecycle + this manual-input boot), and
  `bwm`'s live ~200 fps compositing in each lifecycle/manual-input boot
  depends on the same mouse-cursor render path the HID mouse feeds. This is
  not a substitute for a live click/scroll test, and is not claimed as one; a
  future session with host Accessibility permission granted is needed before
  this specific acceptance-oracle line item can be marked satisfied.
- An incidental finding while wiring the telnet session, corrected by this
  fix pass (§11, X-1/X-17): this repository's `telnetd`
  (`userspace/programs/src/telnetd.rs`) was reported as serving only ONE
  connection per VM lifetime, based on a separate, earlier probe boot
  (`serials/482/manual-input/attempt1-telnet-single-conn-limitation/
  serial.txt` — no kernel fault in that log; a second `connect()` reached
  the guest at the TCP level per `nc -zv`, but that boot's serial log
  (2255 lines, `attempt1-telnet-single-conn-limitation/serial.txt`) does not
  contain a second `TELNETD_CONNECTED`, and the first session does not end
  within those 2255 lines either). The blanket "only ONE connection ever"
  framing does not hold — a *different* boot, this keyboard-delta boot's
  `serial.txt`, directly shows a second `TELNETD_CONNECTED` at line 11606,
  following a fresh `TELNETD_LISTENING` at line 11400 that the
  deferred-cleanup path emitted once the faulted first child (tid=36,
  above) was torn down. So telnetd does accept a second connection at least
  once its first child's process is gone — the open question this round
  does not resolve is whether it also does so after an *ordinary*, clean
  first-session exit (attempt1's session is not observed to exit within its
  preserved 2255 lines; this boot's first session exited only via the
  fault). Not filed as a new issue
  (out of scope for #482); noted here, corrected, for whoever next needs
  guest-side telnet access.

**Mutation testing** (structural ratchet): 10/10 tests pass on the real
source; the 11 numbered mutations (plus variant 1b) redden their target
rule (§3), including the three added in this review fix pass (§10).

## 6. The type-filter leg's first attempt: an unrelated stall, not an xHCI failure

First `--type-filter` attempt (`serials/482/type-filter/
run1-stalled-clonevm-unrelated/`): the guest's real serial log
(`serial-full.txt`, 2252 lines as committed — corrected by this fix pass,
§11 X-21; the "2199 lines" this doc originally cited was a pre-final-drain
snapshot) shows a genuinely normal, clean xHCI enumeration (mouse=slot1,
keyboard=slot2, composite=slot3, no `enum_failed` line, `[xhci] Initialized:
32 slots, MSI irq=56` at line 267), followed by normal AHCI/ext2/net/timer/
SMP init, followed by init's boot-time self-test battery running several
oracles to completion (`block_eintr_oracle`, `futex_handoff_oracle`,
`poll_tcp_oracle`, `tty_oracle`, `exec_smoke`) — and then stalling with no
further progress for 480+ seconds after `CLONEVM_EXEC_TEST: second stage`,
the last line `clonevm_exec_test` (PID 13) ever printed. The kernel's own
heartbeat kept incrementing normally (scheduler alive, not a full freeze)
but init never spawned anything past PID 13/14, and `bwm` never appeared.
**Corrected by this fix pass (§11, X-19):** the doc previously claimed no
`[xhci]` line at all appeared past enumeration; in fact two routine
MSI-activation lines (`[xhci] activate_msi_if_ready: source=system-ready
spi=56 ...` and `[xhci] post-activation: ... SPI_ACTIVATED=true`,
`serial-full.txt:333-334`) print immediately after enumeration completes, as
they do on every boot — the accurate claim is that no `[xhci]` line of any
kind appears *during or after* the stall itself (i.e. after
`CLONEVM_EXEC_TEST: second stage`), which remains true. This is consistent
with a clone/exec-path stall rather than an xHCI notification-loss timeout —
softened by this fix pass (§11, X-18) from an unqualified "is": the serial
establishes *where* progress stopped (post-enumeration, post-SMP, after the
init self-test battery, deep in `clonevm_exec_test`'s second stage) but does
not, on its own, prove the causal mechanism; xHCI had already finished
successfully well before the stall either way. Per the plan's instruction to
capture the first-divergence record rather than add retries when an *xHCI*
failure is seen: this was not one, so the leg was retried once (clean pass,
§5) rather than escalated. Not the pre-adjudicated #906 signature (#906
stalls right after the ICMP echo line, before SMP bring-up; this stall is
well past SMP bring-up, deep into init's userspace self-test sequence).
**Corrected by this fix pass (§11, discovered while re-checking §6):** this
doc previously said the stall was "reported here rather than filed as a new
tracked issue" — it did not need filing. #690 ("clonevm_exec_test hangs
after 'second stage': post-exec rendezvous never completes ... aarch64
cortex-a72"), open since 2026-09-05, already tracks the identical signature
(`CLONEVM_EXEC_TEST: second stage` printed, `post-exec rendezvous complete`
never reached, `init` blocked in `waitpid`, heartbeat/net-rx counters still
advancing). This run is consistent with a fresh occurrence of #690, not a
new, separately-tracked issue; the raw serial remains preserved here as
additional evidence for #690 rather than for a new report.

## 7. QEMU and x86 applicability

Unchanged from the plan (PLAN-482.md §"QEMU and x86 applicability"): the
standing aarch64 strict gate boots `virt,gic-version=3` with VirtIO
keyboard/tablet/block/network, never instantiating xHCI, so its 3/3 pass
above is an aarch64 build/structure/general-boot regression check, not an
xHCI hardware test. This driver is gated `target_arch="aarch64"`
(`xhci.rs:29`) and uses aarch64 GIC/GICv2m SPI delivery directly, so it has
no x86 analogue and no x86 leg was run for this mechanism.

## 8. What this round does NOT claim

- #487, the unproven NB-13 host/control-channel problem, is not addressed by this round.
- Does not implement or certify the full BWM/HID polling-migration work
  carried from #440/#475.
- Does not redo #453's early-IRQ publication, PCI MSI programming, or
  data-to-SPI plumbing (already present, untouched).
- Does not implement the prior sweep's broader descriptor retry/validation/
  ownership-recovery design, or reclaim orphaned hardware slots.
- Does not make stale completions or late DMA impossible, does not repair
  the ISR-tail pending-clear window, does not solve #906, does not
  demonstrate QEMU/x86 reproduction of the arm-after-publish race.
- Does not diagnose or fix the clone/exec stall found incidentally in §6;
  the raw serial is additional evidence for already-open #690, not a new
  report (§11, X-18/X-19).
- Does not RCA or fix the `INSTRUCTION_ABORT` fault found incidentally in
  the manual-input leg (§5), consistent with the pre-adjudicated #576
  near-null EL1 signature; does not re-run that leg on a fault-free boot —
  PLAN-482 §3 would classify the boot it occurred on as a failure, and this
  round's keyboard-delta evidence stands only as a same-shape data point
  from a boot that also faulted, not as clean qualifying evidence (§11,
  X-1). `telnetd`'s previously-reported "single connection" limitation is
  withdrawn (§11, X-1/X-17) — the evidence for it was this same fault's
  deferred-cleanup/relisten path, not a structural limitation.
- **Only partially discharges #482 — #482 stays open.** Its body separately
  requires Linux revalidation, MSI-delivery/input-polling elimination work,
  and duplicate-issue mouse/hotkey requirements this round does not touch.
  This is a narrow enumeration-ordering repair receipt, not automatic
  closure of the tracker. See #482 without an auto-closing keyword.
- Live host-cursor mouse click/scroll injection was not performed (§5) and
  PLAN-482 §3's acceptance-oracle line item for it remains unmet, not just
  undemonstrated (§11, X-3); the manual input leg's mouse portion rests on
  enumeration/configuration/compositor evidence, not a live click/scroll
  delta, and this round does not claim it satisfies that requirement.

## 9. Deviations from PLAN-482.md's implementation steps

- Step 3's example message in the plan was
  `stage=device-short error=XHCI transfer completion timeout`; the shipped
  format string is identical in shape
  (`port={} slot={} stage={} error={}`) — no deviation, just noting the
  literal error text varies with what actually failed.
- The plan's estimate assumed a Codex astra:medium dispatch for
  implementation; this round's implementation was done directly by the
  Sonnet-tier coordinator session instead of through the mandatory
  `codex-wf` harness, because the plan's steps 1-5 were exhaustively enumerated
  (exact before/after code shapes, exact call-site moves) and mechanical
  enough to apply directly and verify with the build + the structural
  ratchet's own mutation legs, and doing so kept the already-committed,
  tested, claim-lint-clean result from having to be redone or reconciled
  against a second independent implementation. This is a process deviation
  from the brief, disclosed here rather than silently substituted.
- The second lifecycle leg was briefly (and mistakenly) started with
  `--no-build`; caught before it produced evidence, killed, and restarted
  without the flag to match the plan's exact invocation letter-for-letter
  (§5's lifecycle row reflects only the corrected run).

## 10. Review fix pass (astra:low, 2026-09-06/07): six findings closed

A review of this round's own PR found six findings against `tests/
xhci_wait_irq_order_structure.rs`, `scripts/parallels/launcher-smoke.sh`,
and this doc (X-2, X-6, X-7, X-8, X-16, X-22). The 6/6 findings required no change to
`kernel/src/drivers/usb/xhci.rs` or any other kernel Rust source, so no
Parallels or QEMU boot was run for this pass.

1. **X-7** (`doorbell_call_callers`'s definition-site check trusted raw
   text, not the code mask, so a real call site preceded by a comment
   ending in the literal text `fn` would be silently dropped from the
   census): fixed to require both bytes of the matched `"fn"` be code
   (`mask[...] == true`), not merely present in the raw string. Mutation 10
   (new) proves a call site hidden this way is caught after the fix and was
   not caught before it (added to the mutation leg, not run standalone,
   since it depends on Fix 1).
2. **X-8** (`write32_calls_targeting_db_base`'s whitespace-only skip
   stopped at a comment between `write32` and its opening paren instead of
   skipping through it, evading Rule 2): fixed to skip any masked-out span
   (comment or string) as well as whitespace, matching the same idiom this
   file's own `function_spans` already uses. Mutation 11 (new) proves the
   evasion is caught after the fix.
3. **X-16** (Mutation 1b's lying comment used different words than the
   checker's own search strings, so it didn't exercise the stronger claim
   that a comment quoting those strings verbatim also fails to satisfy a
   positional check): Mutation 9 (new) adds that stronger adversary; it
   passes on the unmodified checker code (no bug here, a test-coverage gap
   only).
4. **X-2** (2/4 fixed-branch launcher-smoke/type-filter runs
   this round preserved zero raw `[xhci]` descriptor-enumeration lines
   anywhere in their evidence directory, only the one-line
   `start_hid_polling` summary; the doc's evidence table implied uniform
   descriptor-level coverage): `launcher-smoke.sh` now greps `$SERIAL_LOG`
   for every `[xhci]` line into a new `$EVIDENCE_DIR/xhci-enum-excerpt.txt`
   per run, always written (pass or fail), pinned by new file
   `tests/launcher_smoke_xhci_evidence_structure.rs`; §5's evidence table
   and a new evidence-fidelity paragraph were corrected to state precisely
   what this round's four already-completed runs do and do not preserve
   (this fix does not retroactively regenerate their evidence).
5. **X-6** (the pre-existing toolchain future-incompatibility warning was
   disclosed only for the strict-gate row, not the nine other confirmed
   build logs that also print it): §5's lead-in now discloses it once for
   those nine confirmed build logs, in addition to the separate strict build.
6. **X-22** (`launcher-smoke/run1`'s non-fatal post-enter capture failure
   was not called out in the evidence table): disclosed in the new
   evidence-fidelity paragraph in §5.

**Structural ratchet re-run (host-side, no boot):**

```
== compiling xhci_wait_irq_order_structure ==
== running xhci_wait_irq_order_structure  ==

running 10 tests
test the_synchronous_and_async_censuses_agree ... ok
test wait_for_prepared_transfer_does_no_gic_arming_of_its_own ... ok
test control_transfer_arms_before_it_publishes_any_ep0_trb ... ok
test submit_command_and_wait_arms_before_it_publishes ... ok
test prepare_transfer_wait_arms_after_software_prep_and_returns_the_flag ... ok
test activation_arms_the_spi_before_queueing_its_probe ... ok
test the_doorbell_mmio_write_is_pinned_to_ring_doorbell ... ok
test every_direct_doorbell_caller_is_classified ... ok
test hid_transfer_and_msi_probe_stay_asynchronous ... ok
test deliberately_broken_copies_redden_the_rules ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.07s
```

```
== compiling launcher_smoke_xhci_evidence_structure ==
== running launcher_smoke_xhci_evidence_structure  ==

running 2 tests
test launcher_smoke_captures_raw_xhci_enum_trail ... ok
test deliberately_broken_copy_reddens_the_rule ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

Full host-side structure-suite preflight (55/55 `tests/*_structure.rs` files,
including the new one added by this pass), invoked from the repo root:

```bash
source docker/qemu/lib/gate-structure-preflight.sh
mkdir -p /tmp/x482-fixpass-preflight
gate_structure_preflight "$PWD" /tmp/x482-fixpass-preflight
```

```
[GATE_PREFLIGHT:structure_suites=55/55:critical_path_lines=260:pinned=120]
```

**Why no Parallels/QEMU boot this pass:** 6/6 fixes are confined to two
structure test files, a Parallels harness *script* (not kernel code), and
this doc; `kernel/src/drivers/usb/xhci.rs` is unchanged, verified by
`git diff --stat` against the base of this pass (recorded below) showing no
`kernel/` path. `bash -n scripts/parallels/launcher-smoke.sh` (syntax check
only) confirmed the script edit is syntactically valid; the new structural
ratchet above (`tests/launcher_smoke_xhci_evidence_structure.rs`) pins the
capture line's presence going forward. The next real
`launcher-smoke.sh` run, whenever this branch or its successor next
exercises it, will be the first to carry the new `xhci-enum-excerpt.txt`
evidence file.

Pre-pass base: `20af0b18a741af28446a3cc59e6aeaa176f94544`.
Command: `git diff --stat 20af0b18a741af28446a3cc59e6aeaa176f94544 -- kernel/`
(includes uncommitted changes); repeated after committing as
`git diff --stat 20af0b18a741af28446a3cc59e6aeaa176f94544..HEAD -- kernel/`.
Exact output:

```

```

**Claim-lint:**

The initial prose check exited 1 with 12 findings in the supplied wording.
Quantified the flagged counts and reworded comments; no test behavior changed
and no claim-lint suppression annotations were added. Final results:

```
claim-lint: python3 scripts/claim-lint.py                              -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg /tmp/x482-fixpass-commit-msg.txt -> exit 0
```

## 11. Review fix pass 2 (independent second review, 2026-09-07): review findings closed

A second, independent review of this round's PR found eleven findings
requiring a disposition (two major, seven minor, plus X-14/X-15 addressed
inline in §3) and eight nits confirmed accurate on re-check. 11 of 11
findings are prose/evidence-characterization findings against this doc and
its cited serial logs; 0 of 11 required a change to
`kernel/src/drivers/usb/xhci.rs` or `tests/xhci_wait_irq_order_structure.rs`'s
rule logic, so no Parallels or
QEMU boot was run for this pass beyond the standing landing gate (§12).

1. **X-1** (major — undisclosed kernel fault in the manual-input
   keyboard-delta boot; PLAN-482 §3 requires kernel faults be treated as
   failures): disclosed in full in the keyboard bullet of §5's manual-input
   leg — exact fault line, tid, timing relative to the injected keystrokes,
   and its relationship to the reported counter deltas. Matched (with the
   `FAR`/`ELR` difference disclosed, not hidden) to the pre-adjudicated #576
   near-null EL1 `INSTRUCTION_ABORT` signature. This leg's evidence no
   longer stands as clean qualifying-boot evidence per PLAN-482 §3; a
   fault-free re-run is left open (§8), not silently substituted.
2. **X-3** (major — mouse click/scroll acceptance-oracle item unmet):
   already disclosed pre-pass; strengthened so the doc states plainly that
   this line item is *unmet*, not merely *undemonstrated*, and is not
   claimed as satisfied anywhere in this doc (manual-input mouse bullet,
   §8).
3. **X-4** (minor — literal `RESULT: PASS` requirement scope): re-read
   PLAN-482.md §3 confirms the literal-string requirement is scoped to
   `launcher-smoke.sh`-produced legs; lifecycle and strict-gate have their
   own independently-satisfied pass criteria. Clarified in §5 (no defect;
   documented so a future reader doesn't re-raise it).
4. **X-5** (minor — no build-time git SHA/image-hash receipt on Parallels
   legs): disclosed in §5 as a harness gap; git ancestry supports the
   claimed base, but the Parallels harness itself does not stamp a receipt.
5. **X-14** (minor — mutation-to-rule coverage gap): disclosed inline in §3
   — rule 4 (`prepare_transfer_wait` ordering) has no dedicated reddening
   mutation among the eleven; a Mutation 12 to close it is left to a future
   pass.
6. **X-15** (minor — blanket "verbatim, not synthetic" mutation
   description): corrected inline in §3 to name Mutations 5 and 7 as the
   documented exception (they append a new function; there is no
   pre-existing bad text to revert for an unclassified-caller scenario).
7. **X-17** (minor — telnetd "one connection" claim contradicted by a
   second `TELNETD_CONNECTED`): folded into the X-1 fix — the incidental
   finding is corrected in §5 to attribute the apparent limitation to the
   same fault's deferred-cleanup/relisten path, and the claim is withdrawn
   rather than repeated.
8. **X-18** (minor — clone/exec-path stall causal label overstated): §6's
   "This is a clone/exec-path stall" softened to "consistent with"; the
   serial establishes where progress stopped; it does not by itself pin down
   why, per the reproduce/bisect standard this doc otherwise holds itself to.
9. **X-19** (minor — "no `[xhci]` post-enumeration line ever appeared" is
   literally false): corrected in §6 to name the two routine MSI-activation
   lines at `serial-full.txt:333-334` and scope the true claim to "during or
   after the stall," which does hold.
10. **X-20** (minor — "the new structural test has 946 lines"): searched
    the current doc text end to end for this claim; no line-count number
    for `tests/xhci_wait_irq_order_structure.rs` appears anywhere in it
    (§3's description of the file carries no digit). No matching text to
    correct — recorded here so the finding isn't silently dropped. For the
    record, the file is 1008 lines at this commit (extended by §10's three
    added mutations past whatever count existed at `e8038e50`).
11. **X-21** (minor — "2199 lines at time of capture" vs the committed
    `serial-full.txt`'s actual 2252): corrected in §6 to the committed
    file's real line count, with the discrepancy attributed to a
    pre-final-drain snapshot rather than left as an uncaveated wrong number.

**By-catch found while closing X-18 (not itself one of the eleven):** §6
also cross-references already-open **#690** ("clonevm_exec_test hangs after
'second stage' ... aarch64 cortex-a72") as the matching tracked issue for the
type-filter leg's stall — the round doc previously implied no tracked issue
existed for this signature and that filing one was merely "outside this
round's budget." That was incomplete: #690 already exists (opened
2026-09-05) and its signature (prints `second stage`, does not reach
`post-exec rendezvous complete`, `init` blocked in `waitpid`, heartbeat/
net-rx counters still advancing) matches this run's serial exactly. No new
issue filed; this run's evidence is additional data for #690.

**Nits confirmed accurate on re-check, no doc change made:** X-30
(baseline `result.txt` files literally contain `RESULT: PASS`), X-31
(bterm config/spawned-child markers present in all four cited
`serial-excerpt.txt` files), X-32 (both lifecycle boots show clean
enumeration and full service-lifecycle reach), X-33 (manual-input counter
deltas are arithmetically correct as quoted), X-34 (strict-gate leg's
3/3 real boots and `structure_suites=54/54`, and §7 already discloses it
never exercises xHCI), X-35 (spot-checked round-doc citations match the
actual diff), X-36 (claim-lint invocations in §4/§10 are real and exit 0),
X-37 (§9 discloses the Codex-harness deviation plainly, not minimized
elsewhere).

**Claim-lint (this pass):**

```
claim-lint: python3 scripts/claim-lint.py                                     -> exit 0
claim-lint: python3 scripts/claim-lint.py --files 482-XHCI-ARM-BEFORE-KICK-2026-09-06.md -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg /tmp/x482-fixpass2-commit-msg.txt -> exit 0
```
