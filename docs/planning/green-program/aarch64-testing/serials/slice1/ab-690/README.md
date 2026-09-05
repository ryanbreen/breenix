# A/B discriminator for #690 (ruling R160, slice 1)

<!-- claim-lint:ok: #690 -- signature definition quoted from the R160 task -->
Question: does `fix/ttbr0-shadow-reconciliation` raise the rate of the #690
signature (clonevm_exec_test hangs at "second stage", init never leaves
`waitpid`, bsshd never spawns, gate ends `FAIL: bsshd never reached its
listening state`) on the aarch64 production profile relative to
`origin/main`?

- Branch head: `b731e53a7da2526c4f93d7efbecdb6b106b6df69`
  (`fix/ttbr0-shadow-reconciliation`)
- Main head: `d6b7a186e37b67ee53f9c233442cdc54565874df` (`origin/main`)
- Gate: `docker/qemu/run-aarch64-prod-profile-boot-test.sh` (unmodified,
  no-features aarch64-breenix-kernel.json soft-float production profile)
<!-- claim-lint:ok: 20/20 boots preserved -- branch/boot-{1..10}-{gate,serial}.txt, main/boot-{1..10}-{gate,serial}.txt in this directory -->
- Protocol: 20 boots total, alternating branch/main one boot at a time;
  `pgrep -fl qemu-system-aarch64 | wc -l` recorded immediately before every
  launch, gated to <=2 (host-load rule); every boot's gate transcript +
  full guest serial preserved alongside this file.

## Result

Branch: **10/10 PASS**, 0 reds, 0 occurrences of the #690 signature.
Main: **9/10 PASS**, 1 red (unattributed — not #690, not any of the five
listed known signatures).

Neither arm produced a single #690 occurrence in this 10-boot-per-arm
sample. This does not reproduce the "1 in 3" rate the R160 prompt reports
having observed for this slice/branch on the cortex-a72 service-sequence
profile — see Discussion below.

## Per-boot table

| boot | arm | pgrep-at-launch | result | signature |
|-----:|-----|:---:|:---|:---|
| 1 | branch | 0 | PASS | — |
| 1 | main | 0 | PASS | — |
| 2 | branch | 0 | PASS | — |
| 2 | main | 0 | PASS | — |
| 3 | branch | 0 | PASS | — |
| 3 | main | 0 | **FAIL** | UNATTRIBUTED — `TTY_ORACLE:FAIL:cloexec_exec:fcntl_setfd_failed:EAGAIN` (bsshd had already reached listening state in this boot's serial; the gate still fails because the TTY-oracle failure check runs first) |
| 4 | branch | 0 | PASS | — |
| 4 | main | 0 | PASS | — |
| 5 | branch | 0 | PASS | — |
| 5 | main | 0 | PASS | — |
| 6 | branch | 0 | PASS | — |
| 6 | main | 0 | PASS | — |
| 7 | branch | 0 | PASS | — |
| 7 | main | 0 | PASS | — |
| 8 | branch | 0 | PASS | — |
| 8 | main | 0 | PASS | — |
| 9 | branch | 0 | PASS | — |
| 9 | main | 0 | PASS | — |
| 10 | branch | 0 | PASS | — |
| 10 | main | 0 | PASS | — |

Files: `branch/boot-N-{gate,serial}.txt` and `main/boot-N-{gate,serial}.txt`
for N in 1..10.

## #690-adjacent-but-distinct behavior observed on branch

<!-- claim-lint:ok: #690 -- branch/boot-1-serial.txt, lines quoted verbatim below -->
Branch boot 1's serial shows `clonevm_exec_test` printing
`CLONEVM_EXEC_TEST: second stage` followed by
`CLONEVM_EXEC_TEST: ERROR sibling wake of parent failed` and
`CLONEVM_EXEC_TEST: ERROR parent wait was not woken by sibling`, then the
test process **exits(1) on its own** and init logs
`[init] clonevm_exec_test exited pid=12 code=1` and continues normally to
bsshd. This is not the #690 signature: #690 is defined by the test *hanging*
forever (init stuck in `waitpid`, bsshd never spawned). Here the test
self-detects the same underlying race, reports it, and exits cleanly, so the
boot is scored PASS. Quoted for context; not counted as a #690 occurrence
under the R160 definition.

## Fisher's exact test (information only, not the landability rule)

2x2 table (#690 count vs not, branch vs main): `[[0,10],[0,10]]`.
One-sided (branch > main), computed with `scipy.stats.fisher_exact`:

```
p = 1.0
```

<!-- claim-lint:ok: 0/10 branch, 0/10 main -- see per-boot table above -->
With zero #690 occurrences in both arms this is the only value the test can
return; it carries no evidence either way.

## Mechanical decision (per R160)

`landable = (branch has no red outside #690) AND (branch #690 count <= main #690 count + 1)`

- Branch has 0 reds total (10/10 PASS) => "no red outside #690" is
  vacuously true.
- Branch #690 count (0) <= main #690 count (0) + 1.

=> **landable = true** by the stated mechanical rule.

## Discussion (honesty note, not part of the mechanical rule)

The R160 prompt states main sees #690 at ~1-in-30 and this slice previously
saw it at 1-in-3 on the cortex-a72 service-sequence profile. This run used
the aarch64 **production profile** gate
(`run-aarch64-prod-profile-boot-test.sh`), not the service-sequence gate, at
n=10 per arm. At a true 1-in-3 rate, P(zero occurrences in 10 independent
boots) = (2/3)^10 ~= 1.7%, i.e. unlikely but not impossible by chance alone;
it is also consistent with the 1-in-3 figure being specific to the
service-sequence profile/environment rather than the production profile
tested here, or with sample noise in either the original 1-in-3 observation
or this one. This sample cannot rule out an elevated rate on branch; it only
fails to reproduce one on this profile at this n. The one red observed
(main boot 3) is an unrelated TTY-oracle cloexec/EAGAIN failure, not #690,
and does not bear on the #690 question in either direction.
