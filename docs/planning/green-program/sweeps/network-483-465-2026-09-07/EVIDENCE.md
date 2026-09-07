# Evidence: #483 / #465 verification contracts

Facts only. No closure judgement is made in this document.

- **Old sweep SHA**: `6346f2c5381776a0d64ba471538207a71c1cab4` (`origin/main`
  at 2026-09-06 sweep time), evidence at
  `docs/planning/green-program/sweeps/input-gui-aarch64-2026-09-06/`.
- **New worktree SHA**: `891ae034f8c0bb5de0b9ad400cf6524a7e92645c` (`origin/main`
  at fetch time 2026-09-07), detached worktree at
  `/private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/v483/wt`
  (`git rev-parse HEAD` = `891ae034…`, `git status --short` empty at creation).
- 71 commits separate the two SHAs (`git log --oneline 6346f2c5..891ae034 | wc -l`).
  Restricted to the two files the waitqueue/scheduler work touches:

  ```
  $ git log --oneline 6346f2c5..HEAD -- kernel/src/task/waitqueue.rs kernel/src/task/scheduler.rs
  e4ce506b tracing(855): self-publish ring spans and pad per-CPU counters
  e63edadb tracing: double ring capacity after diagnostic sampling plateau
  504d3667 tracing(aarch64): sample scheduler diagnostics and preserve retention limits
  2653a373 Merge remote-tracking branch 'origin/main' into sched/3f-pre-oracle-attribution
  ce56e44d review-fix(3f-pre,pr1): correct the round record's numeric slips, ...
  1c9f0f53 fix(scheduler): attribute the pin-guard oracle's own refusals, ...
  ```

  `kernel/src/task/waitqueue.rs` itself has **0** commits in that range (the
  file did not change between the two SHAs). 6 of the 6 `scheduler.rs`
  commits above are tracing/diagnostic-counter additions (`TRACE_SCHED_DIAG_SAMPLE`
  buffer sizing, per-CPU ring self-publish, cache-line padding) plus one
  `fix(scheduler)` commit whose entire changed path is
  `cfg(all(target_arch = "aarch64", feature = "boot_tests"))`-gated attribution
  of a test-only pin-guard oracle counter <!-- claim-lint:ok: "all" is Rust's cfg(all(...)) combinator syntax, kernel/src/task/scheduler.rs --> —
  0 of the 6 commits touch the waitqueue wake/block ordering the compositor
  path depends on.
  So the two SHAs' shipped (no-feature) waitqueue/scheduler behavior is the
  same; the delta is boot_tests-only instrumentation.

## Contract — #483 (F32c waitqueue deterministic reproducer and Linux parity fix)

Verbatim from `gh issue view 483 --json body -q .body`:

> Factory F32c: build a deterministic waitqueue race reproducer, audit
> Breenix waitqueue ordering against Linux, apply the specific ordering
> fix, and validate without compositor timer fallbacks.
>
> **Acceptance Criteria**
>
> wait_stress reproducer committed; ordering audit committed; Linux-parity
> fix committed; 60s wait_stress has 0 stalls; 5x120s Parallels sweep
> passes; no compositor BlockedOnTimer fallback remains

Six discrete AC clauses:

1. wait_stress reproducer committed
2. ordering audit committed
3. Linux-parity fix committed
4. 60s wait_stress has 0 stalls
5. 5x120s Parallels sweep passes
6. no compositor BlockedOnTimer fallback remains

## Contract — #465 (F32e Linux prepare_to_wait waitqueue parity)

Verbatim from `gh issue view 465 --json body -q .body`:

> Audit Breenix waitqueue blocking against Linux prepare_to_wait_event,
> implement the missing state-check-and-block behavior, validate
> wait_stress and Parallels gates, and merge only if all required gates
> pass. <!-- claim-lint:ok: verbatim quote of issue #465's own body -->

Three discrete clauses: audit against `prepare_to_wait_event`; implement
the missing state-check-and-block behavior; "validate wait_stress and
Parallels gates, and merge only if all required gates pass" — this issue
does **not itself enumerate** what its own quoted "all required gates"
phrase consists of. <!-- claim-lint:ok: quoting issue #465's own "all required gates" phrase -->

## Gap: neither issue body names "FPS >= 160" or an AHCI/fatal-marker grep list

