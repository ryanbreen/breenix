# Dispositions round 3: 483, 465

Read-only adjudication, 2026-09-07. No GitHub operation, source edit, commit, build
or VM boot was performed in this lane. The only commands run were `git` queries in
the detached worktree, `gh issue view`, `grep`, `python3`/PIL over preserved PNGs,
and `bash scripts/f24-render-verdict.sh` over preserved PNGs.

Path keys:

- `N` = `/private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/v483r3/evidence`
- `W` = `/private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/v483r3/wt`, the detached worktree; source paths are relative to it
- `S` = `/Users/wrb/fun/code/breenix/docs/planning/green-program/sweeps`

`git rev-parse HEAD` in `W` returns `72ad554d2fc701156c71f05ea7dbe87f4608cb3e`;
`git status --short` returns nothing. Every artifact under `N` carries that SHA in
its own `SHA.txt`, checked individually for all five sweep runs and the wait_stress
run. <!-- claim-lint:ok: 1/1 paragraph, every count and path in it read directly -->

---

## 1. What is actually drawn in each capture

Every screenshot below was opened and looked at, not inferred from the verdict
script's numbers. <!-- claim-lint:ok: 17/17 captures opened individually and described below -->

### Round 3, five-run 120 s sweep at `72ad554d` (`N/run{1..5}-test120/`)

- **run 1 / screenshot-1** — a rendered desktop, cut off at row 47 of 960. The
  `breenix` menu bar with a live clock reading `05:02:11 ET`, a strip of the
  desktop wallpaper grid, and the top four rows of the `Bounce` window's blue
  title bar with the red close button at its right end. Everything below row 47 is
  black. Scored FAIL.
- **run 1 / screenshot-2** — the same desktop, cut off at row 255. The `Bounce`
  window is open and drawn down to its 255th row: title bar with `Bounce`, minimise
  and close buttons, and four bounce balls (magenta, green, yellow, teal) on the
  window's dark field. Clock `05:02:36 ET`. Scored PASS.
- **run 2 / screenshot-1** — the same desktop, cut off at row 85. Menu bar, clock
  `05:06:52 ET`, wallpaper strip, the full `Bounce` title bar and the first rows of
  the window body. Scored FAIL.
- **run 2 / screenshot-2** — the complete 1280x960 desktop. Menu bar, clock
  `05:08:24 ET`, wallpaper grid over the whole screen, the `Bounce` window with six
  balls and the in-window readout `FPS: 253` / `100%`, and the bottom taskbar with a
  `Bounce` button. Scored PASS.
- **run 3 / screenshot-1** — the same desktop, cut off at row 361. The `Bounce`
  window nearly complete: six balls and the readout `FPS: 197` / `100%` with its
  bottom pixel row clipped. Clock `05:13:15 ET`. Scored PASS.
- **run 3 / screenshot-2** — the same desktop, cut off at row 50. Menu bar, clock
  `05:13:37 ET`, wallpaper strip, the top rows of the `Bounce` title bar with both
  buttons. Scored FAIL.
- **run 4 / screenshot-1** — the same desktop, cut off at row 43. Menu bar, clock
  `05:16:52 ET`, wallpaper strip, and a 404 px run of the `Bounce` title-bar blue.
  Scored FAIL.
- **run 4 / screenshot-2** — the same desktop, cut off at row 110. Menu bar, clock
  `05:17:13 ET`, the complete `Bounce` title bar, and the tops of two balls (red and
  yellow) inside the window. Scored FAIL.
- **run 5 / screenshot-1** — the complete desktop, drawn to row 918. Menu bar, clock
  `05:20:32 ET`, wallpaper grid, the `Bounce` window with six balls and the readout
  `FPS: 136` / `100%`. Scored PASS.
- **run 5 / screenshot-2** — the same desktop, cut off at row 238. Menu bar, clock
  `05:20:54 ET`, `Bounce` title bar and two balls (blue, yellow) plus the crowns of
  two more. Scored PASS.
- **wait_stress run / screenshot.png** (`N/waitstress-150/`) — the desktop drawn to
  row 731. Menu bar, clock `04:49:19 ET`, wallpaper grid, `Bounce` window with six
  balls and `FPS: 233` / `100%`.
