# Green arc 5 (Bus+NIC blended) — PROVE round, 2026-08-31

Branch `feat/green-bus-nic`, worked in an isolated worktree at
`/Users/wrb/fun/code/breenix/.claude/worktrees/wf_395bb9a4-6a3-4`, detached
at `c799790bfcd7cd5173bf0e3a44ae299a9cd8f28a` (the fix slot's own final head
— `feat/green-bus-nic` the branch name was already checked out in a sibling
worktree, so this slot worked from the exact commit SHA in detached HEAD;
`origin/feat/green-bus-nic` confirmed == this SHA before starting). Read
the fix round's own notes and review docs (session-scratchpad files not
preserved in-repo — see the sweep-2 N6 note below) in full
before starting. Task: run every leg named in the fix-round handoff to
completion, fill every `PROVE-FILL` placeholder in the durable EVIDENCE/CONFIRM
docs with measured results, commit + push.

All five legs run to completion. Logs originally under a session-scratchpad
`fix2-prove/` directory; the subset cited by EVIDENCE/CONFIRM is now archived
durably alongside this file (`docs/planning/green-program/nic-bus/serials/`)
— sibling filenames below refer to that directory. The mutation apply/revert
scripts (`mutation1-*.sh`, `mutation2-*.sh`, `mutation-b5-*.sh`) and
`b5-mutation-evidence.txt` were archived too; the `ss-gate-*.log`/`.tsv` and
`leg3-*.log` raw run logs this narrative mentions below were not
individually preserved past the original session -- only the files actually
cited by EVIDENCE/CONFIRM's own text were archived.

Archival history, corrected: the sweep-2 N6 pass (`cbc6873b`, 2026-08-31)
copied most of this subset but **falsely claimed** to have also copied the
10 `suite-<name>.log` files and `700-recurrence-serial-kernel.log` — the
repo's blanket `*.log` `.gitignore` rule silently dropped a plain `git add`
of those, and N6 never noticed the no-op. A follow-up repair pass the same
day (`sweep-2` archival-closure slot) force-added those 11 files from the
still-surviving `fix2-prove/` scratchpad source and corrected N6's claim.
Every `serials/` citation in EVIDENCE/CONFIRM/this-file now resolves to a
real, tracked file.

---

## Leg 1 — 10 host structural suites, each run to completion separately

