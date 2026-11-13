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
