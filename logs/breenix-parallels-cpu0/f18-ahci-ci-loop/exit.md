# F18 Exit Report: AHCI CI-Level Completion Loop

## VERDICT

PASS.

Branch: `probe/f18-ahci-ci-loop`

Commits:

- `1dc65313 fix(ahci): drain completions by CI level`
- `03357aa2 test(ahci): record F18 Parallels sweep`
- pending doc commit: investigation doc and exit report

## What I built

- `kernel/src/drivers/ahci/mod.rs`: changed the AHCI ISR from single-shot `PORT_IS` completion handling to a bounded CI-level drain loop. The handler now computes completions from `PORT_ACTIVE_MASK & !PORT_CI`, atomically clears completed active bits, acknowledges sampled `PORT_IS`, loops until the port is stable, and defers slot-0 wake publication until after the port interrupt is no longer asserted.
- `logs/breenix-parallels-cpu0/f18-ahci-ci-loop/summary.txt`: recorded the final 5-run Parallels sweep with prior F-series fields plus `ahci_ci_loop_iterations`.
- `docs/planning/ARM64_CPU0_SMP_INVESTIGATION.md`: appended the F18 audit, Linux citation, fix description, validation table, and completion verdict.
- `logs/breenix-parallels-cpu0/f18-ahci-ci-loop/exit.md`: this report.

## What the original ask was

Fix the F17 AHCI missed-completion failure by auditing the Breenix handler
against Linux AHCI behavior, implementing a CI-diff completion loop in
`handle_interrupt()`, validating with five Parallels runs, and documenting the
result with a clear verdict.

## How what I built meets that ask

- Audit: implemented. Commit `1dc65313` compares the old Breenix edge-sensitive `PORT_IS` flow to Linux v6.8's CI-active completion model in `drivers/ata/libahci.c`.
- Fix: implemented. `kernel/src/drivers/ahci/mod.rs` now uses `AHCI_CI_COMPLETION_LOOP_LIMIT=8`, `AHCI_TRACKED_SLOT_MASK`, `AHCI_TRACE_CI_LOOP`, and a bounded active-mask/`PORT_CI` drain loop.
- Linux cite: implemented. The investigation doc cites `/tmp/linux-v6.8/drivers/ata/libahci.c` lines 1875-1888 and 1963-1966.
- Build clean: implemented. `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64` completed with no warning/error lines.
- Five-run sweep: implemented. Final logs are in `logs/breenix-parallels-cpu0/f18-ahci-ci-loop/run{1..5}/`.
- Pass criteria: implemented. All five final serial logs reached `[init] bsshd started (PID 2)` with `Port 1 TIMEOUT` count 0 and corruption marker count 0.
- Investigation doc: implemented. The appended 2026-04-16 F18 section declares the investigation complete and recommends a cleanup PR.

## What I did NOT build

- I did not remove F8-F17 diagnostic scaffolding. That was explicitly a non-goal for F18 and should be a follow-up cleanup PR.
- I did not change GIC code or any prohibited files.
- I did not add a permanent regression test beyond the required Parallels sweep summary.

## Known risks and gaps

- `./run.sh --parallels --test 60` still exits 1 because the screenshot helper cannot find the generated Parallels VM window. This matches earlier F-series sweeps; serial logs were used as the validation source.
- Passing runs do not dump the AHCI ring, so `ahci_ci_loop_iterations` is visible as 0 in final serial logs. The `CI_LOOP` event exists for timeout-time AHCI ring dumps and was observed during failed intermediate sweeps.
- The current driver still only issues slot 0; the code preserves arrays for future multi-slot work, but only slot 0 has a completion token today.

## How to verify

```bash
cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64 2>&1 | grep -E "^(warning|error)"
```

Expected: no output.

```bash
for i in 1 2 3 4 5; do
  f="logs/breenix-parallels-cpu0/f18-ahci-ci-loop/run$i/serial.log"
  printf "run$i bsshd="
  rg -c "\\[init\\] bsshd started \\(PID 2\\)" "$f" || true
  printf "run$i ahci_timeouts="
  rg -c "Port 1 TIMEOUT" "$f" || true
  printf "run$i corruption_markers="
  rg -c "CORRUPTION|corruption|PC_ALIGN|panic|BAD" "$f" || true
done
```

Expected: every run reports `bsshd=1`, `ahci_timeouts=0`, and
`corruption_markers=0`.

## Recommendation

Open the follow-up cleanup PR to remove the accumulated F8-F17 diagnostic
scaffolding now that F18 has produced a 5/5 pass and closed this AHCI timeout
signature.
