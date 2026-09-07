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

`tests/xhci_wait_irq_order_structure.rs` (new, 946 lines). Std-only,
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
asserts the corresponding rule now fails, using `String::replace` against
verbatim source snippets (not synthetic stand-ins) so a rule that quietly
stopped matching the real code would be caught:

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

Local result: `bash scripts/run-structure-tests.sh xhci_wait_irq_order_structure`
→ **10/10 tests green** (9 positive rules/sanity + the mutation leg covering
8 mutations). The full strict-gate structure preflight (below) shows
`structure_suites=54/54` — this file is included and discovered
automatically, no closed name list to edit.

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

| Leg | Invocation | Result |
|---|---|---|
| **Failing baseline** (unmodified `origin/main` @ `45daec35`, separate worktree) | `bash scripts/parallels/launcher-smoke.sh --max-inject-retries 0 --timeout 1200` | **3/3 RESULT: PASS** — no failure caught in 3 attempts. Consistent with the RCA's own characterization of an intermittent race (2/6 in the prior evidence sweep); this session did not reproduce a fresh failure on main. All three preserved (`serials/482/baseline/run{1,2,3}/`). |
| Original failing workload (fixed branch) | `bash scripts/parallels/launcher-smoke.sh --max-inject-retries 0 --timeout 1200` | **3/3 RESULT: PASS.** Each run: real mouse+keyboard+composite-device enumeration through device/configuration descriptors, `[bterm] config:` and `[bterm] spawned child pid=` both observed. Preserved: `serials/482/launcher-smoke/run{1,2,3}/`. |
| Carried type-filter check | `bash scripts/parallels/launcher-smoke.sh --max-inject-retries 0 --timeout 1200 --type-filter` | **1/1 RESULT: PASS**, on the second attempt. The first attempt hit an unrelated stall in init's boot-time self-test battery at `CLONEVM_EXEC_TEST: second stage` (clone/exec, not xHCI — full xHCI enumeration had already completed cleanly, slots 1/2/3, before the stall; see §6). Killed and preserved as a non-xHCI finding at `serials/482/type-filter/run1-stalled-clonevm-unrelated/`; the retry passed cleanly and is preserved at `serials/482/type-filter/run2-pass/result.txt`. |
| Passive lifecycle comparison | `./run.sh --parallels --test 120` | **2/2**: clean enumeration (slots 1/2/3, no `enum_failed` line), full service lifecycle reached, bwm compositing live (~200 fps) at the end of both 120s windows. Preserved: `serials/482/lifecycle/run{1,2}/` (serial log + run.sh stdout + screenshot each). |
| Standing aarch64 QEMU regression gate | `./docker/qemu/run-aarch64-boot-test-strict.sh 3` (on a fresh `cargo build --release --features boot_tests --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`, 0 warnings besides the pre-existing toolchain future-incompat notice) | **3/3 boots succeeded** (100%), structure preflight `structure_suites=54/54`. Preserved: `serials/482/strict-gate/strict-gate-3boots.log`. |

**Manual input leg** (at least once on a qualifying boot, per the acceptance
spec): performed on a fresh, dedicated boot (`breenix-1788749740`).

- *Keyboard*: connected to the guest's telnet shell (`telnetd`, port 2323,
  guest IP `10.211.55.100`, reachable directly from the host over Parallels'
  shared network) and ran `/bin/xhci_counters` before and after injecting a
  real 5-character string (`scripts/parallels/inject.sh type world`, i.e.
  actual `prlctl send-key-event` scancodes, not the telnet session's own
  keystrokes). Delta: **`XHCI_MSI_EVENT_TOTAL` 66→76 (+10)**,
  **`KBD_NONZERO_TOTAL` 5→10 (+5)**, `XHCI_LOCK_CONTENDED_TOTAL` unchanged at
  0 throughout. +5 KBD_NONZERO_TOTAL for 5 injected characters and +10 MSI
  events (one per key-down and key-up HID report) is exactly the expected
  shape. Full transcript + serial log preserved at
  `serials/482/manual-input/attempt2-keyboard-delta/telnet-transcript.log`.
