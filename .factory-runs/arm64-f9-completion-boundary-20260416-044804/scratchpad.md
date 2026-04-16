# F9 Completion Boundary Scratchpad

## 2026-04-16 04:48

Starting setup.

- Isolated worktree created at `/Users/wrb/fun/code/breenix-worktrees/f9-completion-boundary`.
- Branch: `diagnostic/f9-completion-boundary`, based on `diagnostic/f8-ahci-completion`.
- Beads tracking attempted before code changes, but `bd create` failed because the Dolt runtime server does not expose database `breenix` on `127.0.0.1:63842`. `bd bootstrap` synced from remote but did not repair the runtime-server mismatch. Continuing with this scratchpad per factory runbook.

Starting milestone 1: inspect F8 instrumentation, completion wake path, and stuck-state dump before editing.

Inspection result:

- F8 ring lives in `kernel/src/drivers/ahci/mod.rs`; `push_ahci_event()` is private and site names currently cover `ENTER`, `POST_CLEAR`, `RETURN`.
- AHCI handler calls `AHCI_COMPLETIONS[port][0].complete(cmd_num)` after `POST_CLEAR` when `cmd_num != 0`.
- `Completion::complete()` loads `waiter` and calls `crate::task::scheduler::isr_unblock_for_io(tid)` when nonzero.
- SPI34 stuck dump currently emits only `AHCI_PORT0_IS` via `port0_is_snapshot()`.

Starting milestone 2: add F9 ring sites and port 1 stuck-state IS visibility without changing completion or scheduler semantics.

Edit result:

- `kernel/src/drivers/ahci/mod.rs`: added `BEFORE_COMPLETE`, `AFTER_COMPLETE`, `WAKE_ENTER`, and `WAKE_EXIT` site tags; made `push_ahci_event()` crate-visible; inserted before/after ring pushes around the existing `Completion::complete(cmd_num)` call; added `port_is_snapshot(port)`.
- `kernel/src/task/completion.rs`: wrapped the existing `isr_unblock_for_io(tid)` call with AHCI ring pushes when `tid != 0`.
- `kernel/src/arch_impl/aarch64/gic.rs`: kept `AHCI_PORT0_IS` and added `AHCI_PORT1_IS`.

Starting milestone 3: run build/diff hygiene validation before committing diagnostic code.

Validation result:

- `git diff --check`: passed.
- `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`: passed.
- `grep -E '^(warning|error)' /tmp/f9-aarch64-build.log`: no output.
- Commit 1 created: `36041857 diagnostic(arm64): AHCI Completion::complete boundary ring sites`.

Starting milestone 4: run 5x `./run.sh --parallels --test 60` sweep and write summaries.

Sweep result:

- `run1`: `exit_status=1`, soft-lockup branch before `bsshd`, no AHCI timeout/ring.
- `run2`: `exit_status=1`, `ahci_timeouts=1`, `ahci_ring_entries=32`, `ahci_port0_is=1`, `ahci_port1_is=1`, `bsshd_started=1`, `before_complete=7`, `after_complete=5`, `wake_enter=3`, `wake_exit=2`.
- `run3`: `exit_status=1`, `bsshd_started=1`, no AHCI timeout/ring.
- `run4`: `exit_status=1`, `bsshd_started=1`, no AHCI timeout/ring.
- `run5`: `exit_status=1`, `bsshd_started=1`, no AHCI timeout/ring.
- `exit_status=1` is from the headless Parallels screenshot helper; compile output stayed warning-free.

Case verdict from run2:

- Stuck token `1219` has `ENTER`, `POST_CLEAR`, `BEFORE_COMPLETE`, `WAKE_ENTER`, but no `WAKE_EXIT`, `AFTER_COMPLETE`, or handler `RETURN`.
- Verdict: Case A by top-level BEFORE/AFTER rule, refined to the scheduler wake helper inside `Completion::complete()`.

Starting milestone 5: append investigation document and write mandatory exit.md.
