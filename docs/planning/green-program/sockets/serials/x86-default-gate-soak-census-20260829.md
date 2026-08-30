# x86 default-gate soak, 30 boots at the round-3 merge-candidate bytes

docker/qemu/run-x86-boot-tests.sh, beast breenix-x86 (Incus VM), TCG.
Gate result: FAIL 30/30. Every failure is the tombstone-census reconciliation,
and the cause is poll_tcp_oracle's per-attempt fork(), not the kernel:

    CENSUS_REMOVED = 6 + attempts     (run-x86-boot-tests.sh:472 requires removed == 6)

The fit is exact on 30 of 30 boots. attempts is read from the verdict line
directly (attempts=N) or derived from its ladder (delay_ms = 80 * 2^(attempts-1)).

| boot | removed | attempts | 6+attempts | oracle verdict |
|---|---|---|---|---|
| 1 | 8 | 2 | 8 | `FAIL:late_lost_wake:ready=0 revents=0x0000 elapsed_ms=5302 delay_ms=160` |
| 2 | 8 | 2 | 8 | `FAIL:late_lost_wake:ready=0 revents=0x0000 elapsed_ms=5311 delay_ms=160` |
| 3 | 8 | 2 | 8 | `FAIL:late_lost_wake:ready=0 revents=0x0000 elapsed_ms=5229 delay_ms=160` |
| 4 | 11 | 5 | 11 | `FAIL:late_window_missed:attempts=5 overshoot_ms=34 delay_ms=1280` |
| 5 | 8 | 2 | 8 | `FAIL:late_lost_wake:ready=0 revents=0x0000 elapsed_ms=5020 delay_ms=160` |
| 6 | 11 | 5 | 11 | `FAIL:late_window_missed:attempts=5 overshoot_ms=32 delay_ms=1280` |
| 7 | 10 | 4 | 10 | `FAIL:late_lost_wake:ready=0 revents=0x0000 elapsed_ms=7652 delay_ms=640` |
| 8 | 10 | 4 | 10 | `FAIL:late_lost_wake:ready=0 revents=0x0000 elapsed_ms=5232 delay_ms=640` |
| 9 | 11 | 5 | 11 | `FAIL:late_lost_wake:ready=0 revents=0x0000 elapsed_ms=5307 delay_ms=1280` |
| 10 | 8 | 2 | 8 | `FAIL:late_lost_wake:ready=0 revents=0x0000 elapsed_ms=5190 delay_ms=160` |
| 11 | 8 | 2 | 8 | `FAIL:late_lost_wake:ready=0 revents=0x0000 elapsed_ms=5223 delay_ms=160` |
| 12 | 8 | 2 | 8 | `FAIL:late_lost_wake:ready=0 revents=0x0000 elapsed_ms=5146 delay_ms=160` |
| 13 | 7 | 1 | 7 | `PASS:stages=3:idle_ms=1206:late_ms=2359:park_ms=1884:attempts=1` |
| 14 | 11 | 5 | 11 | `FAIL:late_window_missed:attempts=5 overshoot_ms=30 delay_ms=1280` |
| 15 | 7 | 1 | 7 | `PASS:stages=3:idle_ms=601:late_ms=2676:park_ms=2616:attempts=1` |
| 16 | 11 | 5 | 11 | `FAIL:late_window_missed:attempts=5 overshoot_ms=29 delay_ms=1280` |
| 17 | 11 | 5 | 11 | `FAIL:late_window_missed:attempts=5 overshoot_ms=33 delay_ms=1280` |
| 18 | 8 | 2 | 8 | `FAIL:late_lost_wake:ready=0 revents=0x0000 elapsed_ms=5106 delay_ms=160` |
| 19 | 8 | 2 | 8 | `FAIL:late_lost_wake:ready=0 revents=0x0000 elapsed_ms=5181 delay_ms=160` |
| 20 | 8 | 2 | 8 | `FAIL:late_lost_wake:ready=0 revents=0x0000 elapsed_ms=5243 delay_ms=160` |
| 21 | 8 | 2 | 8 | `FAIL:late_lost_wake:ready=0 revents=0x0000 elapsed_ms=5346 delay_ms=160` |
| 22 | 7 | 1 | 7 | `PASS:stages=3:idle_ms=1371:late_ms=3216:park_ms=2814:attempts=1` |
| 23 | 8 | 2 | 8 | `FAIL:late_lost_wake:ready=0 revents=0x0000 elapsed_ms=5263 delay_ms=160` |
| 24 | 8 | 2 | 8 | `FAIL:late_lost_wake:ready=0 revents=0x0000 elapsed_ms=5352 delay_ms=160` |
| 25 | 8 | 2 | 8 | `FAIL:late_lost_wake:ready=0 revents=0x0000 elapsed_ms=5254 delay_ms=160` |
| 26 | 8 | 2 | 8 | `FAIL:late_lost_wake:ready=0 revents=0x0000 elapsed_ms=5246 delay_ms=160` |
| 27 | 8 | 2 | 8 | `FAIL:late_lost_wake:ready=0 revents=0x0000 elapsed_ms=5156 delay_ms=160` |
| 28 | 11 | 5 | 11 | `FAIL:late_window_missed:attempts=5 overshoot_ms=165 delay_ms=1280` |
| 29 | 9 | 3 | 9 | `FAIL:late_lost_wake:ready=0 revents=0x0000 elapsed_ms=5088 delay_ms=320` |
| 30 | 7 | 1 | 7 | `PASS:stages=3:idle_ms=1545:late_ms=2019:park_ms=1951:attempts=1` |

## Histograms

removed: 7 -> 4 boots, 8 -> 16 boots, 9 -> 1 boots, 10 -> 2 boots, 11 -> 7 boots
verdict: FAIL:late_lost_wake -> 20 boots, FAIL:late_window_missed -> 6 boots, PASS -> 4 boots

Main emits removed=6 exactly. The minimum this branch could emit was removed=7,
on the happy path with one attempt, so there was no configuration in which the
oracle passed this gate -- a deterministic 30/30 red, not a flake.

With poll_tcp_oracle un-wired on x86 (#697) the gate emits removed=6 and passes:
run-x86-boot-tests.sh 3 -> 3/3 PASS at the landing bytes.