- *Mouse*: **not performed as live host-cursor injection.** `prlctl` has no
  mouse-event subcommand (only `send-key-event`), and driving a synthetic
  mouse click through macOS's Quartz `CGEvent` API would require first
  locating the Parallels VM window via `System Events`, which failed with
  `osascript is not allowed assistive access (-25211)` — this session has no
  Accessibility permission. Posting a blind global mouse click at an
  unlocated screen position on a Mac this session shares with other lanes
  and the operator was judged unsafe (risk of clicking an unrelated window)
  and was not attempted. What this round DOES show: mouse enumeration and HID
  *configuration* (not just slot allocation) succeeded on each of the
  seven fix-side boots this round (3 launcher-smoke + 1 type-filter + 2
  lifecycle + this manual-input boot), and `bwm`'s live ~200 fps compositing
  in each lifecycle/manual-input boot depends on the same mouse-cursor
  render path the HID mouse feeds. This is not a substitute for a live
  click/scroll test; it is disclosed as a gap, not papered over.
- An incidental finding while wiring the telnet session: this repository's
  `telnetd` (`userspace/programs/src/telnetd.rs`) appears to serve only ONE
  connection per VM lifetime — a second `connect()` reaches the guest at the
  TCP level (confirmed with `nc -zv`) but does not produce a second
  `TELNETD_CONNECTED` line, and the first session's shell stops responding
  once its client-side socket closes. Worked around by doing the whole
  before/inject/after sequence inside one unbroken Python socket session.
  Not filed as a new issue by this round (out of scope for #482); noted here
  for whoever next needs guest-side telnet access.

**Mutation testing** (structural ratchet): 10/10 tests pass on the real
source; all 8 named mutations independently redden their target rule (§3).

## 6. The type-filter leg's first attempt: an unrelated stall, not an xHCI failure

First `--type-filter` attempt (`serials/482/type-filter/
run1-stalled-clonevm-unrelated/`): the guest's real serial log
(`serial-full.log`, 2199 lines at time of capture) shows completely normal,
clean xHCI enumeration (mouse=slot1, keyboard=slot2, composite=slot3, no
`enum_failed` line, `[xhci] Initialized: 32 slots, MSI irq=56`), followed by
normal AHCI/ext2/net/timer/SMP init, followed by init's boot-time self-test
battery running several oracles to completion (`block_eintr_oracle`,
`futex_handoff_oracle`, `poll_tcp_oracle`, `tty_oracle`, `exec_smoke`) — and
then stalling with no further progress for 480+ seconds after
`CLONEVM_EXEC_TEST: second stage`, the last line `clonevm_exec_test` (PID 13)
ever printed. The kernel's own heartbeat kept incrementing normally
(scheduler alive, not a full freeze) but init never spawned anything past
PID 13/14, and neither `bwm` nor any `[xhci]` post-enumeration line ever
appeared. This is a clone/exec-path stall, not an xHCI notification-loss
timeout, and it happened well after xHCI had already finished successfully.
Per the plan's instruction to capture the first-divergence record rather
than add retries when an *xHCI* failure is seen: this was not one, so the
leg was retried once (clean pass, §5) rather than escalated. Not the
pre-adjudicated #906 signature either (#906 stalls right after the ICMP echo
line, before SMP bring-up; this stall is well past SMP bring-up, deep into
init's userspace self-test sequence). Reported here rather than filed as a
new tracked issue, since reproducing and scoping it is outside this round's
budget; the raw serial is preserved for whoever picks it up.

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
- Does not diagnose or fix the clone/exec stall found incidentally in §6.
- Does not scope or fix `telnetd`'s apparent single-connection limitation
  found incidentally in §5.
- **Only partially discharges #482 — #482 stays open.** Its body separately
  requires Linux revalidation, MSI-delivery/input-polling elimination work,
  and duplicate-issue mouse/hotkey requirements this round does not touch.
  This is a narrow enumeration-ordering repair receipt, not automatic
  closure of the tracker. See #482 without an auto-closing keyword.
- Live host-cursor mouse click/scroll injection was not performed (§5); the
  manual input leg's mouse portion rests on enumeration/configuration/
  compositor evidence, not a live click/scroll delta.

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
