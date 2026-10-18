# x86-64 execution policy

<!-- claim-lint:ok: docs/x86-execution-policy.md records S23 (operator, 2026-10-18); this paragraph prescribes policy, not a measured result. -->
As of 2026-10-18 (operator ruling S23), x86-64 is proved on the R240 through the
[metal boot loop](metal/README.md), using `scripts/metal/metal-run`. The main path
measures ARM64 QEMU, Parallels and VMware; an effort can retain a narrower target list.
x86 hardware-profile milestones remain defined but are measured nowhere on the main path.

This policy supersedes earlier landing bars, gate checklists and plans that require
x86 QEMU evidence. Build x86 code when a change touches it; `cargo build`, `cargo check`
and the metal build retain their existing behavior. Choose boot evidence according to
the diff, and run a check once per code state.

The x86 QEMU launchers refuse before starting a boot. Only an operator setting
`BREENIX_ALLOW_X86_QEMU=1` may opt in for an exceptional invocation; the override is
forwarded to a remote QEMU launch. Such a run does not measure a main-path target.
The legacy gate's `BREENIX_GATE_PHASE=build` remains available without a boot override.

<!-- claim-lint:ok: docs/x86-execution-policy.md records S23 (operator, 2026-10-18); this paragraph prescribes policy, not a measured result. -->
Stop only processes started by the current run, by the recorded PID. Never use
`pkill`, `killall`, or a kill by process name or pattern.