The task brief that produced this evidence pass summarized the contract as
including `FPS >= 160` and "no AHCI timeout/fatal marker." A search of both
issue bodies quoted in full above found 0 occurrences of either string and
0 FPS numbers. That checklist is boilerplate "Verification / oracle" text
this same sweep's own plan document (`evidence/PLAN-LAYER-SWEEP.md.orig`)
applies to two other issues in its 19-issue set — verbatim, word-for-word,
at both `### #447` (line 939) and `### #452` (line 959):

> Five independent `./run.sh --parallels --test 120` runs, each with actual
> service lifecycle, CPU0 progress, rendered desktop and original FPS ≥160
> criterion, no AHCI timeout/fatal marker.
> <!-- claim-lint:ok: verbatim quote of PLAN-LAYER-SWEEP.md.orig lines 939/959 -->

#483 and #465 have 0 `### #NNN` sections of their own in that plan document
or in `EVIDENCE.md` (`grep -n "^### #" EVIDENCE.md` lists 19 headers,
neither number among them; `grep -n "483\|465"` across both files returns
1 hit total, an unrelated line-number citation `init.rs:483–486`). This
same sweep's `DISPOSITIONS.md:857` states, for a third issue:

> #465/#483 are adjacent, not established duplicates of this exact
> validation task.
> <!-- claim-lint:ok: verbatim quote of DISPOSITIONS.md:857 -->

So: this sweep's five-run/wait-stress evidence is genuine runtime evidence
about the same waitqueue/compositor subsystem #483 and #465 are about, but
this pass found 0 places in that sweep's own documents indexing it against
#483 or #465 by number, and the FPS/AHCI checklist applied below is
inherited methodology, not literal AC text from either issue.

