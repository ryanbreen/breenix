# T3-G PR3 round-3 gate logs

Captured on the shipped round-3 tree of `fix/producer-corruption-family`. Ruling R47 required the two
host builds to have captured logs; the gate summaries are here so the acceptance table in PR #642 can
be checked against something durable rather than against a scratch directory.

| file | what it is |
|---|---|
| `01-aarch64-prod-build.txt` | aarch64 production build, `aarch64-breenix-kernel.json` (the soft-float kernel target — never the NEON one, per the #528 re-arm rule), after a forced `cargo clean -p kernel --release`. Exit 0. The only `warning:` lines are the pre-existing toolchain `core` future-incompatibility note, which is not project code. |
| `03-x86-build-beast.txt` | x86 `qemu-uefi` build on beast (`breenix-x86` Incus container, `testing,external_test_bins`), after a forced `cargo clean -p kernel --release` **and** `cargo clean -p breenix --release`, so both crates recompiled from scratch. Exit 0, **zero** `warning`/`error` lines. This is R47 item 3: the round-2 x86 build claim had no captured log. |
| `04-ss25-summary.txt` | service-sequence gate, 25 boots × 2 profiles (`max`, `cortex-a72`). Both profiles PASSED, 50/50 GREEN, every named bucket 0, `Resume PC refused: 0/50`, `RET dispatch refused: 0/50`. Per-boot lines elided; the gate's own preserved serials are under `/tmp/breenix_aarch64_service_sequence_gate_*`. |
| `05-strict20-summary.txt` | strict boot gate, 1×20. 20/20, 100%, 203 s. Includes the gate's `kernel_no_neon_guard` preflight: 0 FP/SIMD load/store instructions in kernel `.text`. |
| `06-x86-boot-tests-beast.txt` | x86 custody boot tests on beast, 4 boots (one run of 1, then a run of 3). Every boot `x86 userspace gate: PASS - exited=102 expected>=101 nonzero=0 allowlist=0` and `x86 frame-custody gate run N: PASS`. No reds this round, so nothing to attribute. |

Round-1's single x86 red (`[TEST:userspace:loopback_recv_wake:FAIL:reader_exit_15]`,
`eof_wait_ms=8706`) is attributed to **#636** by field signature — not to "a transient timing flake",
which is the catch-all the campaign law forbids and which the round-1 write-up used. The round-1 x86
tally was 11 boots, 10 PASS, 1 FAIL (#636).
