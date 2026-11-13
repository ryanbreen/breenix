# A.5 u5: ARM gui-apps baseline

Final baseline: **FAIL, 68/73 PASS, 5 FAIL, 0 SKIP**. One actual guest boot:
Vigil `20261113T040422Z-qemu-87249`, commit `be737c42`, clean tree, ARM64 HVF,
SPICE GL, `display=false`. MEMORY PASS and ANGLE/Metal accepted. The failures
and full case table are below. Host preflight repairs have deterministic checks;
this unit makes no guest fixes and runs no second guest boot.

## Original prediction

Runtime head before this document: `e850c3fb`. This unit changes no runtime
behavior. The existing 73-case manifest is the check. Question: does the ARM
HVF launch integrate the shared GUI cases, desktop services, SMB restart and
slirp evidence routes? Predict 73 PASS, zero FAIL or SKIP; no specific case
failure is predicted from the code. One headless SPICE GL boot will answer it.

| Category | Prediction | Code basis |
|---|---:|---|
| terminal-emulation | 5/5 PASS | Scripted screens and keys: `userspace/programs/src/guiapps_suite/terminal_emulation.rs:24` |
| bterm | 9/9 PASS | Prompt, command, tabs, resize and clipboard: `userspace/programs/src/guiapps_suite/bterm.rs:4` |
| blauncher | 8/8 PASS | Filtering and real application launch: `userspace/programs/src/guiapps_suite/blauncher.rs:4` |
| blog | 8/8 PASS | File following and daemon restart: `userspace/programs/src/guiapps_suite/blog.rs:17` |
| bfontpicker | 6/6 PASS | Font selection and persistence: `userspace/programs/src/guiapps_suite/bfontpicker.rs:38` |
| bcheck | 6/6 PASS | Result and shipped network catalog: `userspace/programs/src/guiapps_suite/bcheck.rs:13`, `:92` |
| guskit | 13/13 PASS | Drawing and file operations: `userspace/programs/src/guiapps_suite/guskit.rs:33`, SMB cases `:261`, `:300`, `:363` |
| demos | 6/6 PASS | Real demo launch and interaction: `userspace/programs/src/guiapps_suite/demos.rs:25` |
| guskit-collab | 6/6 PASS | Local peer synchronization and ordering: `userspace/programs/src/guiapps_suite/guskit_collab.rs:133` |
| desktop | 6/6 PASS | Init, launch and cross-app interactions: `userspace/programs/src/guiapps_suite/desktop.rs:4` |

The suite selects a supervised session per case and defers display
(`userspace/programs/src/suite_gui_apps.rs:24`, `:118`). ARM routes the suite
through init (`kernel/src/main_aarch64.rs:310`); init recognizes the staged
selection and waits for shares and clock (`userspace/programs/src/init.rs:173`,
`:387`, `:427`). QEMU stages the selection and private SMB files
(`scripts/qemu-desktop-stage.py:13`, `:34`), starts the evidence receiver
(`scripts/boot-interactive.sh:398`) and answers SMB stop/start requests
(`scripts/boot-interactive.sh:571`, `scripts/qemu-smb-restart.py:62`).
Predict successful stop and start receipts, MEMORY reclaimed PASS and standalone
READY after DONE and MEMORY (U19), uploaded evidence, and a passing host
renderer check. These are independent requirements, not inferred from case
counts. No failure mechanism is asserted without capture evidence.

Comparison baseline: R240 `20261112T190204Z-metal`, 73/73 and MEMORY PASS,
as recorded in the E23 ledger at line 457; later records will be checked
before attributing failures. This is a comparison, not an ARM result.

## Earlier attempt: blocked before boot

The one launch attempt at `d46f079c` exited 1 before QEMU started. ARM
userspace and the production kernel built successfully; the kernel build
printed the existing Rust `core` future-incompatibility notice. The ext2
image was created and the kernel SIMD guard passed. No guest case ran.

