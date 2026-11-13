# A.5 u5: ARM gui-apps baseline prediction

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

## Result: blocked before boot

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
