# Breenix Runs Fixtures

| Fixture | Source | Source commit | Copy note |
|---|---|---|---|
| `Tests/Fixtures/05-runtime-anti-vacuity-strict-serial.txt` | `docs/planning/green-program/aarch64-testing/serials/slice3d/05-runtime-anti-vacuity-strict-serial.txt` | `6c4dc9c39fa8367c81c0ae5eb3b44eb39e219bb5` | Copied byte-for-byte with `cp`; line endings were not normalized. |
| `Tests/Fixtures/testing-boot1-562-panic.txt` | `docs/planning/green-program/aarch64-testing/serials/slice3b/testing-boot1-562-panic.txt` | `76c9174697bedeca7ea0e5b6e4382b84e1824b41` | Copied byte-for-byte with `cp`; line endings were not normalized. |
| `Tests/Fixtures/fatal-regs-labelled-excerpt.txt` | `docs/planning/teardown-unification/607-576-serials/gate-clean100-cortexa72-boot3-stackpc-8600000e.txt` | `71cceff0714df694cd5d2ae47ee7e56e631777b9` | Excerpt copied byte-for-byte with `sed -n '675,710p'`, not the whole file; line endings were not normalized. |
| `Tests/Fixtures/fatal-regs-unlabelled-excerpt.txt` | `docs/planning/teardown-unification/607-576-serials/round2-gates/ss25-r175277c7-cortexa72-boot5-external-abort-endofram-96000010.txt` | `a851aadc66c7f0594336af326017c455d0b3efb0` | Excerpt copied byte-for-byte with `sed -n '683,720p'`, not the whole file; line endings were not normalized. |
| `Tests/Fixtures/gate-boot-facts-positive.txt` | `docs/planning/green-program/gates/GATE-BOOT-FACTS-827-2026-09-05.md` | `0fb5feaf04aa453225ec44aa5a2736a8c3fef206` | Two quoted `GATE_BOOT_FACTS` lines from lines 247-248, with a one-line source comment in the fixture. |
| `Tests/Fixtures/boot2-hard-timeout-serial-no-gate-boot-facts.txt` | `docs/planning/green-program/gates/serials/827-landing-2026-09-05/boot2-hard_timeout-serial.txt` | `0fb5feaf04aa453225ec44aa5a2736a8c3fef206` | Copied byte-for-byte with `cp`; line endings were not normalized. |
| `Tests/Fixtures/bxcap-v1-synthesized.txt` | `/private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/ftc-design/FAILURE-TRACE-CAPTURE-PLAN-2026-09-05.md` section 4 | n/a | Synthesized, not real evidence; constructed by hand from the `BXCAP v1` schema. |