The launcher invokes `scripts/qemu-desktop-stage.py` at
`scripts/boot-interactive.sh:366`. Its import at
`scripts/qemu-desktop-stage.py:9` loads `scripts/metal/metal_root.py`, whose
line 21 imports `metal_probe` as a top-level module. The staging invocation
does not put `scripts/metal` on Python's import path. It failed with
`ModuleNotFoundError: No module named 'metal_probe'`. This is an ARM launcher
staging failure; no claim is made about the shared guest cases.

Vigil ID, renderer result, MEMORY, SMB restart receipt and all 73 per-case
results: **not produced**. The R240 comparison above therefore cannot classify
any guest failure. No fix or second launch was made; the blocker was filed in
the ARM effort ledger. The branch-tools lock was released after the failed
preflight.

## Resumed prediction

The resumed unit keeps the runtime tree unchanged. The earlier launch did not
start a VM, so the authorized single guest boot remains unspent. The import
failure reproduces with `env -u PYTHONPATH python3 scripts/qemu-desktop-stage.py
--help`. Supplying `PYTHONPATH="$PWD/scripts/metal:$PWD/scripts"` permits that
command and the launcher's inline SMB selection/site import to exit with status 0.
The dependency is the sibling import at `scripts/metal/metal_root.py:21`;
`scripts/qemu-evidence.py:14` already establishes that directory for its own
imports. Both QEMU callers need the environment (`scripts/boot-interactive.sh:366`,
`:369`). Choose an explicit launch environment for this no-fixes baseline;
leave an actual launcher repair to a later unit. No checks or expectations change.
No GPU source or generated asset changes, so no MIT-source interpretation is
needed. The 73-case manifest remains this unit's existing check (U29).

The category predictions above stand. The unresolved integration question is
whether ARM's init route actually reaches the shared case sessions with mounted
shares, synchronized time and the evidence receiver, then recovers the SMB
server and completes DONE/MEMORY/READY. The existing per-case diagnostics,
UI_CONTROL records, evidence receipts and SMB helper transcript distinguish
those outcomes. Predict 73 PASS, MEMORY PASS, both SMB actions successful and
ANGLE/Metal renderer accepted. A failing capture ends the unit without a fix
or another boot. Compare with main's latest R240 gui-apps capture
`20261112T190204Z-metal` (E23 ledger line 457, 73/73, MEMORY 27/48 pages).
Fetched `origin/main` remains `d3041e9e`; the branch's differences from it are
ARM routing, QEMU host staging/helpers, host checks and this report.

Validation before the resumed boot: import preflight with explicit search path
exited with status 0; no source or check changed, so no repeated Rust build is required
outside the launcher's own ARM artifact preparation. Builds use
`CARGO_BUILD_JOBS=4` and the launcher's taskpolicy background wrapper.

Claim checks for this prediction: `scripts/claim-lint.py --base HEAD` initially
exited 1 on two unquantified prose claims; after naming exit status 0, it
exited 0. Commit-message lint exited 1 twice on unquantified claims; the amended
message exited 0.

## Host staging repair before the guest boot

The explicit environment passed the first staging command, but the inline SMB
command replaced PYTHONPATH with scripts alone and failed before QEMU again.
The resumed launch log is `/tmp/a5-u5-resumed-a8bb2637/launch.log`.
The ARM builds and SIMD guard completed; there was no guest or Vigil id.
The only build warning was the previously recorded pinned Rust core
future-incompatibility notice. The earlier environment-only choice is superseded.

The focused check in `tests/qemu_desktop_import_test.py:12` exercises staging
help and the launcher's actual inline selection without inherited PYTHONPATH;
both failed on `a8bb2637` with the same missing sibling import. It was committed
alone as `65f27dfd`. Repair only the QEMU host consumers: staging establishes
scripts/metal just as qemu-evidence does, and the inline query supplies both
scripts and scripts/metal. The check then passed (1 test, 2 entry points), and
`bash -n scripts/boot-interactive.sh` exited 0. There is no changed guest or
shared metal code; this repair reaches no x86 suite, and no R240 boot is needed.
The Python launcher consumes the same ARM artifacts, so the next launch uses
`--no-build` to avoid rebuilding the unchanged artifacts. The 73-case prediction
stands, with the one guest boot still unspent. Guest fixes remain out of scope.