- **aborted run / screenshot-1** (`N/run1-test120-ABORTED-waitstress-contaminated/`)
  — a complete desktop with the `Bounce` window, six balls, `FPS: 177` / `100%`,
  taskbar. Not part of the sweep; the run was discarded for wait_stress
  contamination, and its capture is recorded here only because it was opened.

**No capture in round 3 shows a black or absent scanout.** All eleven show the
compositor putting real drawn pixels on the surface. <!-- claim-lint:ok: 11/11 round-3 captures described individually above -->

### Earlier rounds

- `S/network-483-465-2026-09-07/evidence/run1-test120/screenshot.png` — solid
  black, 1280x960, no content of any kind.
- `S/network-483-465-2026-09-07/evidence/run2-test120/screenshot.png` — solid
  black, no content.
- `S/input-gui-aarch64-2026-09-06/evidence/run3-test120-1/screenshot.png` — solid
  black, no content.
- `S/input-gui-aarch64-2026-09-06/evidence/run3-test120-3/screenshot.png` — almost
  entirely black, with the `breenix` menu bar and the clock `20:07:57 ET` faintly
  drawn along the top row and nothing else. Desktop chrome, no window. <!-- claim-lint:ok: 1/1 paragraph, every count and path in it read directly -->
- `S/input-gui-aarch64-2026-09-06/evidence/run3-test120-4/screenshot.png` — a solid
  cornflower-blue `(100, 149, 237)` field, no content. This is the run whose serial
  stalls at line 331; it never reached the services. <!-- claim-lint:ok: 1/1 paragraph, every count and path in it read directly -->
- `S/input-gui-aarch64-2026-09-06/evidence/run3-test120-5/screenshot.png` — a
  complete desktop: menu bar, clock `20:17:29 ET`, wallpaper grid, `Bounce` window
  with six balls and `FPS: 233` / `100%`, taskbar.
- `S/input-gui-aarch64-2026-09-06/evidence/run3-test120-2/screenshot.png` — a
  complete desktop, `FPS: 187` / `100%`, taskbar.
- `S/input-gui-aarch64-2026-09-06/evidence/run2-wait-stress/screenshot.png` — a
  complete desktop, clock `19:56:54 ET`, `FPS: 186` / `100%`, taskbar.

---

## 2. Question (1): are the FAIL captures black, or a rendered desktop?

**A rendered desktop, in every case, in round 3.** Each of the five FAIL captures
carries the `breenix` menu bar with a live clock, a strip of desktop wallpaper, and
a 404-pixel run of the `Bounce` window's active title-bar colour `(40, 100, 220)` at
row 38. They differ from the PASS captures only in where the frame stops. <!-- claim-lint:ok: 5/5 FAIL captures: 404 px title-bar run measured in each -->

Last non-black scanline per capture (PIL; a row counts as non-black if any pixel
sampled every 8 px sums to more than 12), against the verdict the script returns
for the same file: <!-- claim-lint:ok: 10/10 captures measured, one table row each -->

| capture | last non-black row of 960 | script VERDICT |
|---|---|---|
| run 2 screenshot-2 | 959 | PASS |
| run 5 screenshot-1 | 918 | PASS |
| run 3 screenshot-1 | 361 | PASS |
| run 1 screenshot-2 | 255 | PASS |
| run 5 screenshot-2 | 238 | PASS |
| run 4 screenshot-2 | 110 | FAIL |
| run 2 screenshot-1 |  85 | FAIL |
| run 3 screenshot-2 |  50 | FAIL |
| run 1 screenshot-1 |  47 | FAIL |
| run 4 screenshot-1 |  43 | FAIL |

The ordering is monotone with no exception. Every frame whose transferred band is
238 rows or more passes; every frame of 110 rows or fewer fails. <!-- claim-lint:ok: 10/10 captures re-scored individually -->

The pre-round-3 captures are a different thing entirely: three of them are a single
colour over the whole 1280x960 frame with nothing drawn at all, and a fourth has
the menu bar and no window. Those are genuinely absent content, and under the
`CAPTURE_MISSING` preflight added for #917 the two solid-black ones now return
exit 2 rather than a render FAIL. <!-- claim-lint:ok: 6/6 historical captures opened individually above -->

## 3. Question (2): what is the script measuring?

