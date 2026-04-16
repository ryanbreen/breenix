# Scratchpad - F12 SGI Linux Parity

## 2026-04-16T06:05:00-04:00

Starting M1: reference and patch.

- User contract requires branch `probe/f12-sgi-linux-parity` from `diagnostic/f11-send-sgi-boundary`.
- Current main checkout is dirty on `diagnostic/f6-gic-stuck-state`; created a clean worktree at `/Users/wrb/fun/code/breenix-worktrees/f12-sgi-linux-parity`.
- Linux source exists at `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v3.c`.
- Linux v6.8 reference:
  - `drivers/irqchip/irq-gic-v3.c:1350-1363`: `gic_send_sgi()` composes the `ICC_SGI1R_EL1` value and calls `gic_write_sgi1r(val)`.
  - `drivers/irqchip/irq-gic-v3.c:1365-1388`: `gic_ipi_send_mask()` validates SGI ID, runs `dsb(ishst)`, loops target CPUs and calls `gic_send_sgi()`, then runs `isb()`.
- Breenix reference:
  - `kernel/src/arch_impl/aarch64/gic.rs:1023-1079`: `send_sgi()` checks SGI ID, composes target-list SGIR, writes `icc_sgi1r_el1`, then runs `isb()`.
  - Divergence: Breenix lacks Linux's pre-write `dsb ishst`.
  - No Linux DAIF/preemption masking or WFE/SEV was observed in the cited SGI emission path.
- Beads issue `breenix-5ne` created and claimed after restarting the local Dolt server. Auto-push is currently rejected by the remote Dolt branch being non-fast-forward; will retry with `bd dolt pull/push` during closeout.

## 2026-04-16T06:12:00-04:00

Patched M1:

- `kernel/src/arch_impl/aarch64/gic.rs`: added `dsb ishst` in the GICv3 branch of `send_sgi()` before SGIR composition and the `msr icc_sgi1r_el1`.
- `kernel/src/arch_impl/aarch64/gic.rs`: after `msr icc_sre_el1` and its `isb`, read back `icc_sre_el1` and emit `[SRE_AUDIT] cpu=<id> sre=<sre_bit> raw=<full_value>` with existing raw UART helpers.

Running M1 validation build next.

## 2026-04-16T06:15:00-04:00

M1 validation passed:

- `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64` completed cleanly.
- `grep -E '^(warning|error)' /tmp/f12-aarch64-build.log` produced no output.
- `git diff --check` passed.

M2 commit created:

- Commit `128ed10f` `probe(arm64): match Linux gic_send_sgi() byte ordering (dsb ishst / SRE audit)`.
- Commit body cites `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v3.c:1350-1387` and `kernel/src/arch_impl/aarch64/gic.rs:1023-1079`.
- Amended out an unrelated Beads export that the commit hook initially staged into `issues.jsonl`; final commit changes only `kernel/src/arch_impl/aarch64/gic.rs`.

Starting M3: required five-run Parallels sweep. Must clean QEMU before each run.

## 2026-04-16T06:29:00-04:00

Initial sweep exposed a code-side audit gap: every run only emitted
`[SRE_AUDIT] cpu=0 sre=1 raw=0x7`, even when secondary CPU raw markers appeared.
Patched `init_cpu_interface_secondary()` to bring up/audit the ICC
system-register interface before the secondary GICR range guard. Rebuilt cleanly:

- `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
- `grep -E '^(warning|error)' /tmp/f12-aarch64-build-2.log` produced no output.
- `git diff --check` passed.

Amended commit 1 to `c3e34410`. Discarding the initial sweep artifacts and
starting a fresh 5x sweep against the amended code.

## 2026-04-16T06:58:00-04:00

Final 5x sweep completed under:

```text
logs/breenix-parallels-cpu0/f12-sgi-parity/run{1..5}/
```

Summary:

- run1: PASS criteria satisfied; `bsshd_started=1`, `ahci_timeouts=0`, `corruption_markers=0`, `sre_audit_lines=2`.
- run2: PASS criteria satisfied; `bsshd_started=1`, `ahci_timeouts=0`, `corruption_markers=0`, `sre_audit_lines=4`; one garbled SRE line caused `sre_unexpected=1`.
- run3: FAIL; `bsshd_started=1`, `ahci_timeouts=2`, `corruption_markers=0`, SPI34 pending+active stuck-state on cpu=3/cpu=6.
- run4: FAIL; `bsshd_started=1`, `ahci_timeouts=2`, `corruption_markers=0`, SPI34 pending+active stuck-state on cpu=4/cpu=5 and soft lockup.
- run5: PASS criteria satisfied; `bsshd_started=1`, `ahci_timeouts=0`, `corruption_markers=0`, `sre_audit_lines=1`.

Verdict: FAIL. Linux SGI ordering parity (`dsb ishst` before `msr icc_sgi1r_el1`, `isb` after) is not sufficient. F13 should pivot to non-garbled per-CPU ICC/GICR redistributor audit rather than another SGI-side barrier probe.