Claim checks: `scripts/claim-lint.py --base HEAD` and commit-message lint exited
0 for the check commit. For this host repair they also exited 0.

## Authoritative lease preflight repair

The `27a07a27` launch passed the imports and staging, then timed out before
QEMU in the SMB lease preflight. Capture:
`/tmp/a5-u5-baseline-27a07a27/launch.log`. Its helper scanned historical
`boots/*/claim.json` records, and the remote command exceeded 30 seconds.
Read-only inspection found 1,967 historical directories and an absent current
lease. The current machine lease is `lease.json`, read by
`scripts/metal/metal_lib.py:146` and documented in `docs/metal/README.md:357`.

Check `5c6b30f2` adds one fixture test for the actual remote check body: absent
lease, active lease, malformed lease. It failed before the repair (three
subcases). Replace the historical scan with a direct authoritative lease read;
absence permits the launch, a present lease refuses it, and unreadable or
malformed state refuses it. The four SMB helper tests passed, including the
existing active-lease rejection and stop/start/cleanup checks. The live
read-only preflight then exited 0. The 30-second timeout and server safety
condition are unchanged. No shared metal code, guest code or checks change.
The one guest boot remains unspent. Reuse the same ARM artifacts.

Claim checks: diff and commit-message lint exited 0 for `5c6b30f2`; diff and
commit-message lint also exited 0 for the authoritative-lease repair.

## Final capture and verdict

One actual guest boot ran at `be737c42023cd40f9805baf0de14a537faa99a7b`:

```
CARGO_BUILD_JOBS=4 env -u PYTHONPATH scripts/boot-interactive.sh \
  --mode suite --suite gui-apps --gpu-gl --no-display --no-build \
  --serial-log /tmp/a5-u5-baseline-be737c42/serial.txt
```

The preceding preflight prepared ARM userspace, root image and production
kernel using the launcher's taskpolicy background wrapper. Those artifacts
were reused after the QEMU-only host repairs; kernel and userspace source did
not change. No x86 compile or boot was reached by these repairs. No GPU asset,
toolchain, scorer, suite expectation or threshold changed.

Vigil's stored serial is byte-identical to the private capture. Its record
contains commit `be737c42`, dirty=0, display=false, arch=aarch64, platform=qemu,
suite=gui-apps, ended timestamp and exit-status **130**. `run.json` identifies
the launch shape as standalone. The private capture directory is
`/tmp/a5-u5-baseline-be737c42`; retain its writable image, launch.log,
serial.txt.evidence, shares.log, services.txt and report-data.json. Serial SHA-256:
`796aabcf1d56ed6bc75c14a07d9c8cea636ae9e077bcc807d381ae72af437625`.

The guest emitted DONE at serial.txt:5656 (68 passed, 5 failed, 0 skipped,
73 total), MEMORY reclaimed=PASS at :5665, and standalone READY at :5677.
Suite elapsed time was 124628 ms. MEMORY free_before=79204,
free_after=79866, allowance=48 pages: 662 more free pages at the end.
Init reported runner exit code 1 at :5675, stopped its services, remounted
root read-only and requested poweroff. The watcher labeled this QEMU exit
as operator cancellation and returned 130. No operator cancellation occurred.
`watch-qemu.py:124-128` maps a nonfatal QEMU exit to cancelled before the
panel hold completes; `:169-171` then bypasses clock/suite scoring. This is a
shared QEMU observer defect, tracked in #2927, with no R240 analogue because
metal uses a different launcher. It does not turn the failed cases into a pass.