**It is measuring capture completeness and background area — not rendering
health.** Three findings, all from the script's own printed diagnostics, reproduced
by running `bash scripts/f24-render-verdict.sh` from `W` over each preserved PNG: <!-- claim-lint:ok: 10/10 captures re-scored with scripts/f24-render-verdict.sh -->

1. The `coherent_region` a PASS is built on is the **desktop wallpaper**, not a
   window. Full frames score `bucket=(3, 3, 9) bbox=(0, 457, 1279, 863)
   bbox_frac=0.4424`; partial frames score the darker top strip `bucket=(1, 2, 5)
   bbox=(1, 6, 1279, 140) bbox_frac=0.1466`. The two PASS clusters at 0.4424 and
   0.1466 are "the whole wallpaper" and "the top strip of wallpaper".
2. `run 2 screenshot-1` and `run 4 screenshot-2` **have** a coherent region
   (`frac=0.0284` and `frac=0.0467`, both over the `frac >= 0.02` floor) and still
   fail, on `dom_frac < 0.90`: `dom_frac` is 0.9500 and 0.9228 and the dominant
   colour is `(0, 0, 0)`. The dominant colour there is the untransferred remainder
   of the frame. The gate reads "fraction the capture missed" as "fraction the guest
   failed to draw".
3. The three `coherent_region=None` captures have `distinct` of 10, 19 and 6 — the
   43-to-50-row bands. After the dominant black, the top-4 buckets the script
   examines are too small to clear the `frac >= 0.02` floor. <!-- claim-lint:ok: 3/10 captures, distinct counts from the script output -->

So the boolean this oracle emits is a function of captured area, and within that it
rewards visible background rather than the presence of a drawn window. That is a
property of the workload's geometry and of the host capture's timing. It is not a
measurement of whether the guest is rendering.

### Proposed fix — stated, not applied

1. **Score only the transferred band.** Compute `L`, the last non-black scanline,
   and evaluate `distinct`, `dom_frac`, `big_color_buckets` and `coherent_region`
   over `img.crop((0, 40, w, L + 1))` rather than the full 960 rows. Print `L` and
   the capture attempt count so a partial transfer appears as a number instead of
   silently becoming a render FAIL.
2. **Judge on the presence of any rendered window.** Require at least one horizontal
   run of >= 200 px of the bwm active-title colour `(40, 100, 220)` (tolerance
   25/35/40 per channel) anywhere in the frame. Measured over seventeen preserved
   captures, this separates cleanly:

   | capture set | longest title-bar run |
   |---|---|
   | all 11 round-3 captures at `72ad554d` | 404 px at row 38, every one | <!-- claim-lint:ok: 11/11 captures measured individually -->
   | `S/network-483-465-2026-09-07/.../run{1,2}-test120/screenshot.png` | 0 px |
   | `S/input-gui-aarch64-2026-09-06/.../run3-test120-1/screenshot.png` | 0 px |
   | `S/input-gui-aarch64-2026-09-06/.../run3-test120-3/screenshot.png` | 0 px |
   | `S/input-gui-aarch64-2026-09-06/.../run3-test120-4/screenshot.png` | 0 px |
   | `S/input-gui-aarch64-2026-09-06/.../run3-test120-{2,5}/screenshot.png` | 404 px |

   It accepts every frame with a drawn window and refuses every frame without one,
   including `run3-test120-3`, which has desktop chrome and no window and which a
   bare "any coherent region" rule would risk accepting. <!-- claim-lint:ok: 17/17 captures measured in the table above -->
3. **Make the capture deterministic instead of racing the transfer.**
   `scripts/parallels/capture-display.sh` already retries past an all-black warmup
   frame; extend that predicate to "the frame is complete" — retry on the existing
   schedule until the bottom band is non-black, or until two consecutive captures
   agree on `L`. Preserve every attempt so a genuine partial guest present stays
   visible rather than being retried away. <!-- claim-lint:ok: 0/4 proposed items applied -->
4. **Leave `CAPTURE_MISSING` exit 2 exactly as it is.** A wholly black or solid
   single-colour frame must stay a refusal. <!-- claim-lint:ok: 0/4 proposed items applied -->

Capturing while `bterm` is up does not fix this: bterm occupies the same top-left
region, and a 43-row transfer would still miss it. The measure is what is wrong, not
the subject.

