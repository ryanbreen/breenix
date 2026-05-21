# Turn 53 Validation: P12 AHCI Polling Sites Survey

## Scope

- Survey-only turn.
- Added `turn53-artifacts/p12-ahci-classification.md`.
- No kernel, docs, library, or test source files were modified.

## Method

- Grepped `kernel/src/drivers/ahci/mod.rs` for `spin_loop`, `loop`, `while`, `PORT_CI`, and `wait_ready`.
- Inspected each candidate polling/wait block with line-numbered context.
- Checked upstream Linux references from current `torvalds/linux` source:
  - `drivers/ata/libahci.c::ahci_stop_engine()`
  - `drivers/ata/libahci.c::ahci_start_engine()`
  - `drivers/ata/libahci.c::ahci_handle_port_interrupt()`
  - `drivers/ata/libata-core.c::ata_wait_after_reset()`
  - `drivers/ata/libata-core.c::ata_wait_ready()`

## Result

- Total AHCI polling/wait sites surveyed: 7.
- ALLOWLIST candidates: 5.
- INFRASTRUCTURE candidates: 2.
- Recommended T54: AHCI ALLOWLIST batch for bounded hardware handshakes (engine state waits and taskfile readiness waits).

## Source Diff Sanity

- `turn53-artifacts/source-diff-stat.txt` is intentionally empty for project source paths.
- `turn53-artifacts/source-diff.txt` is intentionally empty for project source paths.
- Pre-existing unrelated turn5/turn7 worktree dirt remains untouched.