The unchanged scorer was run once over Vigil's retained serial:
`scripts/suite-verdict.py docs/suites/gui-apps.json <Vigil boot>/serial.txt`.
It exited 1: `FAIL: SUITE gui-apps ERROR runner exited code 1`.
The host renderer checker ran once inside the launcher and printed
`host renderer: accepted` for ANGLE/Metal, **1102 records**; its artifact is
`host-renderer.json` in both the capture and Vigil record. The cancellation
path skipped the separate wall-clock verdict; clock synchronization reached
ready during init, but this report claims no missing clock verdict.

The receiver retained startup and final panel anchors/receipts. Serial reports
startup bytes=3397956 with sha256
`18cf88a328d930da74c14cadba2747e3c20e6c9611dacb910c1ac3614c3b495e`,
and suite-final-panel bytes=180849 with sha256
`f527ee1b322b06f42bf755e25c2b5b7c9c6d9b4b037249dc7a181069747ba964`.
The two startup picture receipts and final receipt have complete=true; private
route material stays in the capture.

The required `serial.txt.smb-restart.json` is:

```json
{
  "requests": []
}
```

This is **not SMB restart success**: the precondition failed before the case
could request STOP or START (`guiapps_suite/guskit.rs:364`). The host server
was active and a host TCP connect to the configured endpoint succeeded during
this boot. The helper changed no SMB state. The owned branch-tools lock was
released after guest shutdown, guarded by its owner token.

## Failure attribution against main

Comparison: latest main R240 gui-apps record `20261112T190204Z-metal`, accepted
in E23 ledger line 457, 73/73 PASS and MEMORY PASS (27/48 pages). Its retained
serial has a passing record for each of the five failed ARM cases. The shared
case sources match main; ARM-only below means an observed ARM parity failure
against that record, not proof that every reachable shared function is sound.

| Failed case | Capture / code attribution | Classification and follow-up |
|---|---|---|
| guskit/smb-save-open | ARM serial.txt:3670, ms=0, shares failed; call `guiapps_suite/guskit.rs:262` -> `:223-226` -> `libs/libbreenix/src/services.rs:641-650`; R240 serial.txt:5353 PASS 2851 ms | ARM-only observed; #2878 |
| guskit/smb-save-fails-cleanly | ARM :3680, ms=0, same service precondition; call `guiapps_suite/guskit.rs:301` -> same helper; R240 :5412 PASS 905 ms | ARM-only observed; #2878 |
| guskit/smb-share-gone | ARM :3690, ms=0, same service precondition; call `guiapps_suite/guskit.rs:364` -> same helper; R240 :5489 PASS 4290 ms | ARM-only observed; #2878 |
| demos/close-each | ARM :4029, ms=10000; UI_CONTROL :4017 AppExit started=1 completed=0, InputDelivery :4027 completed=24/24; `guiapps_suite/demos.rs:14-17`, `:36` -> `libs/libuitest/src/app.rs:157-161`, `:186` -> `session.rs:381-387`; R240 :5842 PASS 1168 ms | ARM-only observed; #2925 |
| desktop/crash-isolated | ARM :5566, ms=1226, status=11 expected=139; DATA_ABORT from_el0=1 at :5516; `guiapps_suite/desktop.rs:258` -> `ack_app_test.rs:33-47`; ARM `exception.rs:1035`, `:1249-1252` supplies -11 without core bit, `kernel/src/syscall/wait.rs:290-295` returns 11; R240 :7424 PASS 1212 ms | ARM-only producer defect; #2926 |

For the SMB failures, init reports shares startup failed at serial.txt:606.
The stopped image's `/run/breenix/services` reports starts=1, exit=code:1,
output=drained. `/var/log/shares.log` records mount failures for both configured
mount directories. `userspace/programs/src/mount.rs:44-46` aggregates these
failures; `:58` suppresses smbfsd stderr during batch mounting, and `:64-66` rejects a
daemon that exits before READY. `smbfsd.rs:26-27` also collapses connection and
authentication failures. The retained capture therefore identifies failed
mount startup but cannot establish the protocol/authentication phase. Do not
invent a narrower root cause or call the restart helper exercised. See #2878.

