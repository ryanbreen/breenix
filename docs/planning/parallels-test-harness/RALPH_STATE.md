# Parallels Launcher-Test Harness — Ralph State

**Goal (operator, 2026-06-01):** Build an automated testing framework that drives the
real GUI input path inside Parallels — simulate the launcher gesture, open the launcher,
launch the terminal, type into it, and validate it works — so we can test at scale.

**Exit criteria (hard):** the `parallels-launcher-test` workflow reports
`consecutiveGreenAchieved = true` — **10 consecutive green runs** of
gesture → launcher opens → select terminal → Enter → `/bin/bterm` launches, validated.

## Loop protocol (sequential Ralph)
Each turn = **implement/fix the framework, then validate with 10 consecutive runs.**
Stop the loop only when 10-in-a-row pass. Diagnose failures honestly — if a failure is a
real Breenix launcher bug (not a harness timing issue), surface it; do not weaken the test.

## Status — EXIT CRITERION MET (2026-07-04)

**The hard exit criterion is MET: 10 CONSECUTIVE CLEAN GREEN runs**, attempts 1-10,
all `RESULT: PASS` with `inject_retries=0` (no retried/flaky passes — a genuinely
clean streak). Evidence dirs:
`logs/parallels-launcher-test/run-20260704-133953` through
`logs/parallels-launcher-test/run-20260704-141143` (10 dirs: `-133953`, `-134312`,
`-134637`, `-134937`, `-135244`, `-135559`, `-140026`, `-140401`, `-140704`,
`-141143`), each with `result.txt` showing `RESULT: PASS`, `inject_retries=0`.

**The loop is COMPLETE, pending PR merge.**

### What today's session fixed en route to the clean streak
1. **Intermittent GPU-init boot failure** — the 2D framebuffer was BSS-backed and
   not DMA-mappable; moved to heap allocation (`286dafe5`).
2. **Harness honesty fixes** (the gate was previously able to score false greens):
   - Fault-check precedence over PASS — a kernel fault occurring after the
     readiness marker was previously masked by an early PASS verdict; fixed to
     check faults first (`00e4a699`).
   - Retried passes are no longer counted as "clean green" — a run that needed
     input-injection retries is distinguished from a truly clean pass (`8b0abf98`).
   - `--no-build` stale-`.hds` deploy trap — running with `--no-build` could
     silently deploy a stale disk image; closed (`7280a8ba`).
3. **Host input wedge (intermittent keyboard-delivery drops)** — added a
   keyboard-delivery handshake via a new `kbd_nonzero` counter (`30bec1bb`,
   `607ffee0`), with evidence-gated `ENV` classification so a host-side wedge is
   detected and excluded rather than silently miscounted as a kernel bug
   (`0a820bd8`, `a3275dca`).
4. **Fork-kstack scrub + child resume-state reset** (`e65f23cd`) — a plausible
   fix for the intermittent EC=0x0 launcher-spawn crash (see
   `docs/planning/aarch64-launcher-spawn-crash/ROOT_CAUSE.md`). Zero faults
   observed in 25 runs since landing this fix, versus roughly 1-in-15 before.
   **Not proven** — see round-4 RCA addendum in ROOT_CAUSE.md; the underlying
   writer is still unlocated, and this fix addresses only one of two candidate
   upstream writers.
5. **Instrumentation armed for any recurrence** (kept in the tree, does not gate
   the harness): all-CPU `SAVE_SKEW` readout (`97f6e834`, `b0c4ab7c`), an ERET
   frame-anomaly recorder + owner-tid canary (`40ad7042`, `40c187e9`).

### Known open items (not blocking, tracked for follow-up)
- **(a) EC=0x0 crash — unconfirmed-fixed.** The kstack scrub (`e65f23cd`) is a
  plausible mitigation, not a proven fix (0 faults in 25 runs since, vs ~1/15
  before). If it recurs, the armed `[ERET_ANOMALY]`/`owner_tid_canary` probes in
  the postmortem will pin the writer. An eventual definitive fix likely touches
  FROZEN gold-master regions (idle SP handling / ERET dispatch) — **operator
  signoff required before implementation.**
