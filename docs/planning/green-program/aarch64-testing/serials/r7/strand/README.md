# The boot-continuation strand, and the census reading that followed its fix

Round 7 reported the strand as "2 of the first 3" boots and quoted a post-fix
census reading of `queued_nondispatch_ms=1192` / `cpu_silence_ms=1192`. Neither
number had a committed serial (R7-007). The boots were still on the host, so
they are committed here — and two of round 7's numbers do not survive them.

## The three boots after the abort and wedge repairs

`post-abort-wedge-fix-boot{1,2,3}.txt` are the first three strict boots run once
the §1 and §2 repairs were in, before the §3 strand repair.

```
$ for f in post-abort-wedge-fix-boot1.txt post-abort-wedge-fix-boot2.txt post-abort-wedge-fix-boot3.txt; do
      echo "$f: $(grep -aho 'SCHED_STRAND_FIRST[^]]*' $f | head -1)"; done
post-abort-wedge-fix-boot1.txt: SCHED_STRAND_FIRST:tid=11:shape=running:priv=kernel:state=1:dwell_ms=2025
post-abort-wedge-fix-boot2.txt: SCHED_STRAND_FIRST:tid=11:shape=running:priv=kernel:state=1:dwell_ms=2027
post-abort-wedge-fix-boot3.txt: SCHED_STRAND_FIRST:tid=11:shape=running:priv=kernel:state=1:dwell_ms=2043
$ grep -alc "stranded=1" post-abort-wedge-fix-boot*.txt | wc -l
       3
```

**3 of 3, not 2 of 3.** Every one of the three boots stranded `kboot` (tid 11)
and every one carries a `SCHED_STRAND_ORACLE:...:stranded=1` verdict line.
Round 7's "2 of 3" counted the gate's *first-reported* failure class, and boot 1
was reported under `census_widen_oracle` instead (its own census line reads
`cpu_silence_ms=0:...:FAIL`); that stdout was not preserved, so the corrected
statement is the one the serials support.

claim-lint:ok: 3 of 3 boots, the two commands above run against this directory.

`post-abort-wedge-fix-boot2.txt` is the same boot as this round's already
committed `../strict-prefix-boot-continuation-strand.txt`; the two differ only
in that the earlier copy had its CR bytes stripped
(`cmp` reports the first difference at line 605, a `^M`).

## The census reading after the §4 fix

```
$ grep -aho "CENSUS_WIDEN_ORACLE[^]]*" post-census-fix-silence-*.txt
CENSUS_WIDEN_ORACLE:aarch64:arm_target=4:...:queued_nondispatch_ms=1142:cpu_silence_ms=1142:joined=1:retired=1:PASS
CENSUS_WIDEN_ORACLE:aarch64:arm_target=4:...:queued_nondispatch_ms=1112:cpu_silence_ms=1112:joined=1:retired=1:PASS
```

(the `...` above elides `baseline_reported=0:armed_reported=1:tid=1204:
shape=ready_queued_nondispatching:queued_nondispatching=1`, identical in both.)

The committed post-fix readings are **1142** and **1112**, each with
`cpu_silence_ms` equal to `queued_nondispatch_ms` — which is the point the fix
was making. `1192` appears in no preserved serial from this round; it was an
uncommitted number and is restated to these two.

claim-lint:ok: 2 of 2 post-fix serials, `grep` output above.
