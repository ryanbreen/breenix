# Round 4 — `kstrandd`, the lost wake it found, and the asserted age

The 15 captures here were produced on the beast `breenix-x86` container in
`/root/breenix-775`, one QEMU at a time. The production boots are
`docker/qemu/run-x86-prod-profile-boot-test.sh`, which builds with no
`--features` flag; the gate boots are `docker/qemu/run-x86-gate.sh 1 full`
(150 s each).

| directory | head | what it is |
|---|---|---|
| `kstrandd-lost-wake/boot{1..6}` | `4358fd05` | `kstrandd` present, its timer wake lost. 6 of 6 `PASS`, but 1 or 2 census markers per boot and 3 of 6 carrying only the pump's pre-init snapshot. |
| `production/boot{1..6}` | `61447f9c` | six fresh boots with both `blocked_in_syscall` producers guarded. 6 of 6 `PASS`, 37 to 55 markers per boot, 6 of 6 carrying a post-init snapshot. |
| `boot-replay/` | three heads | the x86 TEST-profile crash `kstrandd` surfaced, its control at the round-3 head, and the diagnosis. See that directory's README. |
| `gate-green/boot{1,2}` | `b0b38894` | the TEST-profile gate after the diagnosis: `GATE: PASS` on 2 of 2. |

## The production arm, boot by boot

| boot | lost-wake markers | fixed markers | newest snapshot, fixed |
|---:|---:|---:|---|
| 1 | 2 | 54 | `seq=54:tick=11747:ms=62044:saved=13:stranded=7` |
| 2 | 2 | 37 | `seq=37:tick=7812:ms=42459:saved=12:stranded=7` |
| 3 | 1 | 54 | `seq=54:tick=11473:ms=60728:saved=12:stranded=7` |
| 4 | 1 | 55 | `seq=55:tick=11680:ms=61616:saved=12:stranded=7` |
| 5 | 2 | 53 | `seq=53:tick=10863:ms=62189:saved=12:stranded=7` |
| 6 | 1 | 40 | `seq=40:tick=8739:ms=47100:saved=12:stranded=7` |

On 6 of 6 fixed-arm boots `seq=1` is the loopback pump and `seq=2` is
`kstrandd` at ms 1798-1978, about a second later; the rest of each boot is
`kstrandd` at its 1 Hz cadence. The two arms are not the same six boots -- the
fixed arm was re-run at the final head so the second producer's fix is in it
too -- so the marker columns are two populations of 6, not a paired diff. The `stranded=7` reading is the round-3
caveat unchanged: on this profile those seven threads are parked in syscalls,
the production gate does not call `scripts/x86-gate-verdict.sh`, and no in-repo
consumer reads a production capture as a verdict.
<!-- claim-lint:ok: the 12 marker counts above are `grep -c
     DISPATCH_STRAND_CENSUS` over the 12 committed serial_kernel.txt files. -->

## The gate arm, and the age assertion on a real capture

| boot | verdict | markers | age at the completion marker |
|---:|---|---:|---|
| 1 | `x86 userspace gate: PASS - exited=22 expected>=10 nonzero=0 allowlist=0` | 118 | `1137 ms (newest cadence snapshot seq=28 at 49903 ms, completion snapshot seq=29 at 51040 ms, bound 5000 ms)` |
| 2 | `x86 userspace gate: PASS - exited=22 expected>=10 nonzero=0 allowlist=0` | 115 | `842 ms (newest cadence snapshot seq=25 at 50173 ms, completion snapshot seq=26 at 51015 ms, bound 5000 ms)` |

Both are well inside the 5000 ms bound, which is the point: the bound is a
ratchet against a census that stops being refreshed, not a tight fit. The
round-3 head's committed capture would have read 793 ms on the same measure.