For demos/close-each, the first app (Bounce) remained present until the case
watchdog killed the session at serial.txt:3993-4001. Its first AppExit wait did
not complete; this is not six launches accumulating beyond a budget.
`bounce.rs:252` consumes CloseRequested and `:271` returns on it. The capture
does not distinguish close dispatch, app consumption or exit publication.
Keep the case and watchdog unchanged; investigate #2925 from this evidence.

For crash-isolated, `kernel/src/signal/delivery.rs:1758-1770` preserves the core
bit through fault_death_exit_code, but the ARM data-abort fallback at
`kernel/src/arch_impl/aarch64/exception.rs:1035` computes the bare negative
signal. The suite expectation stays 0x8b. No guest repair is included here.

## Per-case comparison

Times are milliseconds from each capture's CASE record. ARM capture is Vigil
`20261113T040422Z-qemu-87249`; comparison is R240 `20261112T190204Z-metal`.

| Case | ARM | ms | R240 | ms |
|---|---|---:|---|---:|
| terminal-emulation/vt-screen | PASS | 777 | PASS | 560 |
| terminal-emulation/scroll-region | PASS | 781 | PASS | 564 |
| terminal-emulation/alt-screen | PASS | 760 | PASS | 612 |
| terminal-emulation/utf8 | PASS | 766 | PASS | 588 |
| terminal-emulation/keys | PASS | 1427 | PASS | 1578 |
| bterm/prompt | PASS | 739 | PASS | 504 |
| bterm/command-output | PASS | 781 | PASS | 577 |
| bterm/exit-status | PASS | 801 | PASS | 610 |
| bterm/ctrl-c | PASS | 1086 | PASS | 712 |
| bterm/tabs | PASS | 969 | PASS | 961 |
| bterm/resize | PASS | 1093 | PASS | 1142 |
| bterm/scrollback | PASS | 1064 | PASS | 1169 |
| bterm/exec-tab | PASS | 752 | PASS | 467 |
| bterm/paste | PASS | 931 | PASS | 925 |
| blauncher/open | PASS | 809 | PASS | 626 |
| blauncher/filter | PASS | 784 | PASS | 587 |
| blauncher/no-results | PASS | 765 | PASS | 537 |
| blauncher/keys-select | PASS | 968 | PASS | 790 |
| blauncher/launch-gui | PASS | 812 | PASS | 636 |
| blauncher/launch-terminal | PASS | 989 | PASS | 647 |
| blauncher/launch-missing | PASS | 930 | PASS | 843 |
| blauncher/close | PASS | 1294 | PASS | 1050 |
| blog/tail | PASS | 2291 | PASS | 2000 |
| blog/follow | PASS | 1827 | PASS | 1514 |
| blog/truncate-rotate | PASS | 2823 | PASS | 2512 |
| blog/navigation | PASS | 1517 | PASS | 1510 |
| blog/search | PASS | 1517 | PASS | 1523 |
| blog/missing-file | PASS | 1293 | PASS | 974 |
| blog/blogd-records-once | PASS | 316 | PASS | 385 |
| blog/blogd-restart | PASS | 81 | PASS | 192 |
| bfontpicker/lists-fonts | PASS | 1074 | PASS | 905 |
| bfontpicker/select-apply | PASS | 1788 | PASS | 1592 |
| bfontpicker/persists | PASS | 2183 | PASS | 1887 |
| bfontpicker/keeps-unknown | PASS | 1789 | PASS | 1551 |
| bfontpicker/live-bterm | PASS | 2190 | PASS | 1841 |
| bfontpicker/save-fails-cleanly | PASS | 1298 | PASS | 1222 |
| bcheck/results-truthful | PASS | 937 | PASS | 787 |
| bcheck/hang | PASS | 1897 | PASS | 1617 |
| bcheck/forged-output | PASS | 818 | PASS | 611 |
| bcheck/totals-exit | PASS | 854 | PASS | 622 |
| bcheck/responsive | PASS | 2281 | PASS | 2989 |
| bcheck/production-catalog | PASS | 1928 | PASS | 2369 |
| guskit/pencil | PASS | 986 | PASS | 824 |
| guskit/shapes | PASS | 1402 | PASS | 1468 |
| guskit/fill | PASS | 1198 | PASS | 1201 |
| guskit/eraser-color | PASS | 1377 | PASS | 1576 |
| guskit/undo-redo | PASS | 1341 | PASS | 1444 |
| guskit/save-open | PASS | 2893 | PASS | 2626 |
| guskit/save-fails-cleanly | PASS | 1012 | PASS | 914 |
| guskit/smb-save-open | FAIL | 0 | PASS | 2851 |
| guskit/smb-save-fails-cleanly | FAIL | 0 | PASS | 905 |
| guskit/smb-share-gone | FAIL | 0 | PASS | 4290 |
| guskit/open-malformed | PASS | 1070 | PASS | 986 |
| guskit/quit-unsaved | PASS | 1111 | PASS | 997 |
| guskit/shortcuts | PASS | 1639 | PASS | 1928 |
| demos/open-each | PASS | 2181 | PASS | 1612 |
| demos/close-each | FAIL | 10000 | PASS | 1168 |
| demos/resize | PASS | 2041 | PASS | 2273 |
| demos/bounce-input | PASS | 1423 | PASS | 2640 |
| demos/bounded | PASS | 896 | PASS | 641 |
| demos/biconkit-interact | PASS | 867 | PASS | 856 |
| guskit-collab/join-sync | PASS | 1546 | PASS | 1708 |
| guskit-collab/stroke-both-ways | PASS | 2139 | PASS | 2760 |
| guskit-collab/global-order | PASS | 2704 | PASS | 3572 |
| guskit-collab/three-peers | PASS | 2851 | PASS | 3821 |
| guskit-collab/undo-ownership | PASS | 4919 | PASS | 6630 |
| guskit-collab/peer-leaves | PASS | 3461 | PASS | 4767 |
| desktop/init-desktop | PASS | 824 | PASS | 578 |
| desktop/launch-each | PASS | 11947 | PASS | 13816 |
| desktop/copy-paste | PASS | 1419 | PASS | 1291 |
| desktop/font-everywhere | PASS | 4168 | PASS | 4148 |
| desktop/crash-isolated | FAIL | 1226 | PASS | 1212 |
| desktop/close-all-baseline | PASS | 2883 | PASS | 2815 |