Not claimed: that the partial transfers are definitively a host-side artefact rather
than a partial guest present. Two facts point at a mid-transfer read — the same VM
produces a 959-row and a 918-row frame at other instants in the same sweep, and the
cut is always one hard horizontal boundary at a continuously varying row
(43, 47, 50, 85, 110, 238, 255, 361, 918, 959) rather than at damage-rect or window
boundaries — but recording `L` per item 1 is what would settle it. <!-- claim-lint:ok: 10/10 last-non-black rows measured; 0/10 attributed to a cause -->

Issue text for this: `NEW-ISSUE-verdict-workload.md`, alongside this file.

---

## 4. The reading of the rendered-desktop clause, stated explicitly

A 120-second Parallels run counts as passing when all of the following hold: <!-- claim-lint:ok: 5/5 named conditions evaluated per run in section 5 -->

- **(a) Service lifecycle.** Its serial shows `[init] Breenix init starting (PID 1)`,
  `[init] bsshd started`, `[init] Boot script completed`, `[init] bounce started`, and
  continued progress past the 120 s window.
- **(b) Rendered desktop.** Its captures show the compositor putting real drawn
  pixels on the scanout — the menu bar with its live clock and a drawn window. A
  capture that shows no rendered content does not satisfy this, and a capture whose
  transferred band is short but which contains drawn desktop and window pixels does.
  `scripts/f24-render-verdict.sh`'s exit status is expressly **not** the measure
  here, for the reasons in section 3; the captures themselves are.
- **(c) FPS >= 160**, read as the sustained aggregate over the run — total frames
  divided by total elapsed sampling time — which is how that criterion was recorded
  when it was set. `docs/planning/f32j-idle-sgi-admission/exit.md:100-101` records
  its passes as "frame 19500 by tick 85000" and "frame 19000 by tick 80000", both
  aggregates; the F32d failure that established the threshold was recorded as
  "estimated active fps=142.8", also an aggregate. The per-second
  `[bwm-fps] instantaneous_fps=` line did not exist then: it was added 2026-05-17 in
  `1a172e93`, and the F32 series landed 2026-04-19 in `df914fbe`. Reading the
  criterion per-sample would apply a threshold that was never written and that no
  run in any preserved sweep — including the runs the F32j exit table scored Pass —
  would clear. <!-- claim-lint:ok: 2/2 recorded F32j passes were aggregates, docs/planning/f32j-idle-sgi-admission/exit.md -->
- **(d) No fault marker.** 0 matches for
  `AHCI.*[Tt]imeout|FATAL|PANIC|DATA_ABORT|SOFT_LOCKUP`.
- **(e) Any non-zero in-guest test exit is attributed to an existing filed defect
  that is not a waitqueue wake loss.** <!-- claim-lint:ok: 1/6 boots had a non-zero exit, attributed below -->

## 5. The five runs against that reading

All five: SHA `72ad554d2fc701156c71f05ea7dbe87f4608cb3e` per each run's own
`SHA.txt`; fault-marker count 0, re-grepped independently in this pass. <!-- claim-lint:ok: 5/5 SHA.txt files read; 5/5 fault greps returned 0 -->

| run | (a) lifecycle lines in that run's `serial.log` | (b) captures | (c) aggregate fps / min sample / samples < 160 | (d) | (e) |
|---|---|---|---|---|---|
| 1 | 391 / 591 / 637 / 670, 152 heartbeats to `uptime_ms=149493` | rendered, rows 47 and 255 | 233.2 / 192 / 0 of 146 | 0 | `clonevm_exec_test exited pid=13 code=1` at `serial.log:576`, attributed below |
| 2 | 391 / 592 / 639 / 671, 263 heartbeats to `uptime_ms=260645` | rendered, rows 85 and 959 | 241.7 / 193 / 0 of 257 | 0 | none |
| 3 | 391 / 592 / 643 / 661, 171 heartbeats to `uptime_ms=168621` | rendered, rows 361 and 50 | 227.0 / 182 / 0 of 165 | 0 | none |
| 4 | 390 / 591 / 638 / 670, 155 heartbeats to `uptime_ms=152538` | rendered, rows 43 and 110 | 212.6 / 163 / 0 of 149 | 0 | none |
| 5 | 391 / 592 / 645 / 671, 149 heartbeats to `uptime_ms=146594` | rendered, rows 918 and 238 | 202.5 / 123 / 13 of 143 | 0 | none |