- **(b) Host-side `CUsbKeyboard` wedge** — detected and excluded with evidence
  (see fix #3 above); the underlying Parallels-side cause is unaddressed.
- **(c) Rare AHCI boot hang** — 1 occurrence, `run-20260704-105450`; not
  reproduced again, not investigated further.

## VERDICT (2026-06-01 night — superseded by 2026-07-04 above)
- **Harness: BUILT & verified.** `scripts/parallels/inject.sh`, `scripts/parallels/launcher-smoke.sh`, `.claude/workflows/parallels-launcher-test.js`, `docs/planning/parallels-test-harness/README.md`. Injection method isolated to one config block (`SUPER_PREFIX=224 SUPER_CODE=91 INTER_TAP_MS=150 ENTER_CODE=28`).
- **Parallels injection blocker ROOT-CAUSED: the macOS screen is LOCKED.** `CGSSessionScreenIsLocked=True` → VM console detached → `prlctl send-key-event` accepted (rc=0) but silently dropped (functional `=`-into-Bounce test: no effect; no hotkey `[spawn]`). NOT a TCC grant (send-key-event injects into the virtual XHCI HID via prl_disp_service, not via macOS CGEvent/PostEvent). NOT a run.sh misconfig. Guest USB keyboard is healthy/enumerated — input just never lands. Evidence: `logs/parallels-launcher-test/unblock-2026-06-01-rootcause.txt`.
- **OPERATOR ACTION to validate on Parallels:** physically unlock the Mac at the console, then `caffeinate -d &` (prevent re-lock), then run `bash scripts/parallels/launcher-smoke.sh` (or the `parallels-launcher-test` workflow). There is no non-interactive unlock bypass.

## QEMU logic-validation pivot — EVALUATED, NOT VIABLE
We considered QEMU as a lock-independent alternative (QEMU injects keys via its own
monitor, not macOS events). It does **not** work for this flow, for two independent reasons:
- **BWM never starts on QEMU** — BWM's ARM64 path needs the VirGL 3D compositor
  (Parallels-specific; absent on QEMU here), so the window manager never comes up.
- **SUPER never observed on QEMU** — the double-tap-Super hotkey reads `SUPER_PRESSED`
  only from the USB-HID/xHCI driver, which never enumerates on QEMU. The `virtio-keyboard`
  MMIO driver never tracks Super, so the gesture can't be recognized.
Making QEMU viable would require kernel changes (software-compositor fallback for BWM +
a `virtio-keyboard`→SUPER bridge) — out of scope for this host-side harness.
For reference, the working QEMU ARM64 boot recipe is `-M virt,gic-version=3 -cpu max`
(run.sh's `cortex-a72` hangs); run.sh exposes a monitor on `tcp:127.0.0.1:4444` + QMP at
`/tmp/breenix-qmp.sock`.
**Conclusion: the 10× validation must run on Parallels with an unlocked Mac. No QEMU substitute.**

## Architecture decisions (resolved this session)
- **Trigger is double-tap SUPER, not double-Control.** `bwm.rs` `load_defaults()` (aarch64,
  hardcoded; config loading is x86-only) binds `SUPER+SUPER (taps=2) → exec /bin/blauncher`
  and `SUPER+Return → exec /bin/bterm`. The operator's "double control key" = the
  double-tap-Super gesture (Mac Command maps to guest Super). We test the launcher path.
- **Injection = `prlctl send-key-event <VM> --scancode <ps2-set1> --event press|release`**
  (NOT CGEvents — no Accessibility/focus needed; Parallels translates set-1 → guest USB-HID).
  ASCII proven in `scripts/parallels/type-in-vm.sh`. Super = extended `0xE0 0x5B` (224 then 91)
  — exact prlctl form determined empirically by the spike phase.
- **Validation = serial markers (primary) + `scripts/parallels/capture-display.sh` PIL pixel
  probe (secondary).** PASS requires real evidence `/bin/bterm` launched — never "process created".
- **VM lifecycle:** only via `./run.sh --parallels [--no-build]` (fresh epoch VM, tails serial
  forever → background it; serial at `/tmp/breenix-parallels-serial.log`; ~60-90s VirGL warmup
  before capture is trustworthy).

## Deliverables
- `scripts/parallels/launcher-smoke.sh` — one full run → `RESULT: PASS|FAIL` + evidence.
- `.claude/workflows/parallels-launcher-test.js` — runs the smoke script sequentially up to
  15×, requires 10 consecutive PASS, reports the streak + first failure.
- `docs/planning/parallels-test-harness/README.md` — the proven recipe + how-to.
- Evidence under `logs/parallels-launcher-test/`.

## Next action when the construction workflow completes
- `ok=true` → invoke the `parallels-launcher-test` workflow for the 10× gate.
- failed at Boot/Spike → diagnose (injection timing vs. real Breenix launcher bug),
  fix host-side or report the Breenix bug, then re-run.
- After 10 green → commit the harness on a feature branch, open a PR, notify operator.

## COMPLETE (2026-07-04)
10/10 clean greens achieved — see "Status — EXIT CRITERION MET" above for the
evidence paths and the list of fixes landed this session. **The loop is done;**
the only remaining step is merging the branch (`feat/parallels-launcher-test-harness`)
via PR. Open items (a)/(b)/(c) above are tracked but do not block merge — none of
them are harness defects; (a) and (b) are underlying Breenix/Parallels behaviors
the harness now correctly detects and reports rather than silently masking.
