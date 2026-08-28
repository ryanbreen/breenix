# #672 evidence — x86 preempt-bracket asymmetry and underflow guard

Every file here is a raw capture, kept because the campaign's rule is that a
serial which produced a verdict is preserved rather than described.

| file | what it is |
|---|---|
| `main-baseline-bc383295-gate.log` | `run-x86-prod-profile-boot-test.sh` on `main` @ `bc383295`, on beast. PASS. Its liveness line reads `serial bytes over 15s: 45714 -> 45732` — those 18 bytes are three `pFFr1 ` tokens, `per_cpu::can_schedule()`'s every-1000th-refusal trace printing the low byte of the preempt_count #672 wrapped to `0xFFFFFFFF` beside a set `need_resched`. The shipped kernel's only spontaneous output was the defect. |
| `branch-old-liveness-red-serial_*.txt` | the fix with the gate's *old* free-running liveness check: every #672 pin flips correctly (`PRECONDITION 7 ... ✓ PASS`, `[PREEMPT_BRACKET_CENSUS:underflow=0]`) and the gate still reds, on `test "$BYTES_AFTER" -gt "$BYTES_BEFORE"`, because a healthy production kernel emits nothing on its own. This red is why the liveness check became stimulus-response. |
| `branch-gate-pass-serial_*.txt`, `branch-dc987245-gate-pass.log` | the fixed kernel under the re-derived gate. PASS. The user serial ends `breenix> x` — the console's echo of the one byte the gate wrote, which is the liveness proof. |
| `mutation-unpaired-enable-*` | mutation (b): one extra unpaired `kernel::per_cpu::preempt_enable()` planted after the real one, zero-feature build. The guard fired, the count did **not** wrap (`PRECONDITION 7` still passes, boot still healthy), the kernel printed `[PREEMPT_BRACKET_CENSUS:underflow=1]`, and the gate went red on the census pin. |
| `branch-x86-boot-tests-pass.log` | `run-x86-boot-tests.sh` on the branch. `x86 userspace gate: PASS - exited=104 expected>=104 nonzero=0 allowlist=0`, `[BOOT_TESTS:PASS]`, `x86 frame-custody gate run 1: PASS`, zero build warnings. |

Mutation (a) — re-introducing the `#[cfg]` on the disable — is a host-suite
mutation with no serial: it reddens `tests/preempt_bracket_structure.rs`
(`boot_path_preempt_brackets_are_cfg_symmetric` and
`boot_path_preempt_sites_share_one_cfg_context`, the other five tests staying
green).