`5x120s Parallels sweep passes` (#483 clause 5) and `60s wait_stress has 0
stalls` (#483 clause 4) **are** literal AC text, so the WAIT_STRESS and
lifecycle runs below are read against those two clauses directly, plus the
inherited FPS/fatal-marker checklist for completeness since it is what the
brief asked to confirm.

## Run table

Each of the 7 "fatal grep" counts below (5 old-SHA runs + 2 new-SHA runs)
is `grep -aEc "AHCI.*[Tt]imeout|FATAL|PANIC|DATA_ABORT|SOFT_LOCKUP"
<serial.log>` run directly against that run's own named file.

### WAIT_STRESS run (old sweep, SHA 6346f2c5)

| Field | Value | Source |
|---|---|---|
| Command | `BREENIX_WAIT_STRESS=1 ./run.sh --parallels --test 150` | — |
| Start line | `WAIT_STRESS_START duration=60s sample=100ms` | `evidence/run2-wait-stress/serial.log` |
| Pass line | `WAIT_STRESS_PASS entered=2013817 returned=2013816 wakes=17409490 waiters=0` | same file |
| STALL count | 0 (`grep -aE "WAIT_STRESS_PASS\|WAIT_STRESS_FAIL\|STALL"` returns exactly the one PASS line) | same file |
| Fatal grep | 0 | same file |
| Render verdict | `VERDICT=PASS` (`distinct=2002 dominant=(10,10,25) coherent_region=bucket=(3,3,9) frac=0.1462 bbox_frac=0.4424`) — re-run directly in this pass via `bash scripts/f24-render-verdict.sh` against the preserved screenshot, matches the old sweep's own `EVIDENCE.md:185-200` | `evidence/run2-wait-stress/screenshot.png` |

Satisfies #483 clause 4 (60s wait_stress, 0 stalls) directly.

### Five/two-run test-120 lifecycle table (old sweep SHA 6346f2c5, runs 1-5; new worktree SHA 891ae034, runs 6-7)

| # | SHA | VM | Lifecycle | Last heartbeat | FPS min/max (samples) | Fatal grep | Screenshot verdict |
|---|---|---|---|---|---|---|---|
| 1 | 6346f2c5 | breenix-1788739125 | init→…→bsshd(16)→xhci_counters(17)→bwm(18)→telnetd(19)→"Boot script completed"→bounce(20) | `uptime_ms=175669` | 131/244 (173, 13 samples <160) | 0 | **FAIL** distinct=1 dominant=(0,0,0) |
| 2 | 6346f2c5 | breenix-1788739339 | same sequence | `uptime_ms=171641`* | 141/270 (153, 4 samples <160) | 0 | **PASS** coherent_region bbox_frac=0.4424 |
| 3 | 6346f2c5 | breenix-1788739535 | same sequence | `uptime_ms=139588`* | 183/253 (137, 0 samples <160) | 0 | **FAIL** distinct=1 dominant=(0,0,0) |
| 4 | 6346f2c5 | breenix-1788739705 | **STALLED** — filed as #906; last line `[net] ICMP echo reply received from 10.211.55.1 seq=1`; 0 SMP-bring-up lines, 0 service-spawn lines logged | 0 `[heartbeat]` lines | 0 fps samples | n/a — boot did not reach the later AHCI-timeout-prone stages | n/a — 0 `[bwm-fps]` lines |
| 5 | 6346f2c5 | breenix-1788740082 | same sequence | `uptime_ms=201631`* | 137/261 (178, 17 samples <160) | 0 | **PASS** coherent_region bbox_frac=0.4424 |
| 6 | 891ae034 | breenix-1788758111 | same sequence; `block_eintr_oracle exited pid=3 code=0` | `uptime_ms=214672` (214 samples) | 185/248 (212, 0 samples <160) | 0 | **FAIL** distinct=1 dominant=(0,0,0) |
| 7 | 891ae034 | breenix-1788758416 | same sequence; `block_eintr_oracle exited pid=3 code=0` | `uptime_ms=189631` (189 samples) | 180/253 (186, 0 samples <160) | 0 | **FAIL** distinct=1 dominant=(0,0,0) |

\* Runs 2/3/5's heartbeat values were re-derived directly from each run's own
`serial.log` for this pass (`grep -a "[heartbeat]" … | tail -1`) rather than
copied from the prior sweep's summary table, which reported different
numbers for run 1 (163664 vs the 175669 the raw log actually contains) —
runs 2/3/5 matched the prior table exactly on re-check; run 1 did not, so
runs 2/3/5's re-derived values are cited above and run 1's original
mismatch is called out as a `gaps` entry below.

Full lifecycle line-set common to the 6 of 7 runs that completed
(1,2,3,5,6,7), read directly off each run's own `[init]` lines:
```
[init] Breenix init starting (PID 1)
[init] block_eintr_oracle exited pid=3 code=<0 or 1>
[init] futex_handoff_oracle exited pid=6 code=0
[init] poll_tcp_oracle exited pid=7 code=0
[init] tty_oracle exited pid=10 code=0
[init] clonevm_exec_test exited pid=13 code=0
[init] bsshd started (PID 16)
[init] Boot script completed
[init] bounce started (PID 20)
[init] Process 4/5/14/15/17 exited (code 0)
```
(runs 6/7 verified directly against
`evidence/run1-test120/serial.log` and `evidence/run2-test120/serial.log`
in this pass; runs 1/2/3/5 against the old sweep's own
`evidence/run3-test120-{1,2,3,5}/serial.log`.)

File paths for the two new runs:
- `evidence/run1-test120/{serial.log,screenshot.png,stdout.log}` (run 6, VM `breenix-1788758111`)
- `evidence/run2-test120/{serial.log,screenshot.png,stdout.log}` (run 7, VM `breenix-1788758416`)
File paths for the five old runs (unchanged, cited not re-copied):
`docs/planning/green-program/sweeps/input-gui-aarch64-2026-09-06/evidence/run3-test120-{1,2,3,4,5}/`
and `.../evidence/run2-wait-stress/` for the WAIT_STRESS run.

## Contract × criteria

### #483

| Clause | Status | Evidence |
|---|---|---|
| 1. wait_stress reproducer committed | present at both SHAs | `userspace/programs/src/wait_stress.rs` exists in the worktree at `891ae034` |
| 2. ordering audit committed | not checked by this pass (doc-search, not runtime) | out of scope — see gaps |
| 3. Linux-parity fix committed | partial support | `handle_compositor_wait` in `kernel/src/syscall/graphics.rs` at `891ae034` uses `COMPOSITOR_FRAME_WQ.prepare_to_wait(ThreadState::BlockedOnIO)` / `finish_wait()` with a race-closure re-check, matching Linux `prepare_to_wait_event` shape; not independently diffed against a named "ordering audit" doc |
| 4. 60s wait_stress has 0 stalls | **met** (old-sweep evidence only; not re-run this pass) | `WAIT_STRESS_PASS` line above, 0 `STALL` occurrences |
| 5. 5x120s Parallels sweep passes | **not fully met** | 6 of 7 completed+attempted test-120 runs across both SHAs show healthy service lifecycle/heartbeat/FPS with 0 fatal-grep hits, but only 2 of those 6 (runs 2, 5, both old-SHA) show a **PASS** render verdict; run 4 stalled entirely (filed #906); both new-SHA runs (6, 7) show a healthy serial log but a **FAIL** screenshot verdict (solid black capture, `distinct=1`) |
| 6. no compositor BlockedOnTimer fallback remains | **met** at `891ae034` | `grep -n "BlockedOnTimer" kernel/src/syscall/graphics.rs userspace/programs/src/bwm.rs` returns 0 matches in the new worktree (checked directly in this pass) |

### #465

| Clause | Status | Evidence |
|---|---|---|
| Audit against Linux `prepare_to_wait_event` | not independently produced by this pass | see #483 clause 2/3 |
| Implement missing state-check-and-block behavior | present at `891ae034` | same `prepare_to_wait`/`finish_wait` code cited above |
| Validate wait_stress and Parallels gates; merge only if all required gates pass<!-- claim-lint:ok: quoting issue #465's own body --> | same partial picture as #483 clause 5 | WAIT_STRESS passes (old SHA only); Parallels test-120 lifecycle passes at both SHAs; screenshot-verdict gate is 2/6 PASS across both SHAs |

## Gaps

- **#483 and #465 are not in the indexed sweep's per-issue evidence.** The
  `input-gui-aarch64-2026-09-06` sweep's `EVIDENCE.md` has 19 `### #NNN`
  sections; a grep of both files for "483" and "465" (`grep -n "483\|465"
  EVIDENCE.md DISPOSITIONS.md`) returns 0 substantive hits (1 unrelated
  line-number citation, `init.rs:483-486`). That sweep's own
  `DISPOSITIONS.md:857` calls #465/#483 "adjacent, not established
  duplicates" of the one issue (#436) it does discuss in the same
  neighborhood. The WAIT_STRESS/lifecycle runs used above are the same
  underlying subsystem but this pass found 0 places attributing them to
  #483/#465 specifically.
- **The "FPS >= 160" / "no AHCI timeout/fatal marker" checklist is not
  literal AC text in either issue** — see the Gap section above. It is
  copied from this sweep's own generic "Verification / oracle" boilerplate
  for two *other* issues (#447, #452).
- **The screenshot/render-verdict gate is the weakest point of the whole
  picture: only 2 of 6 completed test-120 runs across both SHAs (both from
  the older SHA) pass it**, despite every one of the 6 showing a healthy
  serial log (full service lifecycle, sustained heartbeat, 130-270 fps
  samples, 0 fatal-grep hits). Both new-SHA runs this pass added are FAIL
  on this specific measure. The same `ERROR: No Parallels window found
  matching '<vm>'` line appears in the `stdout.log` of every one of the 7
  attempted runs (old and new alike, PASS and FAIL alike) immediately before
  the screenshot capture step, so that message does not by itself explain
  the PASS/FAIL split — it fires unconditionally. All FAIL screenshots
  (both old and new) are byte-identical in size to each other (e.g. the two
  new runs' `screenshot.png` are both exactly 3674 bytes), consistent with a
  capture-timing/fallback issue in the screenshot step rather than a
  render failure the serial log would also show.
- **Only 2 fresh test-120 boots were run on the new SHA** (891ae034) per the
  task's own instruction ("two more"); no fresh WAIT_STRESS run was
  performed at 891ae034 in this pass, so #483 clause 4 (0 stalls) is
  evidenced only at the old SHA, not re-confirmed at current main.
- **AC clauses 1-3 of #483** ("wait_stress reproducer committed," "ordering
  audit committed," "Linux-parity fix committed") are commit/artifact
  claims, not runtime claims; this pass only spot-checked that the
  reproducer file and a `prepare_to_wait`-shaped compositor path exist at
  `891ae034` — it did not locate or verify a named "ordering audit"
  document, and did not diff the current waitqueue implementation against
  Linux `prepare_to_wait_event` line-by-line.
- **Run 1's old-sweep heartbeat value in the prior EVIDENCE.md table
  (`uptime_ms=163664`) does not match what `run3-test120-1/serial.log`
  actually contains** (`uptime_ms=175669`, the last of 175 heartbeat lines
  in that file) — re-derived directly for this pass rather than trusted
  from the earlier table; not something this pass corrects in that file,
  only flags.