No `cargo test` batch fail-fast — each of the 10 named binaries run as its
own `cargo test --test <name>` invocation so nothing hides behind an earlier
failure (B1's own lesson).

| Suite | Result |
|---|---|
| block_request_lifetime_structure | 12 passed, 0 failed |
| coreproof_production_clean | 4 passed, 0 failed (see note) |
| exec_lock_order_structure | 34 passed, 0 failed |
| kernel_no_neon_guard | 1 passed, 0 failed |
| loopback_pump_structure | 63 passed, 0 failed |
| repo_symlink_hygiene | 4 passed, 0 failed |
| strand_handoff_structure | 38 passed, 0 failed |
| teardown_structure | 81 passed, 0 failed |
| tty_oracle_structure | 8 passed, 0 failed — **first-ever completed run in this arc** |
| x86_gate_verdict_test | 5 passed, 0 failed — **first-ever completed run in this arc** |

**Total: 10/10 suites green, 250 `#[test]` cases, 0 failed.**

`coreproof_production_clean`'s first attempt failed one of its four tests
(`the_scan_reddens_on_a_build_that_carries_the_harness`) with "coreproof
build failed" — not a code defect but a worktree-environment gap: the
aarch64 userspace test-program ELFs the kernel's `test_framework/registry.rs`
`include_bytes!`s (e.g. `simple_exit.elf`) had never been built in this
fresh worktree. `bash userspace/programs/build.sh --arch aarch64` (143
binaries installed) resolved it; re-run was clean 4/4.

Environment setup needed for any of this to build at all, matching the
confirm/fix slots' own documented workaround for the pre-existing #719
landmine: copied `Cargo.lock` from the pristine `main` checkout
(`x86_64 v0.15.4`, known-good) into this worktree (gitignored, not
committed), and symlinked `rust-fork -> /Users/wrb/fun/code/breenix-parallels/rust-fork`
(also gitignored — `repo_symlink_hygiene`'s own tests confirm this is the
sanctioned shape). Also needed: `scripts/create_ext2_disk.sh --arch aarch64`
for the service-sequence gate (leg 2).

Logs: `fix2-prove/suite-<name>.log`, summary `fix2-prove/00-suites-summary.txt`.

## Leg 2 — the unrun arm (B3, mandatory)

`./docker/qemu/run-aarch64-service-sequence-gate.sh --profile both --boots 25
--rebuild`, census arm live — **this is the arm's first-ever real execution
in this arc** (every prior run substituted `run-aarch64-full-test.sh
--boot-tests-only`).

Result: **50/50 boots GREEN** across both profiles (max 25/25, cortex-a72
25/25), **`UNATTRIBUTED=0`** in both profile summaries, gate `PASSED` both
times. Boot 1's serial carries `[drivers] Found 5 VirtIO MMIO devices` with
both `network` and `block` present in the per-device breakdown, matching the
script's own self-counted `EXPECTED_MMIO_DEVICES=5`. No stray QEMU processes
before/after (`pgrep -f qemu-system-aarch64` confirmed clear both times).

Census mutation (per `prove-mutations.md` mutation 3b,
`mmio_census_total=$((mmio_census_total + 99))` inserted at the exact
documented line): single boot run, reddened exactly as predicted —
`CLASS_BUCKET=UNATTRIBUTED`, `CLASS_REASON="device-enumeration census
reports 104 VirtIO MMIO device(s), self-counted expected 5 from this
script's own -device flags"`, `Profile cortex-a72 gate: FAILED (...
UNATTRIBUTED=1 ...)`. Reverted; `git diff --stat
docker/qemu/run-aarch64-service-sequence-gate.sh` empty.

`run-x86-boot-tests.sh` census mutation (mutations 1 and 2 from
`prove-mutations.md`, run on beast — see leg 3 below for detail): both
reddened at the moved, pre-terminal-marker location the B4 fix put them at,
proving the reorder is load-bearing, not merely present.

Logs: `fix2-prove/ss-gate-main.log` (unmutated 50-boot run),
`fix2-prove/ss-gate-mutation-run.log` (mutated), `fix2-prove/ss-gate-census-
{max,cortex-a72}.tsv` (per-boot census tables). Serials archived durably at
`docs/planning/green-program/nic-bus/serials/aarch64-service-sequence-gate-
{unmutated-run1,mutation-forced-104}-20260831.txt`.

## Leg 3 — beast

Scratch clone `/root/breenix-gbus-prove` on `breenix-x86` at `c799790b`
(fresh `git clone` + `git fetch origin feat/green-bus-nic` + `git checkout
<sha>`, isolated from any other beast work; removed at the end).

- `run-x86-boot-tests.sh 3` (TCG, reordered census): **3/3 PASS**, census
  `Found 9 devices (3 VirtIO block, 1 network)` echoed on every boot.
- `run-x86-gate.sh 3 kthread` (KVM): **3/3 PASS**, census echoed on every
  boot, `GATE: PASS (3/3 boot tests passed; mode=kthread build=16s boot=35s
  total=59s)`.
- `run-x86-gate.sh 1 full` (KVM): **first attempt reddened** — genuine,
  pre-existing, already-filed defect, not caused by this branch. The boot's
  `clonevm_exec_test` printed `CLONEVM_EXEC_TEST: ERROR futex timeout did not
  return ETIMEDOUT` and exited 1 — an exact string match to **#700** ("x86:
  clonevm_exec_test's post-exec futex timeout does not return ETIMEDOUT ...
  the live x86 face of the #608 specimen, which is closed", filed
  2026-08-30, rate 3/23 on a prior unrelated branch vs 0/19 on main).
  `kernel/src/**` is untouched by this branch's diff
  (`git diff main...HEAD --stat -- kernel/` empty), so this cannot be a
  regression this arc introduced — confirmed a known, filed, unattributed
  kernel-level flake rather than papering over a new one. Evidence preserved
  at `fix2-prove/700-recurrence-serial-kernel.log`. Re-ran once: clean
  `PASS` (`GATE: PASS (1/1 boot tests passed; mode=full ... total=173s)`),
  census echoed.