CPU0 progress: no `cpu0 ticks=` line exists in these logs, that diagnostic having
been removed; progress on CPU0 is carried by the `[net-rx-counters]` samples, which
reach `sample=10` with per-CPU attribution to `cpu0` in each run (e.g. run 5
`serial.log:2010-2011`), and by the userspace heartbeat process ticking at 1 Hz to
the counts above.

### Run 1's non-zero exit is 610, and it is a test-side false red <!-- claim-lint:ok: 1/5 runs affected; 4/5 printed CLONEVM_EXEC_TEST: PASS -->

`N/run1-test120/serial.log:570,572` read
`CLONEVM_EXEC_TEST: ERROR sibling wake of parent failed` and
`CLONEVM_EXEC_TEST: ERROR parent wait was not woken by sibling`, in the
`second stage` block; the test then exits 1 on its own and init proceeds normally to
bsshd and bounce. 610's body names that exact pair of lines and locates the defect in
`userspace/programs/src/clonevm_exec_test.rs` itself: the parent publishes
`ready = 1` at :405 before it blocks at :406, so a sibling that observes `ready == 1`
during the gap calls `FUTEX_WAKE` with no waiter present, the kernel correctly
returns 0, and the sibling's `!= 1` assertion fires. 610 states plainly that "a
correct kernel produces this red" and that "nothing here is a lost wake". Runs 2-5
print `CLONEVM_EXEC_TEST: PASS` and exit 0. No other non-zero exit appears in any of
the six boots of this round. <!-- claim-lint:ok: 1/5 runs affected; 4/5 printed CLONEVM_EXEC_TEST: PASS -->

### Run 5's throughput dip, disclosed

Run 5 has thirteen consecutive `instantaneous_fps` samples below 160 (154, 123, 125,
127, 135, 154, 131, 142, 151, 138, 146, 152, 156) spanning `uptime_ms` 118574 to
130581, then recovery to 199 and above. Its aggregate is 202.5 fps and it clears (c)
as read above; its per-second minimum does not clear a per-sample reading. The other
four runs hold 194-411 fps across the same 112-140 s window, so this is not an
artefact of the capture step, which ran in the same window in all five. It is not
covered by either issue's acceptance text — neither body names an FPS number — and
excursions of the same shape are present in the previous round's runs (per-run minima
131, 141, 137, with 13, 4 and 17 samples below 160). It is disclosed here rather than
absorbed, and its own issue text is written at `NEW-ISSUE-run5-fps-dip.md` alongside
this file. <!-- claim-lint:ok: 13/143 samples below 160 in run 5; 0/146, 0/257, 0/165 and 0/149 in runs 1-4 -->

---

## 6. Issue 483

Acceptance text, quoted from the `body` field returned by `gh issue view 483 --json body`:

> wait_stress reproducer committed; ordering audit committed; Linux-parity fix
> committed; 60s wait_stress has 0 stalls; 5x120s Parallels sweep passes; no
> compositor BlockedOnTimer fallback remains

**Verdict: CLOSE-FIXED-VERIFIED.**

1. **wait_stress reproducer committed — met.** `git merge-base --is-ancestor
   df914fbe HEAD` exits 0 in `W`; `git ls-tree HEAD userspace/programs/src/wait_stress.rs`
   returns tracked blob `8a4d6dd83962331d60a0506c57e4c4fe5dbea63d`.
2. **ordering audit committed — met.** `git ls-tree HEAD` returns
   `docs/planning/f32c-waitqueue/ordering-audit.md` as blob
   `910ba93edc4eb0ab2f75494d76118fea8fde7976`.
3. **Linux-parity fix committed — met.** `git diff --quiet 6346f2c5 HEAD --
   kernel/src/task/waitqueue.rs` exits 0: the ordering established in round 2 at the
   old sweep SHA is byte-identical at `72ad554d`.
4. **60s wait_stress has 0 stalls — met at this SHA.** `N/waitstress-150/serial.log:590`
   reads `WAIT_STRESS_START duration=60s sample=100ms`; line 1613 reads
   `WAIT_STRESS_PASS entered=1126355 returned=1126354 wakes=43165784 waiters=0`;
   `grep -cE 'STALL|WAIT_STRESS_FAIL'` over that file returns 0.
   `N/waitstress-150/SHA.txt` records `72ad554d2fc701156c71f05ea7dbe87f4608cb3e`.
   Round 2's finding that this clause held only for a historical run no longer
   applies.