## Checks and handoff

- Existing 73-case manifest was the baseline check, committed prediction in
  `d46f079c`; its expectations were unchanged (U29).
- Import check `65f27dfd`: both entry points failed first, then 1 test / 2 entry
  points passed after `27a07a27`. Shell syntax exited 0.
- Lease check `5c6b30f2`: absent/active/malformed fixtures failed first; 4 SMB
  helper tests passed after `be737c42`, including existing refusal and cleanup.
  The live read-only preflight exited 0.
- ARM userspace and production kernel build exited 0 during preflight. Source
  warnings: none observed; the pinned Rust core future-incompat notice remains.
  SIMD guard passed. No build was repeated after host-only changes. Evidence:
  `/tmp/a5-u5-resumed-a8bb2637/launch.log`.
- Claim lint for the final report exited 1 twice on prose claims;
  adding an evidence reference and correcting the prose made it exit 0. Its commit-message lint
  exited 0. Earlier
  invocations and the corrected prediction-message findings are recorded above.

Remaining work: #2878 (three SMB cases), #2925 (demo exit), #2926 (ARM fault
status), #2927 (watcher shutdown classification). #2924's import defect is
repaired on this branch by `27a07a27`, with its check committed first.
A.5 remains red; this unit has recorded its baseline. No second guest boot,
R240 boot, PR or merge occurred.