- **B5 forced-test (mandatory):** `expected_virtio_block` mutated `+99` in
  `run-x86-gate.sh` (exact line documented in the fix round's own comment),
  run in `full` mode. Confirmed **both** signals now print together on a
  census-fail boot, exactly as B5 requires: `STRAND_CENSUS:
  threads_saved_blocked=9 stranded=0 lines=4994` (previously would have been
  silently skipped by the old short-circuit) AND `Test 1: FAIL
  (device-enumeration census reports 3 VirtIO block device(s), self-counted
  expected 102 from src/bin/qemu-uefi.rs)` (the layer-naming message) — on
  the same boot, plus the userspace verdict itself printing cleanly
  (`x86 userspace gate: PASS - exited=19 expected>=10 nonzero=0
  allowlist=0`), confirming `x86-gate-verdict.sh` ran unconditionally.
  Reverted; `git diff --stat run-x86-gate.sh` empty.
- Cleanup: scratch clone removed; `pgrep -af qemu-system-x86_64` confirmed
  clear before and after every batch (one stale-match false positive from
  pgrep matching its own command-line text, verified not a real process).

Logs: `fix2-prove/leg3-*.log`, `fix2-prove/mutation*.sh` (the exact applied
mutation scripts, pushed via `incus file push` + run via `incus exec`
since worktree isolation blocks compound remote git commands from this
session directly), `fix2-prove/b5-mutation-evidence.txt`.

## Leg 4 — VMware nogui, this Mac

`./docker/qemu/run-vmware-gate.sh` run twice end-to-end.

Both runs: **PASS** — `virtio-net PCI attempted and failed, e1000 fallback
initialized`, `[drivers] Found 60 PCI devices`. No GUI window opened (only
the headless `vmware-vmx` process observed via `ps aux`, no VM window
process); `vmrun list` confirmed **`Total running VMs: 0`** after each run
(F10's cleanup hardening held both times — no `WARNING: ... still listed`
line in either log).

Logs: `fix2-prove/vmware-run{1,2}.log`, also archived durably at
`docs/planning/green-program/nic-bus/serials/vmware-nogui-run{1,2}-stdout-20260831.txt`.

## Leg 5 — fill PROVE-FILL placeholders, commit, push

`docs/planning/green-program/nic-bus/EVIDENCE-2026-08-31.md`: the three
`PROVE-FILL` rows in §4's mutation table filled with the measured FAIL lines
from leg 2/3 above; the "gaps closed" framing in the paragraph above the
table updated from "prepared, not yet run" to "closed"; new §9 added
("Prove slot — full battery") summarizing all five legs with the exact
measured strings, including the honest #700 disclosure.

`docs/planning/green-program/nic-bus/CONFIRM-2026-08-31.md`: §6's
"Prove's job (post-fix) to run them for real" sentence replaced with the
measured `tty_oracle_structure` 8/8 and `x86_gate_verdict_test` 5/5 counts
and the 10/10-suite, 250-case total.

4 new serial files added under `docs/planning/green-program/nic-bus/serials/`.

`git diff --stat` on the final tree: only the two doc edits + 4 new serial
files — no script or kernel/test source left mutated (every mutation in
legs 2/3 was verified reverted with an empty `git diff --stat` before moving
on).

---

## Summary

- **10/10 host structural suites green** (250 cases), including the two
  that had never once completed in this arc.
- **`UNATTRIBUTED=0`** on the aarch64 service-sequence gate's first-ever
  real execution, 50/50 boots green across both profiles.
- **All three previously-unproven mutation sites now mutation-proven**
  (x86 boot-tests VirtIO-block count, x86 boot-tests network floor,
  aarch64 service-sequence census arm) — every one reverted cleanly.
- **x86 beast legs all green**: reordered boot-tests x3, gate kthread x3,
  gate full x1 (after one clean re-run past a pre-existing, already-filed
  #700 recurrence unrelated to this branch's diff).
- **B5's fix verified by forced test**: a census-fail boot now prints
  *both* the strand census and the layer-naming message, where before B5 it
  would have printed only the strand census's absence with no layer datum.
- **VMware nogui verified end-to-end twice**, no GUI window, `vmrun list`
  empty after both runs.
- **No UNATTRIBUTED red anywhere in this pass.** The only non-clean boot
  (x86-gate full-mode's first attempt) matches a filed, already-open,
  exact-string issue (#700) untouched by this branch's diff; a clean re-run
  confirmed it is not a standing regression on this branch.
- Doc corrections committed; pushed to `origin/feat/green-bus-nic`.