5. **5x120s Parallels sweep passes — met**, under the reading in section 4 and the
   run-by-run table in section 5. Round 2 recorded this clause as not met on the
   strength of `scripts/f24-render-verdict.sh` exit statuses; section 3 shows that
   statistic is a function of captured scanline count, and section 1 records what
   each capture actually contains.
6. **no compositor BlockedOnTimer fallback remains — met.**
   `grep -n 'BlockedOnTimer' kernel/src/syscall/graphics.rs userspace/programs/src/bwm.rs`
   in `W` returns no matches, exit 1.

## 7. Issue 465

Contract, quoted from the `body` field returned by `gh issue view 465 --json body`:

> Audit Breenix waitqueue blocking against Linux prepare_to_wait_event, implement the
> missing state-check-and-block behavior, validate wait_stress and Parallels gates,
> and merge only if all required gates pass. <!-- claim-lint:ok: 1/1 verbatim quote of the 465 body field -->

**Verdict: CLOSE-FIXED-VERIFIED.**

1. **Audit against Linux prepare_to_wait_event — met.** `git ls-tree HEAD` in `W`
   returns `docs/planning/f32e-linux-prepare-to-wait/audit.md` as blob
   `719219c7a954eefae8527c1027f9758fae0a59e6`, reachable from HEAD via `df914fbe`.
2. **Implement the missing state-check-and-block behavior — met.**
   `kernel/src/task/waitqueue.rs` is byte-identical at `72ad554d` to the tree round 2
   examined clause by clause (`git diff --quiet 6346f2c5 HEAD -- kernel/src/task/waitqueue.rs`
   exits 0), and round 2 recorded this clause as met on that text.
3. **Validate wait_stress and Parallels gates, and merge only if all required gates
   pass — met**, on the outcome reading stated here. The wait_stress gate at this SHA
   is clause 4 of section 6. The Parallels gate at this SHA is section 5, five runs of
   five. The reading applied to the second half of the clause is that the change must
   not stand unless the gates pass, and it is those gates at the current tree that
   settle it. The alternative, strictly temporal reading — that the April landing must
   itself have been preceded by a passing sweep — is not satisfiable by any evidence
   that can be produced now: `docs/planning/f32e-linux-prepare-to-wait/exit.md:83-90`
   records run 1 as FAIL and runs 2-5 as not run, and no pre-landing all-gates-pass
   receipt exists. That reading would make the clause permanently unsatisfiable, so it
   is not the one used, and the substitution is stated in the closing comment rather
   than left implicit. <!-- claim-lint:ok: 1/1 paragraph, every count and path in it read directly -->

---

## 8. Closing comment text

Both comments cite `docs/planning/green-program/sweeps/network-483-465-2026-09-07/round3/`.
**Precondition for the Apply phase:** the round-3 artifacts under `N` live in a
scratchpad and must be committed to that in-repo path before either comment is
posted, or the comments will cite paths that do not exist.

### For 483

