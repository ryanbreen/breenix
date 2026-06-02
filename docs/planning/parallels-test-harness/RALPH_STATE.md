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

## Status
- **Phase 1 — ship branch: DONE.** `fix/aarch64-stale-cached-ttbr0-dispatch` → PR #410 → merged to `main` (`134c532b`). Local `main` synced.
- **Phase 2 — construction workflow: COMPLETED, blocked at spike.** Run `wf_c890dfff-d68`.
  - Boot ✅ VM `breenix-1780359459`, BWM compositing. Ready marker: `[bwm] hotkeys: using built-in defaults for early boot`.
  - Code-recon ✅ Full recipe known: trigger=double-tap Super (`bwm.rs:315`); `blauncher` pre-selects `APPS[0]="Terminal"` → Enter alone launches `/bin/bterm`. Oracles: `[spawn] path='/bin/blauncher'`, `[spawn] path='/bin/bterm'`, `[bterm] config:`.
  - Spike ❌ **HARD host-side blocker:** `prlctl send-key-event` accepted but keystrokes DROPPED before the guest (modifier-free `=` into focused window changed nothing; no hotkey `[spawn]`). Evidence points to missing macOS TCC Accessibility/Input-Monitoring for Parallels + a detached VM GUI view (stale `prlctl capture`). Spike wrote `logs/parallels-launcher-test/inject.sh` + evidence.
  - **OPEN QUESTION being resolved:** is the blocker the detached/headless window (autonomously fixable) or a TCC grant (needs operator)? Decisive test: bring VM window on-screen+focused, inject `=` into Bounce, watch speed.

## VERDICT (2026-06-01 night)
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