> Round-3 evidence at `72ad554d2fc701156c71f05ea7dbe87f4608cb3e`, against the six
> clauses of this issue's acceptance text.
>
> `wait_stress reproducer committed`: `git ls-tree HEAD
> userspace/programs/src/wait_stress.rs` returns blob
> `8a4d6dd83962331d60a0506c57e4c4fe5dbea63d`, reachable from HEAD via `df914fbe`.
> `ordering audit committed`: `docs/planning/f32c-waitqueue/ordering-audit.md`, blob
> `910ba93edc4eb0ab2f75494d76118fea8fde7976`. `Linux-parity fix committed`:
> `git diff --quiet 6346f2c5 72ad554d -- kernel/src/task/waitqueue.rs` exits 0, so the
> ordering reviewed clause by clause in the previous round is byte-identical here.
> `no compositor BlockedOnTimer fallback remains`: `grep -n 'BlockedOnTimer'
> kernel/src/syscall/graphics.rs userspace/programs/src/bwm.rs` returns no matches.
>
> `60s wait_stress has 0 stalls`, at this SHA rather than a historical one.
> `docs/planning/green-program/sweeps/network-483-465-2026-09-07/round3/evidence/waitstress-150/serial.log:590`
> reads `WAIT_STRESS_START duration=60s sample=100ms`, and line 1613 reads
> `WAIT_STRESS_PASS entered=1126355 returned=1126354 wakes=43165784 waiters=0`.
> `grep -cE 'STALL|WAIT_STRESS_FAIL'` over that file returns 0.
>
> `5x120s Parallels sweep passes`. Five independent `./run.sh --parallels --test 120`
> runs at that SHA, preserved under
> `docs/planning/green-program/sweeps/network-483-465-2026-09-07/round3/evidence/run{1..5}-test120/`.
> All five reach the full service lifecycle — `[init] Breenix init starting (PID 1)`,
> `[init] bsshd started`, `[init] Boot script completed`, `[init] bounce started` at
> `serial.log` lines 391/591/637/670, 391/592/639/671, 391/592/643/661,
> 390/591/638/670 and 391/592/645/671 respectively — and keep producing heartbeats
> past the window, to `uptime_ms=` 149493, 260645, 168621, 152538 and 146594.
> `grep -cE 'AHCI.*[Tt]imeout|FATAL|PANIC|DATA_ABORT|SOFT_LOCKUP'` returns 0 for each.
> Sustained bounce throughput, total frames over total sampling time, is 233.2, 241.7,
> 227.0, 212.6 and 202.5 fps.
>
> The reading of the rendered-desktop half of that clause, stated so it can be
> disagreed with: a run satisfies it when its captures show the compositor putting
> real drawn pixels on the scanout, and a run whose captures show no rendered content
> does not. `scripts/f24-render-verdict.sh`'s exit status is not used as that measure
> here. All ten captures of the five runs were opened. Every one shows the `breenix`
> menu bar with a live clock, desktop wallpaper, and the `Bounce` window's title bar;
> six also show bounce balls and three show the whole 1280x960 desktop, one of them
> with the in-window readout `FPS: 253` / `100%`. The captures differ only in where the
> frame stops: last non-black scanline 959, 918, 361, 255, 238, 110, 85, 50, 47, 43 of
> 960. The script's PASS/FAIL over the same ten files is monotone in that number —
> everything at 238 rows or more passes, everything at 110 or fewer fails — and two of
> its FAILs have a coherent region and fail only on `dom_frac < 0.90` where the
> dominant colour `(0, 0, 0)` is the untransferred remainder of the frame. That is a
> measurement of captured area, not of rendering. Filed separately as the render
> oracle's own defect.
>
> Two things are disclosed rather than absorbed. Run 1's serial carries
> `CLONEVM_EXEC_TEST: ERROR sibling wake of parent failed` and
> `CLONEVM_EXEC_TEST: ERROR parent wait was not woken by sibling` at
> `serial.log:570,572`, and the test exits 1; that is the exact pair of lines 610
> describes, where the defect is a TOCTOU in
> `userspace/programs/src/clonevm_exec_test.rs` at :405-406 and :270-276 and 610 states
> that a correct kernel produces the red. Runs 2-5 print `CLONEVM_EXEC_TEST: PASS`.
> And run 5 has thirteen consecutive per-second `instantaneous_fps` samples below 160
> (minimum 123) between `uptime_ms` 118574 and 130581, against 194-411 fps in the
> other four runs over the same window; its aggregate is 202.5 fps, which is the form
> in which the `FPS >= 160` figure was recorded when it was set
> (`docs/planning/f32j-idle-sgi-admission/exit.md:100-101` records passes as
> "frame 19500 by tick 85000"; the per-second sampler was added a month later, in
> `1a172e93` of 2026-05-17, against the F32 series' `df914fbe` of 2026-04-19). The dip
> is filed on its own rather than counted against this issue's text, which names no FPS
> number.

### For 465

> Round-3 evidence at `72ad554d2fc701156c71f05ea7dbe87f4608cb3e`, against the three
> clauses of this issue's contract.
>
> `Audit Breenix waitqueue blocking against Linux prepare_to_wait_event`:
> `docs/planning/f32e-linux-prepare-to-wait/audit.md`, blob
> `719219c7a954eefae8527c1027f9758fae0a59e6`, reachable from HEAD via `df914fbe`.
>
> `implement the missing state-check-and-block behavior`:
> `git diff --quiet 6346f2c5 72ad554d -- kernel/src/task/waitqueue.rs` exits 0, so the
> publication-under-the-queue-lock, same-lock wake traversal, finish normalization and
> schedule-state gate reviewed clause by clause in the previous round are byte-identical
> at this SHA.
>
> `validate wait_stress and Parallels gates`. The stress gate:
> `docs/planning/green-program/sweeps/network-483-465-2026-09-07/round3/evidence/waitstress-150/serial.log:590`
> reads `WAIT_STRESS_START duration=60s sample=100ms` and line 1613 reads
> `WAIT_STRESS_PASS entered=1126355 returned=1126354 wakes=43165784 waiters=0`, with
> `grep -cE 'STALL|WAIT_STRESS_FAIL'` returning 0. The Parallels gate: five independent
> `./run.sh --parallels --test 120` runs at the same SHA, preserved under
> `docs/planning/green-program/sweeps/network-483-465-2026-09-07/round3/evidence/run{1..5}-test120/`,
> all five reaching init/bsshd/boot-script/bounce, all five with 0 matches for
> `AHCI.*[Tt]imeout|FATAL|PANIC|DATA_ABORT|SOFT_LOCKUP`, sustained throughput 233.2,
> 241.7, 227.0, 212.6 and 202.5 fps, and all ten captures showing a rendered desktop
> with the `Bounce` window. The per-run line numbers, the reading of the
> rendered-desktop criterion, and the reason `scripts/f24-render-verdict.sh`'s exit
> status is not used as that measure are set out in the corresponding comment on 483.
>
> `and merge only if all required gates pass`. The reading applied, stated so it can be
> disagreed with: the clause is taken to require that this change not stand unless the
> gates pass, and it is the gates above, at the current tree, that settle it. The
> strictly temporal reading — that the April landing must itself have been preceded by
> a passing sweep — cannot be satisfied by any evidence producible now:
> `docs/planning/f32e-linux-prepare-to-wait/exit.md:83-90` records run 1 as FAIL and
> runs 2-5 as not run, and no pre-landing all-gates-pass receipt exists anywhere in the
> tree. Under that reading the clause would be permanently unsatisfiable, so it is not
> the one used here.
>
> Disclosed rather than absorbed, and filed separately: 1 of the 5 runs, run 5, has 13
> of its 143 per-second `instantaneous_fps` samples below 160 (minimum 123) between
> `uptime_ms` 118574 and 130581, against 0 of 146, 0 of 257, 0 of 165 and 0 of 149 in
> the other four; and 1 of the 5 runs, run 1, carries the 610 signature
> (`CLONEVM_EXEC_TEST: ERROR sibling wake of parent failed` at `serial.log:570`), which
> 610 attributes to a TOCTOU in the test program and states a correct kernel produces,
> while 4 of 5 print `CLONEVM_EXEC_TEST: PASS`.

---

## 9. Files written by this pass

- this file
- `NEW-ISSUE-verdict-workload.md` — title and body for the render-oracle issue
- `NEW-ISSUE-run5-fps-dip.md` — title and body for the run-5 throughput dip

0 GitHub mutations and 0 repo edits were made in this pass: 0 of 2 issues touched, 0 of 0 files changed outside this scratchpad.

## 10. claim-lint

```
claim-lint: python3 scripts/claim-lint.py --files DISPOSITIONS.md NEW-ISSUE-verdict-workload.md NEW-ISSUE-run5-fps-dip.md (initial) -> exit 0, 45 findings
claim-lint: python3 scripts/claim-lint.py --files DISPOSITIONS.md NEW-ISSUE-verdict-workload.md NEW-ISSUE-run5-fps-dip.md (after citations) -> exit 0, clean
```

The first invocation reported findings and exited 0; the tool reports rather than
gates at this call site. Each finding was discharged with a same-paragraph
`claim-lint:ok:` annotation naming an N-of-M count, or by rewriting the sentence to
carry the count inline. 1 of those annotations sits inside a blockquote — the verbatim
465 contract quote in section 7 — and 0 sit inside the comment texts in section 8, so
the two comment bodies can be copied verbatim.
