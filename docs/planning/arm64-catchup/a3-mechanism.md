# A.3 recovery mechanism analysis — a3-u10-mechanism

## Measurement 20261111T190334Z-qemu-43440: font read interval remains unexplained

At `3d9201e8`, the normal ARM build and disk staging preceded this single
headless HVF SPICE GL measurement. The disk suite and build both hashed
`c3d164308ba510f20f07c763b2c17f6d29112c792e1306b8a759d6ab6a45175d`;
the client copies both hashed `d978abff9335fe68e44a3031f00db3eb835f446a968a8274a56bc437e042dc0c`.
The boot kernel hashed `bf2c5a822cc849e57052e9a98ce4d97982be4fcb5cc8a826c393a23f812e702c`
and the disk `feac6807e0e4838e2b86aa5f9b800b9268b8d7b130fb9127dcf77bb64e1dc610`.
The required labels were present. The serial SHA-256 is
`e70921b7b800acb04be7e41c908115cd0499cccfb2ebca48a12a1f2a23e576d5`;
the two-second load sample file beside the Vigil capture hashes
`82b773bff23ce1e456e759485a0cc5a7996fcd2d2f9bb2066a08fb732cdb2fe1`.
The sampler's recorded PID was stopped after the launcher exited. Host load
started at 6.38 and ranged 4.00–8.67. Serial receipt near the exported failure
corresponded to 5.53, with a two-second sampling interval and delayed export;
that is not a precise load measurement at the guest midpoint.

The run completed 80/81, with reclaimed memory PASS (10 of 48 pages), READY
after DONE and MEMORY, and accepted ANGLE/Metal (994 records). The sole failure
was supervisor-restarts. Client 83 commit 4 waited 2005.676292 ms against
2000 ms, margin **-5.676292 ms**. The replacement PID 781 first composed at
280520028541 ns, 144.464083 ms after the observer had timed out. At the
midpoint snapshot (279432301125 ns), PID 781 TID 1014 was **running** in GPU
syscall 416, not recorded as queued; PID 781 TID 1016 was blocked in read
syscall 63 on waitqueue 18446462599822208016 for 26.713 ms. The observer was
in its timer sleep, due at 279436633625 ns. The snapshot does not identify the
read's file or the condition and producer responsible for waking that queue,
nor the GPU operation's remaining work. It cannot establish scheduler starvation
or a causal blocked-wait mechanism.

The startup stamps show font-config-ready at 278372079041 ns and
font-file-ready at 280293164166 ns: **1921.085125 ms** between them. The code
between those marks prints the configuration and calls `std::fs::read` on the
display font (`userspace/programs/src/bwm.rs:163–168`); the main thread joins
the worker before proceeding (`bwm.rs:229–231`). These measurements identify a
critical interval, not which operation or wait caused it. The whole frame check
then took 139.827 ms from its first band to return. First composite followed at
2159.974 ms after exec; this is a startup phase measurement, not the observer
wait duration. A causal forcing check and repair would be speculative.

Restart-adopts passed: its reap was 21.523333 ms against 500 ms, margin
478.476667 ms. This fast reap cannot classify the prior 893 ms pre-exit delay.
Stack-restored passed; one positive-click wait began at 266502958958 ns and
observed its event at 267266859208 ns, 763.900250 ms against 1000 ms, margin
236.099750 ms. Its midpoint snapshot and delivery/handling records survive,
but this pass cannot classify the earlier native failure. Frame-floor passed at
493 Hz against 60 Hz; endpoint gaps were 0.916916 and 1.937416 ms against
16.666667 ms, margins 15.749751 and 14.729251 ms. This does not classify the
earlier frame-floor failure. No outer watchdog failed in this measurement.

The bounded dump retained 512 rows and overwrote 421, but includes the failed
supervisor snapshot and click records; the measurement is not void. The
remaining question is what the font worker's read and its waitqueue were
waiting for, what should wake them, and whether that condition accounts for the
observed 1921 ms interval. The earlier slow reap and native stack-restored
mechanisms remain unclassified. Under coordinator line 190, stop here: no
forcing check, behavior fix, proof boot, R240 boot or board update is justified.

## Measurement 20261111T173538Z-qemu-95695: stale suite artifact

The single measurement launch recorded clean checkout `aef5df85`, HVF and
headless SPICE GL. It finished 79/81: restart-adopts and supervisor-restarts
failed; stack-restored and frame-floor passed. Memory reclaimed 10 pages against
the 48-page allowance, READY followed DONE and MEMORY, and the host renderer
accepted ANGLE/Metal (982 records). Start uptime loads were 8.32 / 13.91 / 17.04;
end loads were 7.35 / 8.72 / 12.96. No lane build, installer or heavy work ran
during the boot. The raw Vigil serial SHA-256 is
`cf85a48bc28289b3e26670768c4a769a4a12d162c5ce03240d638e5cd0518157`.

The checkout commit in the Vigil record does not identify the guest binaries.
The launcher documents that `--no-build` boots the existing target disk. The
capture emits `wm-reap ... failed_ns=...` and `wm-reap-status`, matching the
failure-only code at `b26abb03`, whereas current `Child::stop` emits
`wm-reap-wait` and unconditional `reap-elapsed`. The four new observer,
readback, reap-wait and frame-endpoint export families are absent. Thus this boot used a
stale suite artifact and cannot answer the instrumentation question.
<!-- claim-lint:ok: the retained serial SHA-256 cf85a48bc28289b3e26670768c4a769a4a12d162c5ce03240d638e5cd0518157 above identifies this negative search for the four export families. -->

The old diagnostic measures PID 660's kill return at 245388556875 ns and
timeout at 245891152708 ns: 502.596 ms, 2.596 ms beyond the 500 ms deadline.
The leader's stage 17 occurs 893.112 ms after kill return and stage 20 at
893.139 ms, both after the timeout. This locates the late exit but does not
explain the delay before it. The required 125 ms positive margin is absent.

Supervisor replacement PID 720 reaches context-ready at 1152.442 ms,
fonts-ready at 2094.711 ms and frame-pixels-ready at 2156.256 ms after exec;
the case times out at 2000 ms with EAGAIN reason 0. Exec-relative stamps are
not the observer wait interval, and the refusal branch and completion receipts
are unavailable. The 500 ms required headroom is not established. The native
stack-restored EAGAIN signature remains ambiguous for the same reasons stated
below; this ARM pass does not identify its native cause.

Frame-floor reports 2444 intervals over 4988.764041 ms, or 489 Hz, within a
5000.011583 ms measurement window. The combined unobserved endpoints are
11.247542 ms; individual client endpoint gaps and their 25% headroom cannot be
calculated from this stale producer. Passing the case does not identify the mechanism.

No failure mechanism is established. Before any further measurement, stage and
verify the instrumented suite and kernel artifacts against the intended head;
the unanswered question is which refusal branch and completion state actually
occur during the observer wait, alongside the reap and endpoint intervals.
Coordinator line 176 requires stopping here, without a speculative fix or
another boot.

This is an analysis unit under coordinator ledger line 167, superseding line 154(4).
The input head is c81001cd; runtime evidence ends at 403f5501. No boot, deployment,
installer, board change, PR creation or merge was performed in this unit.

The captures do not establish one common root cause. In particular,
`last completed=None` means **this wait never obtained a valid observation**;
it does not mean the compositor never completed a frame. The native failing
replacement has a first-composite timestamp at 270.700 ms (E37:L3577), and the
failing ARM replacement at 5fa5325c has one at 2139.174 ms (E34:L1930).
Conversely the latest ARM replacement has no checks-ready or client-compose-begin
stamp in its exported timeline (E39:L2064–2099). Treating these as the same
unfinished startup is contradicted by those records. <!-- claim-lint:ok: Cited code conditions and raw records indexed in docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt; absence is scoped to the hashed capture inputs, not runtime progress. -->

## What EAGAIN / reason 0 establishes

The suite process, **not bwm**, receives the SCANOUT_READBACK error in these
failure messages. `Screen::read` allocates a fresh zeroed readback argument and
calls window op 22 (`userspace/programs/src/wm_suite/desktop.rs:362–380`).
Bwm's startup pixel check instead calls GPU READBACK on target 0, window CAPTURE
for client pixels, and desktop-target READBACK; it does not call
SCANOUT_READBACK (`userspace/programs/src/bwm/compositor.rs:634–706`). This
separates the observing process from the producing replacement.

The reachable EAGAIN paths are:

| Path | Condition and meaning | Source |
| --- | --- | --- |
| Window readback gate, busy | `reg.present.busy == true`: scene submission has reserved the producer slot; observer returns before pinning scanout or calling GPU readback. The argument stays zero, including reason. | `kernel/src/syscall/window.rs:273–280`; reservation at `kernel/src/window/scene.rs:519–548`, release at `kernel/src/window/scene.rs:422–430`  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt. --> |
| Window readback gate, no admitted composite since claim | Current `present.serial == claim.first_serial`; there is a new claim but no later Admission::begin. It is independent of whether raw startup PRESENTs have completed. | `kernel/src/syscall/window.rs:277`; claim baseline `kernel/src/window/ops.rs:1306–1312`; serial increment `kernel/src/window/scene.rs:401–420` |
| GPU fallback observation context creation, context quota | If the scanout's last writer context is no longer live, observe can acquire the kernel screen context. If that screen target must be recreated, context creation can return EAGAIN when process 0 has reached the context quota. The error is propagated as an errno with reason NONE. This is reachable in code, **not established in any capture**. | `kernel/src/gpu/mod.rs:6505–6537`, `kernel/src/gpu/screen.rs:450–461`, `kernel/src/gpu/screen.rs:230–236`, `kernel/src/gpu/screen.rs:51–56`, `kernel/src/gpu/mod.rs:3400–3419,3481–3485`  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt. --> |

For the GPU path, `observe_scanout` maps an errno refusal to reason NONE
(`kernel/src/gpu/mod.rs:5806–5811`), and the window wrapper writes a reason
only for EIO (`kernel/src/syscall/window.rs:282–295`). Thus reason 0 is not a
GPU device-status diagnosis. The two quota EAGAIN sites for ordinary resource
creation and the framebuffer/window-target constructors are not calls made by
this readback: readback staging uses Pages plus Charge, which returns ENOMEM,
EINVAL or ENODEV (`kernel/src/gpu/staging.rs:18–24`,
`kernel/src/gpu/quota.rs:86–97`; unrelated constructors at
`kernel/src/gpu/mod.rs:3874–3878`, `kernel/src/gpu/framebuffer.rs:19–29`,
`kernel/src/gpu/window_targets.rs:133–142`). Display-transition waits map to
EINTR/ETIMEDOUT, and an unknown binding is EIO/reason 10, not EAGAIN
(`kernel/src/gpu/display.rs:659–685,706–718`,
`kernel/src/gpu/mod.rs:6005–6028`). Device observation and transfer failures
flow through `device_reason`; the checked VirGL/SVGA/native adapter source has
no EAGAIN producer on this transfer path (`kernel/src/gpu/mod.rs:6589–6659,6780–6785`). <!-- claim-lint:ok: Cited code conditions and raw records indexed in docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt; absence is scoped to the hashed capture inputs, not runtime progress. -->

The captures have no per-refusal snapshot of `busy`, claimant identity,
`first_serial`, `present.serial`, or fallback-context creation. Therefore
selecting one of these paths **for each observed refusal** is impossible from
these records. Claiming that the GPU continually refused a readback would be
incorrect when the early window gate is responsible: it makes no GPU call.

## Producer state, ordering and the native comparison

A compositor claim is made before startup chrome and full-frame validation.
Bwm warms fonts while claiming/allocating targets, joins its font worker, paints
chrome, validates fill samples, discovers windows, and overlaps title coverage
with its full-frame pixel check. Only after those workers return does it stamp
checks-ready and enter the event loop that submits client composites
(`userspace/programs/src/bwm.rs:154–231,272–385`,
`userspace/programs/src/bwm/startup_prepare.rs:13–21`,
`userspace/programs/src/bwm/compositor.rs:418–425`). Startup raw PRESENT is not a
window-composition receipt (`userspace/programs/src/bwm.rs:266–270`). A new
claim whose startup validation is still running therefore legitimately leaves
the suite's observation gate at its no-admitted-composite condition.

At 403f5501, replacement 736 has claimed epoch 133 and printed its hotkey setup
(E39:L2111–2115). The timeline reaches initial-windows-ready at 2015.147 ms,
title-plan-return at 2015.263 ms, and frame-pixels-ready at 2057.163 ms; it has
not exported checks-ready, desktop-ready, client-chrome-ready,
client-compose-begin or first-composite (E39:L2064–2099). The latest known
state is **startup frame validation at least one band through**, not a proven
blocked COMPOSITE. With CHECK_BAND_BYTES=4194304 and width 2560, each band is
409 rows; a 1600-row frame takes four bands. The startup stamp uses a
compare-exchange and records only the first occurrence, so frame-scanout-return,
frame-desktop-return and frame-pixels-ready cannot identify the final band or
prove that the frame check returned (`userspace/programs/src/bwm/compositor.rs:32,664–670,677–746`;
`libs/libuitest/src/startup.rs:50–57`). Output/reporting and worker joining also
lie before checks-ready (`userspace/programs/src/bwm/compositor.rs:595–609`,
`userspace/programs/src/bwm/chrome.rs:22–35`,
`userspace/programs/src/bwm/startup_prepare.rs:17–20`). Which operation was still
running at the deadline is **unknown**. A claimed font-submit or reclaim cause
would exceed the evidence. <!-- claim-lint:ok: Cited code conditions and raw records indexed in docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt; absence is scoped to the hashed capture inputs, not runtime progress. -->

At 5fa5325c, replacement 665 does reach checks-ready at 2098.373 ms and first
composite at 2139.174 ms (E34:L1912,L1930). This rules out a permanently stuck
producer in that failure. At 7dc1a45c, replacement 731 reaches checks-ready at
1287.849 ms and first composite at 1311.134 ms (E38:L2011,L2031). The newer
403f5501 replacement has slower startup before any client COMPOSITE; its font
planning interval is 895.329 ms versus 425.361 ms, and submit interval is
674.799 ms versus 38.735 ms. These are differences in measured intervals, not
identified causes (E39:L2086–2090; E38:L2012–2016).

The native stack-restored replacement 857 is different: checks-ready 237.484 ms,
client-compose-begin 245.928 ms, first-composite 270.700 ms
(E37:L3559,L3575,L3577), yet the observer reports EAGAIN/None
(E37:L3578,L3600). Its presentation trace progresses from seq 4 at
152181551943 ns to seq 236 at 156063500370 ns (E37:L3593,L3596).
Consequently **“no composite ever completes” is false here**. The same errno
and local observer state are a shared symptom, not evidence of the same
unfinished-startup mechanism as 403f5501. Polling only during busy reservations
can starve observation despite completed frames. The guard in this unit forces
that schedule, but the capture lacks gate snapshots to prove that it was the
native run's schedule. Fallback-context quota exhaustion is not demonstrated
either. The common causal mechanism remains unproven. <!-- claim-lint:ok: Cited code conditions and raw records indexed in docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt; absence is scoped to the hashed capture inputs, not runtime progress. -->

The ordering that decides an individual observation is explicit: sample the
registry while it is neither busy nor still at the claim baseline, then complete
pinned GPU readback and the suite's v2 validation, and find the requested client
commit (`kernel/src/syscall/window.rs:273–295`,
`userspace/programs/src/wm_suite/desktop.rs:380–388,625–652`). The wait retries
with a 5 ms sleep, and fails on its deadline or the case's remaining-time guard
(`userspace/programs/src/wm_suite/desktop.rs:14–41`). `WaitObservation.record`
is assigned only on a successful, validated Screen::read; errors merely retain
the latest error (`userspace/programs/src/wm_suite/desktop.rs:487–510`). None
cannot distinguish no production, busy-poll starvation, or repeated invalid
observations. The historical invalid-scanout failures predate the early gate
added by 02c3ccef; they cannot be retroactively labelled gate EAGAIN failures
(E01–E16 case table; `git blame` at `kernel/src/syscall/window.rs:273–280`). <!-- claim-lint:ok: Cited code conditions and raw records indexed in docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt; absence is scoped to the hashed capture inputs, not runtime progress. -->

## Reaping, classifications and margins

Restart-adopts freezes the surviving client, stops the old compositor, then
starts its replacement (`userspace/programs/src/wm_suite/cases.rs:1070–1077`).
The 500 ms stop interval begins **after SIGKILL returns**, not at the kill request
(`userspace/programs/src/wm_suite/desktop.rs:87–96`). Both exported reap-timeout
failures precede replacement creation: c1a8e250 (E35:L1862) and 7dc1a45c
(E38:L1820). Neither has exit stages. Earlier failures include replacement
startup timeout or exit(1), rather than the same 500 ms failure (case table).
Those should not be aggregated into a single reaping cause.

The expected stage comparison at 403f5501 is unavailable. Its retained passing
reap segment is E39:L1855–1876: PID 661's kill log, census, replacement startup,
and the 6217 ms **case total**, without monotonic exit stages. Stages 17–22 are
stored in the guest's bounded pipeline ring (`kernel/src/task/process_task.rs:1009,1021,1042,1115,1198,1203`;
`kernel/src/window/pipeline.rs:12–45`), but stop reads/exports that ring only
inside its error handler (`userspace/programs/src/wm_suite/desktop.rs:97–116`).
The success path does not export kill request, kill return or reap completion.
No stage can be assigned the earlier timeout from those captures. The stage
17 entry would also omit work before handle_thread_exit, and stage 22 is not
itself a parent wait-return timestamp. No causal credit is assigned to 403f5501
from one passing reap.

| Case / observed failure family | Classification supported by code and captures | Passing margin in ms |
| --- | --- | --- |
| supervisor-restarts, late startup on ARM | Slow critical path observed; **deterministic slow path versus scheduling race is unresolved**. 5fa5325c completes at 2139.174 ms, 7dc1a45c at 1311.134 ms, 403f5501 is still before checks-ready after 2057.163 ms. No fixed-cost attribution exists. | E38's exec-to-first-composite proxy leaves **688.866** ms against 2000. E17 (095252) and E33 (132259) pass but have no phase stamps, so their margin is **unknown**. Exact wait-start-to-success margin is unknown in all three.  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E38, E17, E33. --> |
| supervisor-restarts / stack-restored, busy-readback admission | **Race mechanism is demonstrated in code by forced interleaving**, not attributed conclusively to each capture. Same producer progress, different poll placement, different observer outcome. Native stack-restored already completed at 270.700 ms yet its wait never recorded a frame. | Native supervisor E37 has an exec-to-first-composite proxy margin **1799.246** ms (replacement 879 at 200.754 ms, E37:L3744); this is not its wait margin and not a passing stack-restored result.  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E37. --> |
| restart-adopts, 500 ms reap | **Unknown: race versus deterministic slow stage cannot be decided.** No kill/wait/stage durations are exported for the failure pair or the passing latest reap. Address-space-sized synchronous work was removed by 403f5501, but that ordering proof does not identify these captures' cause (`docs/planning/arm64-catchup/a3-reap.md:17–44`). | **Unknown for the retained passes.** 6217 ms at E39:L1876 and the other passing case ms cannot be subtracted from 500.  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E39. --> |
| restart-adopts replacement startup / exit(1), and stack-restored positive-click failure at 71c03a24 | Different failure sites; **unclassified here**, not evidence for the reap mechanism. | Inner wait measurements absent; see the exact CASE messages and diagnostics below. |

Even the proxies above are not the coordinator's required measured 25% margin:
the supervisor case first waits for a different child PID, then parks the pointer
and asks the client for a fresh barrier commit, then starts shown_after's wait
(`userspace/programs/src/wm_suite/cases.rs:39–42,1119–1139`). Exec is not that
wait's begin, and first-composite may name an earlier client commit. The two
2000 ms waits (child replacement and client observation) are distinct. Moreover
until_from evaluates its predicate before checking time; a successful poll can
return after the deadline (`userspace/programs/src/wm_suite/desktop.rs:32–40`).
No bounded exact duration is inferred from PASS alone.

## Check, unresolved question and handoff

Check-first commit cdd94ea0 adds `tests/a3_readback_interleaving_test.py`.
It extracts the production gate and forces 400 busy polls interleaved with
producer serial advancement, then admits the same state in a quiet interval.
It also checks the pre-first-composite refusal. It compiles with `-Dwarnings`
and passes on the input runtime: a **U29 regression guard**, not an old-red/new-green
repair and not a capture-cause proof. The initial fixture compile failed because
its Held type lacked production Claim's Copy trait; correcting that fixture
produced the passing run. No production code or deadline changed. Only this
host check and document/evidence checks are in scope; kernel/userspace release
artifacts are unchanged and were not rebuilt.

The analysis remains blocked on these exact questions, to be instrumented
before any later authorized boot:

- At each observer refusal, was the gate busy, at claim baseline, or did GPU
  fallback context creation return EAGAIN? Record claimant, serial/baseline,
  branch, and monotonic time in a bounded always-on buffer, exported on failure.
  Correlate its tail with completed composition receipts and the wait boundaries.
- For replacement startup, did the later readback bands, post-check reporting,
  or worker join consume the interval after the first band? Record each band's
  start/end and a whole-check-return plus worker-join stamp. The existing first-only
  marks cannot answer this.
- For restart-adopts, what are kill request, kill return, stages 17–22 **for the
  stopped process/thread group**, and parent wait completion? Export them on a
  pass as well as a failure, before the ring is overwritten. This supplies the
  missing stage attribution and actual 500 ms margin.

No new runtime instrumentation was added in this analysis unit. No further boot
is authorized by this report; ledger line 167(3–4) still requires a supported
mechanism and a deterministic failing check for its repair. A later instrumented
window-manager boot must answer the questions above in the same capture, not
provide another recurrence sample. Issue #2872 remains the recovery blocker.
The capture audit passed: 39 run identities/hashes, 88 CASE records, 650 stamps,
and 1308 decoded excerpt lines matched the retained raw captures. The latest
launcher and Vigil serials are byte-identical. `python3 scripts/claim-lint.py`
exits 0 (88 files checked; 267 existing findings outside changed hunks).
The explicit whole-file report/test lint also exits 0; `git diff --check` is clean.
These are document and host checks, not suite acceptance.

## Capture inventory

Scope: the retained November 11 window-manager run records: ARM QEMU plus the named native shared-path run. Partial and missing-serial records are included. Nov 10 native runs belong to other branches and are outside this comparison. Each E-key names `<boot-id>/serial.txt` in the Vigil boot store; `L` is a physical LF line number, matching `rg -n`. The companion [evidence index](serials/a3-mechanism-captures.txt) retains selected records with their original line numbers; each line payload is a JSON string so trailing spaces survive without repository whitespace errors. Decode the payload to compare it with the raw capture. `commit:L1` refers to that boot folder's commit file, not a phase timestamp. A missing CASE is **not recorded**, never an inferred FAIL or PASS. Case ms includes setup and cleanup, not just its inner deadline. <!-- claim-lint:ok: Cited code conditions and raw records indexed in docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt; absence is scoped to the hashed capture inputs, not runtime progress. -->

| Key / boot id | Commit (commit:L1) | Selection (only-case) / dirty | Serial SHA-256 |
| --- | --- | --- | --- |
| E01 `20261111T054423Z-qemu-34249` | `46c2abd341942f6d1ed6835724ecafb90d771de2` | `full` / 0 | `c75a969cb0465a8e31182b32e875e4eb5294810152a149d4a6ec5148db4597b1` |
| E02 `20261111T065657Z-qemu-97132` | `09d8846d6e44c550ac603ffee978e189e740431c` | `full` / 0 | `cc0ef6c70a102a6b1dc13c93b37ce787ccfe1fed530b63c5fd8a45ac07ca12ca` |
| E03 `20261111T071913Z-qemu-52040` | `47d581a8c3fc155650ed63e42ca78b746adc9416` | `full` / 0 | `7bc7232d4a392f066f28eb18d8ba1649cd1d4f451df40d9c322b35e44f7628ba` |
| E04 `20261111T073308Z-qemu-79759` | `f06e3705087eec8dd889e65c8376f3f010f00452` | `full` / 0 | `8430943afe4dd71d7c7eaa6391484ab080d7e2bd9409a3890fefb42d76973347` |
| E05 `20261111T074925Z-qemu-22275` | `47beead010178cb71cb99de7772110e82dfe08cd` | `full` / 0 | `5ccf2af6a94aba12ab22708091c0e031d90a7d9db914393f507ef324d7ed5f48` |
| E06 `20261111T080136Z-qemu-47569` | `ec776ca09699093241ec6e42cca4a8b8c4327db8` | `full` / 0 | `614c5711fd32886d2fd0e3fa5da47460b7239911c8182a232a63ac8a904ba393` |
| E07 `20261111T081601Z-qemu-a3-recovery` | `93b294f4331e172d930ff9138069397259ea000c` | `recovery/supervisor-restarts` / 0 | `absent` |
| E08 `20261111T081707Z-qemu-a3-recovery` | `93b294f4331e172d930ff9138069397259ea000c` | `recovery/supervisor-restarts` / 0 | `14c9e81b850c9279b97f2facbfde3ff7f6bdb0e720d94a39382278eb51bcac6e` |
| E09 `20261111T082030Z-qemu-a3-recovery` | `f3c9044938b693135007983a1810f940e5ef743d` | `recovery/supervisor-restarts` / 0 | `448cbe335fe2c4e64d4e06edece41c0f7605f625b4317e9879a773d0c30c5508` |
| E10 `20261111T083128Z-qemu-a3-recovery` | `77b3d0145cb94f372e7dd9f5b14f5ca08f993dbc` | `recovery/supervisor-restarts` / 0 | `f3df0aa91772c3751cdb3d54dda87bc2f90cf1e2a1c75a699d419001688775aa` |
| E11 `20261111T083348Z-qemu-a3-recovery` | `77b3d0145cb94f372e7dd9f5b14f5ca08f993dbc` | `recovery/supervisor-restarts` / 0 | `d79534c5d8fbbd2afdfa0c8f96f63cdd1c42e8fdded32e23392eae5b874cc036` |
| E12 `20261111T084203Z-qemu-29873` | `d8c21fda79b301341480baa11c17ef9dc4ea2e56` | `full` / 0 | `d7017ed06dbd4bc406b29e841e9082797b2ee783fa7ac53fa5f4f7435f23fd15` |
| E13 `20261111T085633Z-qemu-a3-recovery` | `df4d2d1d86294999fb364a13235c9b230777a1c1` | `recovery/supervisor-restarts` / 0 | `1ae304fcbb2a5e075ecca4d06a990568c8824309b1fe5e75c882b6257b7b7b82` |
| E14 `20261111T090051Z-qemu-a3-recovery` | `c71a0144ba02a83eebe6dbbd5a2bec3aba52e2dc` | `recovery/supervisor-restarts` / 0 | `9eda2daa08b55e64ad68ce507eb3b42350b6b20bcf1ecd5e7c00329bb88d819f` |
| E15 `20261111T091646Z-qemu-4047` | `737c0717fa378b680e55ece07abcaf46d6b59070` | `full` / 0 | `2c1247e5de2b73cce183dfa09088e57e09ea5d296092b6840052f9da2b90b778` |
| E16 `20261111T093101Z-qemu-31845` | `49c298c84ebffe83d183c1fd9cb590edf6bc7fae` | `full` / 0 | `f49b2c508c7b663796a8d7936a12109d86f8fbc1b9d68eac962de0fa9635a7b5` |
| E17 `20261111T095252Z-qemu-69448` | `c3bcb8e6f507a850aecf982469106a399ea2ca81` | `full` / 0 | `cec36691ab84be78f9d28ffda79bca32bd4c503bda8730285cedd8a62a2f899b` |
| E18 `20261111T100510Z-qemu-a3-startup` | `7de65d2f45ea988a126020b2d9c1ce829eedfa2c` | `startup/supervisor-starts` / 0 | `173dcf64999057392542d266d2e9bc853e500f42d7bee6c20dc7c8539800c07b` |
| E19 `20261111T100557Z-qemu-3669` | `7de65d2f45ea988a126020b2d9c1ce829eedfa2c` | `full` / 0 | `4a0433a62bb4deb08b7184de1f05a33cfc7abd42cc14c20af775cbffac7c7a75` |
| E20 `20261111T101807Z-qemu-28726` | `c81ab1516aea55e5d2a4210b3f74144afaf5539e` | `full` / 0 | `d78c604a88d090c9e1d7d18f974f8a859df57779221d37beaec43beeea84efe0` |
| E21 `20261111T103525Z-qemu-60402` | `4951dcdb57203ec7b68c7638931337a37b395ac9` | `full` / 0 | `33c8eac53ec5b353dc2d64528342ea2e6dce0c3a3992f9d38359ea90a16a263d` |
| E22 `20261111T104731Z-qemu-91576` | `93fbd4891b7e229f618a241b79a7a7f47e8daf35` | `full` / 0 | `1c1cb01e921756d19047f38542fb8d2cae2500dd9049a4453aa1e63caf902814` |
| E23 `20261111T111827Z-qemu-a3-recovery-copy` | `46c379bd1165b2f73bc7c5a7c70cf92e28107e52` | `recovery` / 0 | `3e4df83b0b0f51aef17971aab326474468ea3f7dd52867d46f31f126c9603d11` |
| E24 `20261111T111844Z-qemu-a3-recovery-copy` | `46c379bd1165b2f73bc7c5a7c70cf92e28107e52` | `recovery` / 0 | `6fcf56d2637922433d05543c4eb3011f7d3af1fead770f61cebee0d8753104a9` |
| E25 `20261111T112523Z-qemu-99765` | `a1556efff3baea9e79740c059cf2db8e5b8a33e1` | `full` / 0 | `00fa0a80d20050c861dff409c7686ffb91e00752ebf355e873e95f0c991c4de7` |
| E26 `20261111T113614Z-qemu-a3-reset` | `41af61893ba04107ad0b35a57964f58d4e5f5107` | `recovery/restart-adopts` / 0 | `a7202a5c89a2dcc6aea12dd686c30cbd0257e068f8ece6b2ae69f35f0968497d` |
| E27 `20261111T113747Z-qemu-a3-reset-cat` | `41af61893ba04107ad0b35a57964f58d4e5f5107` | `recovery` / 0 | `c99bfbc84c7c81d4b2965dbd0b4a3636fd7e2454f394fd0b503057da9ecec56d` |
| E28 `20261111T114527Z-qemu-34390` | `226bc9783e1458b17c4315149dbf20314beb3bf6` | `full` / 0 | `d56bcbaf2d49967558773a06ea11580ccdfd5da9172a26b4423622fd95cc4015` |
| E29 `20261111T120252Z-qemu-a3-phases` | `075e6d129be0204ef16946937157583116b6c90d` | `recovery/supervisor-restarts` / 0 | `359b5c427644cc77d63f086fb038e78f81eb762e4132055e7a4d342abf6973d5` |
| E30 `20261111T120852Z-qemu-a3-phase-env` | `3c6aad9708239edcc9ed1ecab405283f57f02fea` | `recovery/supervisor-restarts` / 0 | `478b569095621f56f7347cb7e5ce0828370ef514861d8a0d89696f5f395ac49d` |
| E31 `20261111T121843Z-qemu-5644` | `ed618ce3eb5b096615db8e093d16a6953783b5f7` | `full` / 0 | `83a72a3029bd5ccdf510f6bd6d2b8d942935f5fcbdc0aee1c7f19b652cf92713` |
| E32 `20261111T124428Z-qemu-52293` | `fce39d83ade0fe12026cba2d4e06314b4faa7c1a` | `full` / 0 | `2cf29df2816856726688ab02051168cfabf99232d0a17b6950d99017ffb42515` |
| E33 `20261111T132259Z-qemu-76612` | `8dfb9b2c5681aa43ae5be226b3d739bd04b77600` | `full` / 0 | `2149cb185a2caac138385608dec5008dc645b28f8f1d33bc2597f8f094d2bed7` |
| E34 `20261111T134512Z-qemu-29569` | `5fa5325cd40d5a2c9d16667a4e840f09daf1facd` | `full` / 0 | `a417ff6fb4baa23337090a041049e3cd87a13e4e8ab133498bf16131060aacab` |
| E35 `20261111T142945Z-qemu-31433` | `c1a8e2506071c6f6a8a194f0ebad329a34ad1e79` | `full` / 0 | `731c25ffd8d14a77bccb82a8bd4cc015c1f21ccf2d0fd82a492e79a3f6402d58` |
| E36 `20261111T145221Z-qemu-11150` | `71c03a245b40c0babb2cb14b40804cf42ceaeb24` | `full` / 0 | `cde0420037411188d49fc048a71ae30bce0ff4ab0ef60344643c594a4f00f9d4` |
| E37 `20261111T150757Z-metal` | `71c03a245b40c0babb2cb14b40804cf42ceaeb24` | `full` / 0 | `b3013e1265a24119ee0901047866c88203707927165165e5f1c29340b7932849` |
| E38 `20261111T154755Z-qemu-92251` | `7dc1a45c5110d1dc82436ce34dcd605d64fdfbc0` | `full` / 0 | `dfe2d335a9e5351ca8ba57e20b8ba812c8a8f08919315a3a8cd7a7827be5feb5` |
| E39 `20261111T161134Z-qemu-52524` | `403f55016063bb8b961016dcbe847c29589bc040` | `full` / 0 | `9320a5352d5fd28792fa768df7e9bd28d528fdac51abf96986bb49acddcc3330` |

## Case results and readback state

The diagnostic column preserves the CASE message, including truncation. The follow-up table supplies longer exported observation fields when available. Blank diagnostics on PASS mean the producer did not export a wait measurement.

| Capture | supervisor-restarts (ms; message) | restart-adopts (ms; message) | stack-restored (ms; message) |
| --- | --- | --- | --- |
| E01 | FAIL 9209 ms [E01:L1743] — bwm composite of client 83 commit 4: no acknowledgment within 2000 ms; [init] Breenix init starting (PID 345) [init] not PID 1: test-control check skipped [init | FAIL 6388 ms [E01:L1619] — bwm first completed desktop: no acknowledgment within 4000 ms; [bwm] settings font.size=14 clock.visible=true [bwm] Breenix Window Manager starting... (v2-chrom | PASS 8605 ms [E01:L1690] |
| E02 | FAIL 4400 ms [E02:L1731] — bwm first completed desktop: no acknowledgment within 4000 ms; previous context=0; last readback error=Some("scanout lacks its complete v2 dimensions, binding, | PASS 6813 ms [E02:L1617] | PASS 8791 ms [E02:L1694] |
| E03 | FAIL 4203 ms [E03:L1747] — bwm first completed desktop: no acknowledgment within 4000 ms; previous context=0; last readback error=Some("invalid scanout: backend=4 flags=1 context=357 seri | PASS 7009 ms [E03:L1620] | PASS 7737 ms [E03:L1692] |
| E04 | FAIL 9392 ms [E04:L1786] — bwm composite of client 83 commit 5: no acknowledgment within 2000 ms; last readback error=Some("invalid scanout: backend=4 flags=1 context=361 serial=0 size=25 | PASS 6607 ms [E04:L1635] | PASS 8387 ms [E04:L1709] |
| E05 | FAIL 9124 ms [E05:L1773] — bwm composite of client 83 commit 5: no acknowledgment within 2000 ms; last readback error=Some("invalid scanout: backend=4 flags=1 context=363 serial=0 size=25 | PASS 6763 ms [E05:L1630] | PASS 8796 ms [E05:L1698] |
| E06 | FAIL 9115 ms [E06:L1770] — bwm composite of client 83 commit 5: no acknowledgment within 2000 ms; last readback error=Some("invalid scanout: backend=4 flags=1 context=361 serial=0 size=25 | PASS 7327 ms [E06:L1624] | PASS 7711 ms [E06:L1694] |
| E07 | not recorded | not recorded | not recorded |
| E08 | not recorded | not recorded | not recorded |
| E09 | FAIL 9507 ms [E09:L361] — bwm composite of client 2 commit 4: no acknowledgment within 2000 ms; last readback error=Some("invalid scanout: backend=4 flags=1 context=10 serial=0 size=2560 | not recorded | not recorded |
| E10 | FAIL 9233 ms [E10:L374] — bwm composite of client 2 commit 5: no acknowledgment within 2000 ms; last readback error=Some("invalid scanout: backend=4 flags=1 context=10 serial=0 size=2560 | not recorded | not recorded |
| E11 | FAIL 9095 ms [E11:L367] — bwm composite of client 2 commit 4: no acknowledgment within 2000 ms; last readback error=Some("invalid scanout: backend=4 flags=1 context=10 serial=0 size=2560 | not recorded | not recorded |
| E12 | FAIL 9952 ms [E12:L1921] — bwm composite of client 83 commit 4: no acknowledgment within 2000 ms; last readback error=Some("invalid scanout: backend=4 flags=1 context=365 serial=0 size=25 | PASS 6500 ms [E12:L1749] | PASS 8734 ms [E12:L1827] |
| E13 | not recorded | not recorded | not recorded |
| E14 | FAIL 9451 ms [E14:L371] — bwm composite of client 2 commit 4: no acknowledgment within 2000 ms; last readback error=Some("invalid scanout: backend=4 flags=1 context=10 serial=0 size=2560 | not recorded | not recorded |
| E15 | FAIL 9591 ms [E15:L1915] — bwm composite of client 83 commit 5: no acknowledgment within 2000 ms; last readback error=Some("invalid scanout: backend=4 flags=1 context=362 serial=0 size=25 | PASS 7287 ms [E15:L1742] | PASS 8660 ms [E15:L1822] |
| E16 | FAIL 8522 ms [E16:L1908] — bwm composite of client 83 commit 5: no acknowledgment within 2000 ms; last readback error=Some("invalid scanout: backend=4 flags=1 context=365 serial=0 size=25 | PASS 7356 ms [E16:L1740] | PASS 8479 ms [E16:L1817] |
| E17 | PASS 8499 ms [E17:L1880] | PASS 4862 ms [E17:L1750] | PASS 7968 ms [E17:L1827] |
| E18 | not recorded | not recorded | not recorded |
| E19 | FAIL 4332 ms [E19:L1858] — init desktop's initial client reached scanout: no acknowledgment within 2000 ms; windows=[]; last readback error=None; last completed=Some("context=344 serial=1 | PASS 6073 ms [E19:L1733] | PASS 7101 ms [E19:L1808]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E19. --> |
| E20 | FAIL 7758 ms [E20:L1851] — bwm composite of client 82 commit 4: no acknowledgment within 2000 ms; last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None; [ini | PASS 6430 ms [E20:L1683] | PASS 7678 ms [E20:L1760]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E20. --> |
| E21 | FAIL 8842 ms [E21:L1915] — bwm composite of client 83 commit 5: no acknowledgment within 2000 ms; last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None; [ini | FAIL 5092 ms [E21:L1748] — desktop process exited with wait status 256; [bwm] settings font.size=14 clock.visible=true [bwm] Breenix Window Manager starting... (v2-chromeless-skip) [bwm] | PASS 6622 ms [E21:L1826]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E21. --> |
| E22 | FAIL 8687 ms [E22:L1900] — bwm composite of client 83 commit 4: no acknowledgment within 2000 ms; last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None; [ini | FAIL 5278 ms [E22:L1738] — desktop process exited with wait status 256; [bwm] settings font.size=14 clock.visible=true [bwm] Breenix Window Manager starting... (v2-chromeless-skip) [bwm] | PASS 7462 ms [E22:L1811]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E22. --> |
| E23 | not recorded | not recorded | not recorded |
| E24 | FAIL 8795 ms [E24:L504] — bwm composite of client 7 commit 5: no acknowledgment within 2000 ms; last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None; [init | FAIL 5114 ms [E24:L331] — desktop process exited with wait status 256; [bwm] settings font.size=14 clock.visible=true [bwm] Breenix Window Manager starting... (v2-chromeless-skip) [bwm] | PASS 7374 ms [E24:L408]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E24. --> |
| E25 | FAIL 7859 ms [E25:L1913] — bwm composite of client 83 commit 5: no acknowledgment within 2000 ms; last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None; [ini | FAIL 5464 ms [E25:L1746] — desktop process exited with wait status 256; [bwm] settings font.size=14 clock.visible=true [bwm] Breenix Window Manager starting... (v2-chromeless-skip) [bwm] | PASS 6511 ms [E25:L1820]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E25. --> |
| E26 | not recorded | PASS 6369 ms [E26:L302] | not recorded |
| E27 | FAIL 8885 ms [E27:L499] — bwm composite of client 7 commit 4: no acknowledgment within 2000 ms; last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None; [init | FAIL 5251 ms [E27:L329] — desktop process exited with wait status 256; [bwm] settings font.size=14 clock.visible=true [bwm] Breenix Window Manager starting... (v2-chromeless-skip) [bwm] | PASS 7365 ms [E27:L407]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E27. --> |
| E28 | FAIL 7855 ms [E28:L1893] — bwm composite of client 83 commit 4: no acknowledgment within 2000 ms; last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None; [ini | PASS 5826 ms [E28:L1723] | PASS 7622 ms [E28:L1803]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E28. --> |
| E29 | FAIL 8604 ms [E29:L369] — bwm composite of client 2 commit 4: no acknowledgment within 2000 ms; last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None; [init | not recorded | not recorded  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E29. --> |
| E30 | FAIL 8716 ms [E30:L448] — bwm composite of client 2 commit 4: no acknowledgment within 2000 ms; last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None; [init | not recorded | not recorded  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E30. --> |
| E31 | FAIL 8437 ms [E31:L2058] — bwm composite of client 83 commit 4: no acknowledgment within 2000 ms; last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None; [ini | PASS 6164 ms [E31:L1810] | PASS 7371 ms [E31:L1890]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E31. --> |
| E32 | FAIL 8444 ms [E32:L1941] — bwm composite of client 83 commit 4: no acknowledgment within 2000 ms; last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None; [ini | PASS 5269 ms [E32:L1702] | PASS 6504 ms [E32:L1776]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E32. --> |
| E33 | PASS 7422 ms [E33:L1843] | PASS 6007 ms [E33:L1710] | PASS 7202 ms [E33:L1787] |
| E34 | FAIL 8398 ms [E34:L1962] — bwm composite of client 83 commit 4: no acknowledgment within 2000 ms; last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None; [ini | PASS 5927 ms [E34:L1712] | PASS 7259 ms [E34:L1793]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E34. --> |
| E35 | FAIL 8916 ms [E35:L2121] — bwm composite of client 83 commit 4: no acknowledgment within 2000 ms; last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None; [ini | FAIL 3271 ms [E35:L1862] — reap owned process: no acknowledgment within 500 ms | PASS 7661 ms [E35:L1946]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E35. --> |
| E36 | FAIL 6194 ms [E36:L1965] — init desktop's initial client reached scanout: no acknowledgment within 2000 ms; windows=[(82, 0)]; last readback error=None; last completed=Some("context=342 s | PASS 6944 ms [E36:L1835] | FAIL 6825 ms [E36:L1913] — positive click delivery to target client: positive click delivery to target client: no acknowledgment within 1000 ms  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E36. --> |
| E37 | PASS 3357 ms [E37:L3746] | PASS 2457 ms [E37:L3375] | FAIL 6444 ms [E37:L3600] — bwm composite of client 81 commit 1: no acknowledgment within 2000 ms; last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None; [bwm  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E37. --> |
| E38 | PASS 7615 ms [E38:L2046] | FAIL 3481 ms [E38:L1820] — reap owned process: no acknowledgment within 500 ms | PASS 7524 ms [E38:L1904] |
| E39 | FAIL 8857 ms [E39:L2134] — bwm composite of client 83 commit 4: no acknowledgment within 2000 ms; last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None; [ini | PASS 6217 ms [E39:L1876] | PASS 6337 ms [E39:L1965]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E39. --> |

## Exported observation diagnostics and replacement identity

Diagnostic blocks are indexed by their following CASE. A printed context belongs to a successful observer read, not necessarily the target commit. `None` is the wait's local observation state. Replacement PID is reported only when the log or the two startup timelines identifies it. Single timelines on native stack-restored refer to its replacement; a single timeline elsewhere is not relabelled as a replacement. <!-- claim-lint:ok: Cited code conditions and raw records indexed in docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt; absence is scoped to the hashed capture inputs, not runtime progress. -->

| Capture / case | Exported last readback error and completed context | Replacement PID evidence |
| --- | --- | --- |
| E03 supervisor-restarts | last readback error=Some("invalid scanout: backend=4 flags=1 context=357 serial=0 size=2560x1600 render=1100/207 read=1100/206 present=1100"); last completed=None [E03:L1725–L1734] | not identified by this block [E03:L1725–L1734] |
| E04 supervisor-restarts | last readback error=Some("invalid scanout: backend=4 flags=1 context=361 serial=0 size=2560x1600 render=1100/207 read=1100/206 present=1100"); last completed=None [E04:L1753–L1771] | 352 [E04:L1753–L1771] |
| E05 supervisor-restarts | last readback error=Some("invalid scanout: backend=4 flags=1 context=363 serial=0 size=2560x1600 render=1100/207 read=1100/206 present=1100"); last completed=None [E05:L1740–L1757] | 352 [E05:L1740–L1757] |
| E06 supervisor-restarts | last readback error=Some("invalid scanout: backend=4 flags=1 context=361 serial=0 size=2560x1600 render=1100/207 read=1100/206 present=1100"); last completed=None [E06:L1737–L1754] | 352 [E06:L1737–L1754] |
| E09 supervisor-restarts | last readback error=Some("invalid scanout: backend=4 flags=1 context=10 serial=0 size=2560x1600 render=1100/207 read=1100/206 present=1100"); last completed=None [E09:L326–L346] | 10 [E09:L326–L346] |
| E10 supervisor-restarts | last readback error=Some("invalid scanout: backend=4 flags=1 context=10 serial=0 size=2560x1600 render=1100/207 read=1100/206 present=1100"); last completed=None [E10:L339–L359] | 10 [E10:L339–L359] |
| E11 supervisor-restarts | last readback error=Some("invalid scanout: backend=4 flags=1 context=10 serial=0 size=2560x1600 render=1100/207 read=1100/206 present=1100"); last completed=None [E11:L333–L353] | 10 [E11:L333–L353] |
| E12 supervisor-restarts | last readback error=Some("invalid scanout: backend=4 flags=1 context=365 serial=0 size=2560x1600 render=1100/207 read=1100/206 present=1100"); last completed=None [E12:L1887–L1907] | 352 [E12:L1887–L1907] |
| E14 supervisor-restarts | last readback error=Some("invalid scanout: backend=4 flags=1 context=10 serial=0 size=2560x1600 render=1100/207 read=1100/206 present=1100"); last completed=None [E14:L339–L357] | 10 [E14:L339–L357] |
| E15 supervisor-restarts | last readback error=Some("invalid scanout: backend=4 flags=1 context=362 serial=0 size=2560x1600 render=1100/207 read=1100/206 present=1100"); last completed=None [E15:L1881–L1901] | 878 [E15:L1881–L1901] |
| E16 supervisor-restarts | last readback error=Some("invalid scanout: backend=4 flags=1 context=365 serial=0 size=2560x1600 render=1100/207 read=1100/206 present=1100"); last completed=None [E16:L1876–L1894] | 899 [E16:L1876–L1894] |
| E19 supervisor-restarts | last readback error=None; last completed=Some("context=344 serial=1353 clients=[] pixel=Some(4279506462)") [E19:L1835–L1845] | not identified by this block [E19:L1835–L1845]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E19. --> |
| E20 supervisor-restarts | last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None [E20:L1817–L1837] | 792 [E20:L1817–L1837]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E20. --> |
| E21 restart-adopts | last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None;  [E21:L1740–L1740] | not identified by this block [E21:L1740–L1740]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E21. --> |
| E21 supervisor-restarts | last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None [E21:L1884–L1901] | 841 [E21:L1884–L1901]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E21. --> |
| E22 restart-adopts | last readback error=Some("invalid scanout: backend=4 flags=1 context=331 serial=0 size=2560x1600 render=1100/205 read=1100/206 present=1100"); last completed=None;  [E22:L1720–L1730] | not identified by this block [E22:L1720–L1730] |
| E22 supervisor-restarts | last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None [E22:L1868–L1886] | 829 [E22:L1868–L1886]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E22. --> |
| E24 restart-adopts | last readback error=Some("invalid scanout: backend=4 flags=1 context=12 serial=0 size=2560x1600 render=1100/205 read=1100/206 present=1100"); last completed=None;  [E24:L313–L323] | not identified by this block [E24:L313–L323] |
| E24 supervisor-restarts | last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None [E24:L470–L490] | 89 [E24:L470–L490]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E24. --> |
| E25 restart-adopts | last readback error=Some("invalid scanout: backend=4 flags=1 context=331 serial=0 size=2560x1600 render=1100/205 read=1100/206 present=1100"); last completed=None;  [E25:L1728–L1738] | not identified by this block [E25:L1728–L1738] |
| E25 supervisor-restarts | last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None [E25:L1879–L1899] | 841 [E25:L1879–L1899]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E25. --> |
| E27 restart-adopts | last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None;  [E27:L313–L322] | not identified by this block [E27:L313–L322]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E27. --> |
| E27 supervisor-restarts | last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None [E27:L465–L485] | 89 [E27:L465–L485]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E27. --> |
| E28 supervisor-restarts | last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None [E28:L1861–L1879] | 819 [E28:L1861–L1879]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E28. --> |
| E29 supervisor-restarts | last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None [E29:L335–L355] | 19 [E29:L335–L355]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E29. --> |
| E30 supervisor-restarts | last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None [E30:L416–L433] | 15 [E30:L416–L433]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E30. --> |
| E31 supervisor-restarts | last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None [E31:L2027–L2044] | 837 [E31:L2027–L2044]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E31. --> |
| E32 supervisor-restarts | last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None [E32:L1911–L1927] | 636 [E32:L1911–L1927]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E32. --> |
| E34 supervisor-restarts | last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None [E34:L1931–L1948] | 665 [E34:L1931–L1948]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E34. --> |
| E35 supervisor-restarts | last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None [E35:L2092–L2108] | 728 [E35:L2092–L2108]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E35. --> |
| E36 supervisor-restarts | last readback error=None; last completed=Some("context=342 serial=487 clients=[(82, 0)] pixel=Some(4279506462)") [E36:L1941–L1951] | not identified by this block [E36:L1941–L1951]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E36. --> |
| E37 stack-restored | last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None [E37:L3578–L3586] | not identified by this block [E37:L3578–L3586]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E37. --> |
| E39 supervisor-restarts | last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None [E39:L2100–L2116] | 736 [E39:L2100–L2116]  <!-- claim-lint:ok: Literal capture fields and the cited source conditions; docs/planning/arm64-catchup/serials/a3-mechanism-captures.txt E39. --> |

## Replacement PID coverage

Identity is taken from the phase export immediately before the CASE or the reconstructed init output for that CASE. `unknown` denotes missing identity evidence, including passing runs with no export. A failure in initial launch precedes replacement creation.

| Capture | supervisor-restarts replacement | restart-adopts replacement | stack-restored replacement |
| --- | --- | --- | --- |
| E01 | unknown | unknown | unknown |
| E02 | not reached [E02:L1731] | unknown | unknown |
| E03 | not reached [E03:L1747] | unknown | unknown |
| E04 | 352 [E04:L1753–L1771] | unknown | unknown |
| E05 | 352 [E05:L1740–L1757] | unknown | unknown |
| E06 | 352 [E06:L1737–L1754] | unknown | unknown |
| E07 | CASE not recorded | CASE not recorded | CASE not recorded |
| E08 | CASE not recorded | CASE not recorded | CASE not recorded |
| E09 | 10 [E09:L326–L346] | CASE not recorded | CASE not recorded |
| E10 | 10 [E10:L339–L359] | CASE not recorded | CASE not recorded |
| E11 | 10 [E11:L333–L353] | CASE not recorded | CASE not recorded |
| E12 | 352 [E12:L1887–L1907] | unknown | unknown |
| E13 | CASE not recorded | CASE not recorded | CASE not recorded |
| E14 | 10 [E14:L339–L357] | CASE not recorded | CASE not recorded |
| E15 | 878 [E15:L1881–L1901] | unknown | unknown |
| E16 | 899 [E16:L1876–L1894] | unknown | unknown |
| E17 | unknown | unknown | unknown |
| E18 | CASE not recorded | CASE not recorded | CASE not recorded |
| E19 | not reached [E19:L1858] | unknown | unknown |
| E20 | 792 [E20:L1817–L1837] | unknown | unknown |
| E21 | 841 [E21:L1884–L1901] | unknown | unknown |
| E22 | 829 [E22:L1868–L1886] | unknown | unknown |
| E23 | CASE not recorded | CASE not recorded | CASE not recorded |
| E24 | 89 [E24:L470–L490] | unknown | unknown |
| E25 | 841 [E25:L1879–L1899] | unknown | unknown |
| E26 | CASE not recorded | unknown | CASE not recorded |
| E27 | 89 [E27:L465–L485] | unknown | unknown |
| E28 | 819 [E28:L1861–L1879] | unknown | unknown |
| E29 | 19 [E29:L335–L355] | CASE not recorded | CASE not recorded |
| E30 | 15 [E30:L336–L415] | CASE not recorded | CASE not recorded |
| E31 | 837 [E31:L1948–L2026] | unknown | unknown |
| E32 | 636 [E32:L1831–L1910] | unknown | unknown |
| E33 | unknown | unknown | unknown |
| E34 | 665 [E34:L1851–L1930] | unknown | unknown |
| E35 | 728 [E35:L2016–L2091] | unknown | unknown |
| E36 | not reached [E36:L1965] | unknown | unknown |
| E37 | 879 [E37:L3667–L3744] | unknown | 857 [E37:L3539–L3577] |
| E38 | 731 [E38:L1952–L2031] | unknown | unknown |
| E39 | 736 [E39:L2025–L2099] | unknown | unknown |

## Startup phase table

Each cell below is an exact retained stamp: `phase = absolute ns / elapsed us @ original line`. No absent phase is filled in. The original and replacement timelines stay separate. Stamps use compare-exchange and retain the **first** occurrence of a phase (`libs/libuitest/src/startup.rs:52–55`); for a band loop, `frame-pixels-ready` does not establish completion of the entire frame (`userspace/programs/src/bwm/compositor.rs:743`).

| Capture / PID | Retained phase stamps (ns / us; source line) |
| --- | --- |
| E01 | no exported wm-startup stamps |
| E02 | no exported wm-startup stamps |
| E03 | no exported wm-startup stamps |
| E04 | no exported wm-startup stamps |
| E05 | no exported wm-startup stamps |
| E06 | no exported wm-startup stamps |
| E07 | no exported wm-startup stamps |
| E08 | no exported wm-startup stamps |
| E09 | no exported wm-startup stamps |
| E10 | no exported wm-startup stamps |
| E11 | no exported wm-startup stamps |
| E12 | no exported wm-startup stamps |
| E13 | no exported wm-startup stamps |
| E14 | no exported wm-startup stamps |
| E15 | no exported wm-startup stamps |
| E16 | no exported wm-startup stamps |
| E17 | no exported wm-startup stamps |
| E18 | no exported wm-startup stamps |
| E19 | no exported wm-startup stamps |
| E20 | no exported wm-startup stamps |
| E21 | no exported wm-startup stamps |
| E22 | no exported wm-startup stamps |
| E23 | no exported wm-startup stamps |
| E24 | no exported wm-startup stamps |
| E25 | no exported wm-startup stamps |
| E26 | no exported wm-startup stamps |
| E27 | no exported wm-startup stamps |
| E28 | no exported wm-startup stamps |
| E29 | no exported wm-startup stamps |
| E30 / 4 | compositor-exec = 1346542625 / 0 @E30:L336<br>config-ready = 1346628875 / 86 @E30:L337<br>context-begin = 2323800708 / 977258 @E30:L338<br>context-ready = 2323804166 / 977261 @E30:L339<br>desktop-target-ready = 2326830541 / 980287 @E30:L340<br>background-target-ready = 2331294166 / 984751 @E30:L341<br>scanout-a-ready = 2335117791 / 988575 @E30:L342<br>scanout-b-ready = 2339289375 / 992746 @E30:L343<br>targets-ready = 2346987666 / 1000445 @E30:L347<br>startup-font-begin = 2323812583 / 977269 @E30:L348<br>fonts-ready = 2726506875 / 1379964 @E30:L349<br>scene-ready = 2323691791 / 977149 @E30:L350<br>background-ready = 3059544291 / 1713001 @E30:L351<br>chrome-blit-ready = 3322964750 / 1976422 @E30:L352<br>chrome-taskbar-ready = 3328887041 / 1982344 @E30:L353<br>chrome-appbar-ready = 3329734000 / 1983191 @E30:L354<br>chrome-ready = 3329735666 / 1983193 @E30:L355<br>fills-ready = 3345894208 / 1999351 @E30:L356<br>pointer-presented = 3347284833 / 2000742 @E30:L357<br>checks-ready = 3488042041 / 2141499 @E30:L358<br>font-plan-begin = 2348605583 / 1002062 @E30:L359<br>font-plan-return = 2352141333 / 1005598 @E30:L360<br>font-submit-begin = 2352142250 / 1005599 @E30:L361<br>font-submit-return = 2723596291 / 1377053 @E30:L362<br>font-wait-return = 2726501541 / 1379958 @E30:L363<br>font-draw-return = 3328877875 / 1982335 @E30:L364<br>frame-check-begin = 3347288833 / 2000746 @E30:L365<br>frame-buffers-ready = 3352888500 / 2006345 @E30:L366<br>frame-scanout-return = 3387079166 / 2040536 @E30:L367<br>frame-desktop-return = 3379503458 / 2032960 @E30:L368<br>frame-pixels-ready = 3388509791 / 2041967 @E30:L369<br>initial-windows-ready = 3345914583 / 1999371 @E30:L370<br>title-plan-begin = 3345977041 / 1999434 @E30:L371<br>title-plan-return = 3346006791 / 1999464 @E30:L372<br>client-chrome-ready = 3489516833 / 2142974 @E30:L373<br>client-compose-begin = 3493410083 / 2146867 @E30:L374<br>desktop-ready = 3488046625 / 2141504 @E30:L375<br>first-composite = 4707953416 / 3361410 @E30:L376 |
| E30 / 15 | compositor-exec = 7479618125 / 0 @E30:L377<br>config-ready = 7479734625 / 116 @E30:L378<br>context-begin = 8441788541 / 962170 @E30:L379<br>context-ready = 8441792916 / 962174 @E30:L380<br>desktop-target-ready = 8445015208 / 965397 @E30:L381<br>background-target-ready = 8448175916 / 968557 @E30:L382<br>scanout-a-ready = 8451514833 / 971896 @E30:L383<br>scanout-b-ready = 8454687333 / 975069 @E30:L384<br>scene-target-ready = 8457700333 / 978082 @E30:L385<br>targets-ready = 8459741833 / 980123 @E30:L386<br>startup-font-begin = 8441806625 / 962188 @E30:L387<br>fonts-ready = 8839740916 / 1360122 @E30:L388<br>scene-ready = 8441689041 / 962070 @E30:L389<br>background-ready = 9188358666 / 1708740 @E30:L390<br>chrome-blit-ready = 9299807541 / 1820189 @E30:L391<br>chrome-taskbar-ready = 9423539208 / 1943921 @E30:L392<br>chrome-appbar-ready = 9424997208 / 1945379 @E30:L393<br>chrome-ready = 9424998125 / 1945380 @E30:L394<br>fills-ready = 9440236041 / 1960617 @E30:L395<br>pointer-presented = 9443557750 / 1963939 @E30:L396<br>checks-ready = 9578320958 / 2098702 @E30:L397<br>font-plan-begin = 8455107875 / 975489 @E30:L398<br>font-plan-return = 8463625375 / 984007 @E30:L399<br>font-submit-begin = 8463626208 / 984008 @E30:L400<br>font-submit-return = 8837024291 / 1357406 @E30:L401<br>font-wait-return = 8839730416 / 1360112 @E30:L402<br>font-draw-return = 9423529750 / 1943911 @E30:L403<br>frame-check-begin = 9443566041 / 1963947 @E30:L404<br>frame-buffers-ready = 9447118708 / 1967500 @E30:L405<br>frame-scanout-return = 9480253916 / 2000635 @E30:L406<br>frame-desktop-return = 9472998458 / 1993380 @E30:L407<br>frame-pixels-ready = 9481814625 / 2002196 @E30:L408<br>initial-windows-ready = 9440261958 / 1960643 @E30:L409<br>title-plan-begin = 9440323333 / 1960705 @E30:L410<br>title-plan-return = 9440371666 / 1960753 @E30:L411<br>client-chrome-ready = 9606406833 / 2126788 @E30:L412<br>client-compose-begin = 9610796416 / 2131178 @E30:L413<br>desktop-ready = 9578325041 / 2098706 @E30:L414<br>first-composite = 9612911083 / 2133292 @E30:L415 |
| E31 / 825 | compositor-exec = 245392907666 / 0 @E31:L1948<br>config-ready = 245392985833 / 78 @E31:L1949<br>context-begin = 246377882875 / 984975 @E31:L1950<br>context-ready = 246377887125 / 984979 @E31:L1951<br>desktop-target-ready = 246380844791 / 987937 @E31:L1952<br>background-target-ready = 246383832166 / 990924 @E31:L1953<br>scanout-a-ready = 246387025500 / 994117 @E31:L1954<br>scanout-b-ready = 246390005875 / 997098 @E31:L1955<br>scene-target-ready = 246392878875 / 999971 @E31:L1956<br>targets-ready = 246397784416 / 1004876 @E31:L1957<br>startup-font-begin = 246377893875 / 984986 @E31:L1958<br>fonts-ready = 246832100791 / 1439193 @E31:L1959<br>scene-ready = 246377791625 / 984883 @E31:L1960<br>background-ready = 246397781750 / 1004874 @E31:L1961<br>chrome-blit-ready = 247126772083 / 1733864 @E31:L1962<br>chrome-taskbar-ready = 247156057041 / 1763149 @E31:L1963<br>chrome-appbar-ready = 247156844125 / 1763936 @E31:L1964<br>chrome-ready = 247156845041 / 1763937 @E31:L1965<br>fills-ready = 247383181416 / 1990273 @E31:L1966<br>pointer-presented = 247385499041 / 1992591 @E31:L1967<br>checks-ready = 247524901333 / 2131993 @E31:L1970<br>font-plan-begin = 246392749583 / 999841 @E31:L1971<br>font-plan-return = 246400510833 / 1007603 @E31:L1972<br>font-submit-begin = 246400511791 / 1007604 @E31:L1973<br>font-submit-return = 246829360416 / 1436452 @E31:L1974<br>font-wait-return = 246832095458 / 1439187 @E31:L1975<br>font-draw-return = 247156047291 / 1763139 @E31:L1976<br>frame-check-begin = 247385503333 / 1992595 @E31:L1977<br>frame-buffers-ready = 247388916750 / 1996009 @E31:L1978<br>frame-scanout-return = 247421101083 / 2028193 @E31:L1979<br>frame-desktop-return = 247413902208 / 2020994 @E31:L1980<br>frame-pixels-ready = 247422620333 / 2029712 @E31:L1981<br>initial-windows-ready = 247383196291 / 1990288 @E31:L1982<br>title-plan-begin = 247383259333 / 1990351 @E31:L1983<br>title-plan-return = 247383285250 / 1990377 @E31:L1984<br>client-chrome-ready = 247526024291 / 2133116 @E31:L1985<br>client-compose-begin = 247530355875 / 2137448 @E31:L1986<br>desktop-ready = 247524904875 / 2131997 @E31:L1987<br>first-composite = 249146934375 / 3754026 @E31:L1988 |
| E31 / 837 | compositor-exec = 251085676875 / 0 @E31:L1989<br>config-ready = 251085797041 / 120 @E31:L1990<br>context-begin = 252062746625 / 977069 @E31:L1991<br>context-ready = 252062751125 / 977074 @E31:L1992<br>desktop-target-ready = 252065861166 / 980184 @E31:L1993<br>background-target-ready = 252068705666 / 983028 @E31:L1994<br>scanout-a-ready = 252071683958 / 986007 @E31:L1995<br>scanout-b-ready = 252074769333 / 989092 @E31:L1996<br>scene-target-ready = 252077661708 / 991984 @E31:L1997<br>targets-ready = 252082694583 / 997017 @E31:L1998<br>startup-font-begin = 252062770833 / 977093 @E31:L1999<br>fonts-ready = 252470144083 / 1384467 @E31:L2000<br>scene-ready = 252062641666 / 976964 @E31:L2001<br>background-ready = 252082692500 / 997015 @E31:L2002<br>chrome-blit-ready = 252522316500 / 1436639 @E31:L2003<br>chrome-taskbar-ready = 252679345625 / 1593668 @E31:L2004<br>chrome-appbar-ready = 252681767041 / 1596090 @E31:L2005<br>chrome-ready = 252681768250 / 1596091 @E31:L2006<br>fills-ready = 253069921875 / 1984245 @E31:L2007<br>pointer-presented = 253073123125 / 1987446 @E31:L2008<br>checks-ready = 253217343125 / 2131666 @E31:L2009<br>font-plan-begin = 252077228125 / 991551 @E31:L2010<br>font-plan-return = 252086133083 / 1000456 @E31:L2011<br>font-submit-begin = 252086134041 / 1000457 @E31:L2012<br>font-submit-return = 252467750166 / 1382073 @E31:L2013<br>font-wait-return = 252470139541 / 1384462 @E31:L2014<br>font-draw-return = 252679336916 / 1593660 @E31:L2015<br>frame-check-begin = 253073129416 / 1987452 @E31:L2016<br>frame-buffers-ready = 253076569583 / 1990892 @E31:L2017<br>frame-scanout-return = 253110854375 / 2025177 @E31:L2018<br>frame-desktop-return = 253110845166 / 2025168 @E31:L2019<br>frame-pixels-ready = 253112268125 / 2026591 @E31:L2020<br>initial-windows-ready = 253069963166 / 1984286 @E31:L2021<br>title-plan-begin = 253070031291 / 1984354 @E31:L2022<br>title-plan-return = 253070076083 / 1984399 @E31:L2023<br>client-chrome-ready = 253250891375 / 2165214 @E31:L2024<br>client-compose-begin = 253254232833 / 2168555 @E31:L2025<br>desktop-ready = 253217347541 / 2131670 @E31:L2026 |
| E32 / 626 | compositor-exec = 241606711333 / 0 @E32:L1831<br>config-ready = 241606795541 / 84 @E32:L1832<br>context-begin = 242588996250 / 982284 @E32:L1833<br>context-ready = 242589002291 / 982290 @E32:L1834<br>desktop-target-ready = 242591963375 / 985252 @E32:L1837<br>background-target-ready = 242594953416 / 988242 @E32:L1838<br>scanout-a-ready = 242598054500 / 991343 @E32:L1839<br>scanout-b-ready = 242600971375 / 994260 @E32:L1840<br>scene-target-ready = 242605511666 / 998800 @E32:L1841<br>targets-ready = 242609810208 / 1003098 @E32:L1842<br>startup-font-begin = 242589013250 / 982301 @E32:L1843<br>fonts-ready = 243049259125 / 1442547 @E32:L1844<br>scene-ready = 242588911458 / 982200 @E32:L1845<br>background-ready = 242609808250 / 1003096 @E32:L1846<br>chrome-blit-ready = 243259529666 / 1652818 @E32:L1847<br>chrome-taskbar-ready = 243381682666 / 1774971 @E32:L1848<br>chrome-appbar-ready = 243382532250 / 1775820 @E32:L1849<br>chrome-ready = 243382533791 / 1775822 @E32:L1850<br>fills-ready = 243572315333 / 1965604 @E32:L1851<br>pointer-presented = 243573653833 / 1966942 @E32:L1852<br>checks-ready = 243713865958 / 2107154 @E32:L1853<br>font-plan-begin = 242611976541 / 1005265 @E32:L1854<br>font-plan-return = 242615478333 / 1008767 @E32:L1855<br>font-submit-begin = 242615479166 / 1008767 @E32:L1856<br>font-submit-return = 243046914666 / 1440203 @E32:L1857<br>font-wait-return = 243049253916 / 1442542 @E32:L1858<br>font-draw-return = 243381674000 / 1774962 @E32:L1859<br>frame-check-begin = 243573658166 / 1966946 @E32:L1860<br>frame-buffers-ready = 243576661750 / 1969950 @E32:L1861<br>frame-scanout-return = 243609028125 / 2002316 @E32:L1862<br>frame-desktop-return = 243601519583 / 1994808 @E32:L1863<br>frame-pixels-ready = 243610529625 / 2003818 @E32:L1864<br>initial-windows-ready = 243572334291 / 1965622 @E32:L1865<br>title-plan-begin = 243572402583 / 1965691 @E32:L1866<br>title-plan-return = 243572427416 / 1965716 @E32:L1867<br>client-chrome-ready = 243715053458 / 2108342 @E32:L1868<br>client-compose-begin = 243719821166 / 2113109 @E32:L1869<br>desktop-ready = 243713871125 / 2107159 @E32:L1870<br>first-composite = 245247196541 / 3640485 @E32:L1871 |
| E32 / 636 | compositor-exec = 247478170416 / 0 @E32:L1872<br>config-ready = 247478259041 / 88 @E32:L1873<br>context-begin = 248403877625 / 925707 @E32:L1874<br>context-ready = 248403884083 / 925713 @E32:L1875<br>desktop-target-ready = 248407045416 / 928875 @E32:L1876<br>background-target-ready = 248410081458 / 931911 @E32:L1877<br>scanout-a-ready = 248413009625 / 934839 @E32:L1878<br>scanout-b-ready = 248416075958 / 937905 @E32:L1879<br>scene-target-ready = 248419026166 / 940855 @E32:L1880<br>targets-ready = 248424306083 / 946135 @E32:L1881<br>startup-font-begin = 248403895083 / 925724 @E32:L1882<br>fonts-ready = 248862435875 / 1384265 @E32:L1883<br>scene-ready = 248403771208 / 925600 @E32:L1884<br>background-ready = 248424304291 / 946133 @E32:L1885<br>chrome-blit-ready = 249015162666 / 1536992 @E32:L1886<br>chrome-taskbar-ready = 249020774958 / 1542604 @E32:L1887<br>chrome-appbar-ready = 249021632250 / 1543461 @E32:L1888<br>chrome-ready = 249021633166 / 1543462 @E32:L1889<br>fills-ready = 249415069166 / 1936898 @E32:L1890<br>pointer-presented = 249416393458 / 1938223 @E32:L1891<br>checks-ready = 249562457250 / 2084286 @E32:L1892<br>font-plan-begin = 248419298416 / 941128 @E32:L1893<br>font-plan-return = 248427355708 / 949185 @E32:L1894<br>font-submit-begin = 248427356500 / 949186 @E32:L1895<br>font-submit-return = 248859352000 / 1381181 @E32:L1896<br>font-wait-return = 248862430541 / 1384260 @E32:L1897<br>font-draw-return = 249020766666 / 1542596 @E32:L1898<br>frame-check-begin = 249416396333 / 1938225 @E32:L1899<br>frame-buffers-ready = 249420659125 / 1942488 @E32:L1900<br>frame-scanout-return = 249447715958 / 1969545 @E32:L1901<br>frame-desktop-return = 249454896625 / 1976726 @E32:L1902<br>frame-pixels-ready = 249456435333 / 1978264 @E32:L1903<br>initial-windows-ready = 249415103833 / 1936933 @E32:L1904<br>title-plan-begin = 249415163541 / 1936993 @E32:L1905<br>title-plan-return = 249415207625 / 1937037 @E32:L1906<br>client-chrome-ready = 249598499958 / 2120329 @E32:L1907<br>client-compose-begin = 249602995291 / 2124824 @E32:L1908<br>desktop-ready = 249562461083 / 2084290 @E32:L1909<br>first-composite = 249605229791 / 2127059 @E32:L1910 |
| E33 | no exported wm-startup stamps |
| E34 / 654 | compositor-exec = 241101990250 / 0 @E34:L1851<br>config-ready = 241102074708 / 84 @E34:L1852<br>context-begin = 242059999708 / 958009 @E34:L1853<br>context-ready = 242060003500 / 958013 @E34:L1854<br>desktop-target-ready = 242160915375 / 1058925 @E34:L1855<br>background-target-ready = 242267941791 / 1165951 @E34:L1856<br>scanout-a-ready = 242357181083 / 1255190 @E34:L1857<br>scanout-b-ready = 242725873500 / 1623883 @E34:L1858<br>scene-target-ready = 242769779250 / 1667789 @E34:L1859<br>targets-ready = 243006498208 / 1904507 @E34:L1860<br>startup-font-begin = 241107175583 / 5185 @E34:L1861<br>fonts-ready = 243035696250 / 1933706 @E34:L1864<br>scene-ready = 242059995666 / 958005 @E34:L1865<br>background-ready = 243006496041 / 1904505 @E34:L1866<br>chrome-blit-ready = 243036216083 / 1934225 @E34:L1867<br>chrome-taskbar-ready = 243040773250 / 1938783 @E34:L1868<br>chrome-appbar-ready = 243042268250 / 1940278 @E34:L1869<br>chrome-ready = 243042269166 / 1940278 @E34:L1870<br>fills-ready = 243058512791 / 1956522 @E34:L1871<br>pointer-presented = 243060744291 / 1958754 @E34:L1872<br>checks-ready = 243201595291 / 2099605 @E34:L1873<br>font-plan-begin = 243014654416 / 1912664 @E34:L1874<br>font-plan-return = 243017614125 / 1915623 @E34:L1875<br>font-submit-begin = 243017615083 / 1915624 @E34:L1876<br>font-submit-return = 243033412291 / 1931422 @E34:L1877<br>font-wait-return = 243035691583 / 1933701 @E34:L1878<br>font-draw-return = 243040766000 / 1938775 @E34:L1879<br>frame-check-begin = 243060751875 / 1958761 @E34:L1880<br>frame-buffers-ready = 243064665291 / 1962675 @E34:L1881<br>frame-scanout-return = 243098570791 / 1996580 @E34:L1882<br>frame-desktop-return = 243098539208 / 1996548 @E34:L1883<br>frame-pixels-ready = 243100015041 / 1998024 @E34:L1884<br>initial-windows-ready = 243058526833 / 1956536 @E34:L1885<br>title-plan-begin = 243058593791 / 1956603 @E34:L1886<br>title-plan-return = 243058622250 / 1956632 @E34:L1887<br>client-chrome-ready = 243202649541 / 2100659 @E34:L1888<br>client-compose-begin = 243202718916 / 2100728 @E34:L1889<br>desktop-ready = 243201599041 / 2099608 @E34:L1890<br>first-composite = 244232363541 / 3130373 @E34:L1891 |
| E34 / 665 | compositor-exec = 246864521833 / 0 @E34:L1892<br>config-ready = 246864541416 / 19 @E34:L1893<br>context-begin = 247832652083 / 968130 @E34:L1894<br>context-ready = 247832655750 / 968133 @E34:L1895<br>desktop-target-ready = 247930094458 / 1065572 @E34:L1896<br>background-target-ready = 248126409833 / 1261888 @E34:L1897<br>scanout-a-ready = 248491484125 / 1626962 @E34:L1898<br>scanout-b-ready = 248522276541 / 1657754 @E34:L1899<br>scene-target-ready = 248578633583 / 1714111 @E34:L1900<br>targets-ready = 248796654583 / 1932132 @E34:L1901<br>startup-font-begin = 246869800125 / 5278 @E34:L1902<br>fonts-ready = 248673760291 / 1809238 @E34:L1903<br>scene-ready = 247832646500 / 968124 @E34:L1904<br>background-ready = 248796652875 / 1932131 @E34:L1905<br>chrome-blit-ready = 248797081666 / 1932559 @E34:L1906<br>chrome-taskbar-ready = 248802735333 / 1938213 @E34:L1907<br>chrome-appbar-ready = 248803464041 / 1938942 @E34:L1908<br>chrome-ready = 248803465291 / 1938943 @E34:L1909<br>fills-ready = 248820104833 / 1955583 @E34:L1910<br>pointer-presented = 248821449958 / 1956928 @E34:L1911<br>checks-ready = 248962895000 / 2098373 @E34:L1912<br>font-plan-begin = 246893585250 / 29063 @E34:L1913<br>font-plan-return = 248038074833 / 1173553 @E34:L1914<br>font-submit-begin = 248038076166 / 1173554 @E34:L1915<br>font-submit-return = 248671539250 / 1807017 @E34:L1916<br>font-wait-return = 248673753791 / 1809231 @E34:L1917<br>font-draw-return = 248802728416 / 1938206 @E34:L1918<br>frame-check-begin = 248821452875 / 1956931 @E34:L1919<br>frame-buffers-ready = 248824943416 / 1960421 @E34:L1920<br>frame-scanout-return = 248859358541 / 1994836 @E34:L1921<br>frame-desktop-return = 248859349875 / 1994828 @E34:L1922<br>frame-pixels-ready = 248860740208 / 1996218 @E34:L1923<br>initial-windows-ready = 248820136250 / 1955614 @E34:L1924<br>title-plan-begin = 248820195750 / 1955673 @E34:L1925<br>title-plan-return = 248820248041 / 1955726 @E34:L1926<br>client-chrome-ready = 248999280791 / 2134758 @E34:L1927<br>client-compose-begin = 248999366750 / 2134844 @E34:L1928<br>desktop-ready = 248962898916 / 2098377 @E34:L1929<br>first-composite = 249003696708 / 2139174 @E34:L1930 |
| E35 / 716 | compositor-exec = 250324808416 / 0 @E35:L2016<br>config-ready = 250324888708 / 80 @E35:L2017<br>context-begin = 251416354541 / 1091546 @E35:L2018<br>context-ready = 251416358625 / 1091550 @E35:L2019<br>desktop-target-ready = 250336766083 / 11957 @E35:L2020<br>background-target-ready = 250346778333 / 21969 @E35:L2021<br>scanout-a-ready = 250349866833 / 25058 @E35:L2022<br>scanout-b-ready = 250353434041 / 28625 @E35:L2023<br>scene-target-ready = 250419681291 / 94872 @E35:L2024<br>targets-ready = 251580374291 / 1255565 @E35:L2025<br>startup-font-begin = 250333639041 / 8830 @E35:L2026<br>fonts-ready = 252035140916 / 1710332 @E35:L2027<br>scene-ready = 251468095541 / 1143287 @E35:L2029<br>background-ready = 251580372958 / 1255564 @E35:L2030<br>chrome-blit-ready = 252353964500 / 2029156 @E35:L2031<br>chrome-taskbar-ready = 252360191458 / 2035383 @E35:L2032<br>chrome-appbar-ready = 252361430875 / 2036622 @E35:L2033<br>chrome-ready = 252361432375 / 2036623 @E35:L2034<br>fills-ready = 252379082416 / 2054274 @E35:L2035<br>pointer-presented = 252382214458 / 2057406 @E35:L2036<br>checks-ready = 252528879500 / 2204071 @E35:L2037<br>font-plan-begin = 250353912958 / 29104 @E35:L2038<br>font-plan-return = 251532434791 / 1207626 @E35:L2039<br>font-submit-begin = 251532436000 / 1207627 @E35:L2040<br>font-submit-return = 251977562708 / 1652754 @E35:L2041<br>font-wait-return = 252035134541 / 1710326 @E35:L2042<br>font-draw-return = 252360184541 / 2035376 @E35:L2043<br>frame-check-begin = 252382217333 / 2057408 @E35:L2044<br>frame-buffers-ready = 252385172833 / 2060364 @E35:L2045<br>frame-scanout-return = 252419757500 / 2094949 @E35:L2046<br>frame-desktop-return = 252419748500 / 2094940 @E35:L2047<br>frame-pixels-ready = 252421139625 / 2096331 @E35:L2048<br>initial-windows-ready = 252379102833 / 2054294 @E35:L2049<br>title-plan-begin = 252379172750 / 2054364 @E35:L2050<br>title-plan-return = 252379199750 / 2054391 @E35:L2051<br>client-chrome-ready = 252529987375 / 2205178 @E35:L2052<br>client-compose-begin = 252530048000 / 2205239 @E35:L2053<br>desktop-ready = 252528883583 / 2204075 @E35:L2054<br>first-composite = 253498235333 / 3173426 @E35:L2055 |
| E35 / 728 | compositor-exec = 256410678375 / 0 @E35:L2056<br>config-ready = 256410758583 / 80 @E35:L2057<br>context-begin = 257485681916 / 1075003 @E35:L2058<br>context-ready = 257485687625 / 1075009 @E35:L2059<br>desktop-target-ready = 256425167791 / 14489 @E35:L2060<br>background-target-ready = 256432445416 / 21767 @E35:L2061<br>scanout-a-ready = 256435605291 / 24926 @E35:L2062<br>scanout-b-ready = 256438748708 / 28070 @E35:L2063<br>scene-target-ready = 256495771125 / 85092 @E35:L2064<br>targets-ready = 257653378208 / 1242699 @E35:L2065<br>startup-font-begin = 256420655250 / 9976 @E35:L2066<br>fonts-ready = 258464648583 / 2053970 @E35:L2067<br>scene-ready = 257538086875 / 1127408 @E35:L2068<br>background-ready = 257653376500 / 1242698 @E35:L2069<br>chrome-taskbar-ready = 258468813666 / 2058135 @E35:L2073<br>chrome-appbar-ready = 258469511000 / 2058832 @E35:L2074<br>chrome-ready = 258469513416 / 2058835 @E35:L2075<br>fills-ready = 258486517666 / 2075839 @E35:L2076<br>pointer-presented = 258488234291 / 2077555 @E35:L2077<br>font-plan-begin = 258096291875 / 1685613 @E35:L2078<br>font-plan-return = 258438751166 / 2028072 @E35:L2079<br>font-submit-begin = 258438752500 / 2028074 @E35:L2080<br>font-submit-return = 258462448458 / 2051770 @E35:L2081<br>font-wait-return = 258464643375 / 2053965 @E35:L2082<br>font-draw-return = 258468804333 / 2058125 @E35:L2083<br>frame-check-begin = 258488236583 / 2077558 @E35:L2084<br>frame-buffers-ready = 258491512875 / 2080834 @E35:L2085<br>frame-scanout-return = 258526331083 / 2115652 @E35:L2086<br>frame-desktop-return = 258526324583 / 2115646 @E35:L2087<br>frame-pixels-ready = 258527779291 / 2117100 @E35:L2088<br>initial-windows-ready = 258486556333 / 2075877 @E35:L2089<br>title-plan-begin = 258486615791 / 2075937 @E35:L2090<br>title-plan-return = 258486667916 / 2075989 @E35:L2091 |
| E36 | no exported wm-startup stamps |
| E37 / 857 | compositor-exec = 151910851374 / 0 @E37:L3539<br>config-ready = 151911177058 / 325 @E37:L3540<br>context-begin = 151954650510 / 43799 @E37:L3541<br>context-ready = 151959532396 / 48681 @E37:L3542<br>desktop-target-ready = 151922858069 / 12006 @E37:L3543<br>background-target-ready = 151930598176 / 19746 @E37:L3544<br>scanout-a-ready = 151937995134 / 27143 @E37:L3545<br>scanout-b-ready = 151946606601 / 35755 @E37:L3546<br>scene-target-ready = 151954640193 / 43788 @E37:L3547<br>targets-ready = 151969983751 / 59132 @E37:L3548<br>startup-font-begin = 151915064675 / 4213 @E37:L3549<br>fonts-ready = 152052906847 / 142055 @E37:L3550<br>scene-ready = 151965442011 / 54590 @E37:L3551<br>background-ready = 151969982981 / 59131 @E37:L3552<br>chrome-blit-ready = 152054520900 / 143669 @E37:L3553<br>chrome-taskbar-ready = 152062382998 / 151531 @E37:L3554<br>chrome-appbar-ready = 152063868316 / 153016 @E37:L3555<br>chrome-ready = 152063868965 / 153017 @E37:L3556<br>fills-ready = 152080980099 / 170128 @E37:L3557<br>pointer-presented = 152092202102 / 181350 @E37:L3558<br>checks-ready = 152148335884 / 237484 @E37:L3559<br>font-plan-begin = 151928665904 / 17814 @E37:L3560<br>font-plan-return = 151977917293 / 67065 @E37:L3561<br>font-submit-begin = 151977918370 / 67066 @E37:L3562<br>font-submit-return = 152042345935 / 131494 @E37:L3563<br>font-wait-return = 152052900312 / 142048 @E37:L3564<br>font-draw-return = 152062377454 / 151526 @E37:L3565<br>frame-check-begin = 152092204899 / 181353 @E37:L3566<br>frame-buffers-ready = 152095564838 / 184713 @E37:L3567<br>frame-scanout-return = 152108785300 / 197933 @E37:L3568<br>frame-desktop-return = 152107715499 / 196864 @E37:L3569<br>frame-pixels-ready = 152111804893 / 200953 @E37:L3570<br>initial-windows-ready = 152081026889 / 170175 @E37:L3571<br>title-plan-begin = 152081102055 / 170250 @E37:L3572<br>title-plan-return = 152081117202 / 170265 @E37:L3573<br>client-chrome-ready = 152156642814 / 245791 @E37:L3574<br>client-compose-begin = 152156780304 / 245928 @E37:L3575<br>desktop-ready = 152148340225 / 237488 @E37:L3576<br>first-composite = 152181551943 / 270700 @E37:L3577 |
| E37 / 867 | compositor-exec = 157596626314 / 0 @E37:L3667<br>config-ready = 157596983692 / 357 @E37:L3668<br>context-begin = 157658665233 / 62038 @E37:L3669<br>context-ready = 157661179226 / 64552 @E37:L3670<br>desktop-target-ready = 157615831096 / 19204 @E37:L3671<br>background-target-ready = 157623718306 / 27091 @E37:L3672<br>scanout-a-ready = 157631411605 / 34785 @E37:L3673<br>scanout-b-ready = 157650235688 / 53609 @E37:L3674<br>scene-target-ready = 157658651071 / 62024 @E37:L3675<br>targets-ready = 157672143734 / 75517 @E37:L3676<br>startup-font-begin = 157608295626 / 11669 @E37:L3677<br>fonts-ready = 157703190517 / 106564 @E37:L3678<br>scene-ready = 157668093209 / 71466 @E37:L3679<br>background-ready = 157672142133 / 75515 @E37:L3680<br>chrome-blit-ready = 157704359940 / 107733 @E37:L3681<br>chrome-taskbar-ready = 157708536692 / 111910 @E37:L3682<br>chrome-appbar-ready = 157708728163 / 112101 @E37:L3683<br>chrome-ready = 157708728752 / 112102 @E37:L3684<br>fills-ready = 157718294366 / 121668 @E37:L3685<br>pointer-presented = 157730010103 / 133383 @E37:L3686<br>checks-ready = 157782145716 / 185519 @E37:L3687<br>font-plan-begin = 157616448571 / 19822 @E37:L3688<br>font-plan-return = 157676263653 / 79637 @E37:L3689<br>font-submit-begin = 157676264870 / 79638 @E37:L3690<br>font-submit-return = 157701018892 / 104392 @E37:L3691<br>font-wait-return = 157703185870 / 106559 @E37:L3692<br>font-draw-return = 157708531926 / 111905 @E37:L3693<br>frame-check-begin = 157730012784 / 133386 @E37:L3694<br>frame-buffers-ready = 157733382171 / 136755 @E37:L3695<br>frame-scanout-return = 157744132928 / 147506 @E37:L3696<br>frame-desktop-return = 157745188331 / 148562 @E37:L3697<br>frame-pixels-ready = 157748335154 / 151708 @E37:L3698<br>initial-windows-ready = 157718313735 / 121687 @E37:L3699<br>title-plan-begin = 157718376715 / 121750 @E37:L3700<br>title-plan-return = 157718386378 / 121760 @E37:L3701<br>client-chrome-ready = 157795868630 / 199242 @E37:L3702<br>client-compose-begin = 157795962999 / 199336 @E37:L3703<br>desktop-ready = 157782150426 / 185524 @E37:L3704<br>first-composite = 157996149690 / 399523 @E37:L3705 |
| E37 / 879 | compositor-exec = 159194897094 / 0 @E37:L3706<br>config-ready = 159195216334 / 319 @E37:L3707<br>context-begin = 159237793841 / 42896 @E37:L3708<br>context-ready = 159238852514 / 43955 @E37:L3709<br>desktop-target-ready = 159207111246 / 12214 @E37:L3710<br>background-target-ready = 159214918074 / 20020 @E37:L3711<br>scanout-a-ready = 159222850166 / 27953 @E37:L3712<br>scanout-b-ready = 159231505715 / 36608 @E37:L3713<br>scene-target-ready = 159237783163 / 42886 @E37:L3714<br>targets-ready = 159249167263 / 54270 @E37:L3715<br>startup-font-begin = 159199127570 / 4230 @E37:L3716<br>fonts-ready = 159291268476 / 96371 @E37:L3717<br>scene-ready = 159244467128 / 49570 @E37:L3718<br>background-ready = 159249166547 / 54269 @E37:L3719<br>chrome-blit-ready = 159291809181 / 96912 @E37:L3720<br>chrome-taskbar-ready = 159295845154 / 100948 @E37:L3721<br>chrome-appbar-ready = 159296039331 / 101142 @E37:L3722<br>chrome-ready = 159296039923 / 101142 @E37:L3723<br>fills-ready = 159307101865 / 112204 @E37:L3724<br>pointer-presented = 159315392951 / 120495 @E37:L3725<br>checks-ready = 159363043753 / 168146 @E37:L3726<br>font-plan-begin = 159211339189 / 16442 @E37:L3727<br>font-plan-return = 159254302623 / 59405 @E37:L3728<br>font-submit-begin = 159254303336 / 59406 @E37:L3729<br>font-submit-return = 159287567111 / 92670 @E37:L3730<br>font-wait-return = 159291264884 / 96367 @E37:L3731<br>font-draw-return = 159295841026 / 100943 @E37:L3732<br>frame-check-begin = 159315394771 / 120497 @E37:L3733<br>frame-buffers-ready = 159318820642 / 123923 @E37:L3734<br>frame-scanout-return = 159329129480 / 134232 @E37:L3735<br>frame-desktop-return = 159327941503 / 133044 @E37:L3736<br>frame-pixels-ready = 159332157515 / 137260 @E37:L3737<br>initial-windows-ready = 159307135833 / 112238 @E37:L3738<br>title-plan-begin = 159307188282 / 112291 @E37:L3739<br>title-plan-return = 159307203521 / 112306 @E37:L3740<br>client-chrome-ready = 159371534831 / 176637 @E37:L3741<br>client-compose-begin = 159371684624 / 176787 @E37:L3742<br>desktop-ready = 159363047888 / 168150 @E37:L3743<br>first-composite = 159395651763 / 200754 @E37:L3744 |
| E38 / 718 | compositor-exec = 243766551666 / 0 @E38:L1952<br>config-ready = 243766657166 / 105 @E38:L1953<br>context-begin = 244864139458 / 1097587 @E38:L1954<br>context-ready = 244864143916 / 1097592 @E38:L1955<br>desktop-target-ready = 243829445833 / 62894 @E38:L1956<br>background-target-ready = 243833520583 / 66968 @E38:L1957<br>scanout-a-ready = 243889817541 / 123265 @E38:L1958<br>scanout-b-ready = 243993273333 / 226721 @E38:L1959<br>scene-target-ready = 244148225333 / 381673 @E38:L1960<br>targets-ready = 245019849291 / 1253297 @E38:L1961<br>startup-font-begin = 243814814833 / 48263 @E38:L1962<br>fonts-ready = 245745857666 / 1979306 @E38:L1963<br>scene-ready = 244929156125 / 1162604 @E38:L1964<br>background-ready = 245019847541 / 1253295 @E38:L1965<br>chrome-blit-ready = 245746429666 / 1979878 @E38:L1966<br>chrome-taskbar-ready = 245749888833 / 1983337 @E38:L1967<br>chrome-appbar-ready = 245750594250 / 1984042 @E38:L1968<br>chrome-ready = 245750595041 / 1984043 @E38:L1969<br>fills-ready = 245765951208 / 1999399 @E38:L1970<br>pointer-presented = 245767284958 / 2000733 @E38:L1971<br>checks-ready = 245913216666 / 2146665 @E38:L1972<br>font-plan-begin = 245722863333 / 1956311 @E38:L1973<br>font-plan-return = 245725137291 / 1958585 @E38:L1974<br>font-submit-begin = 245725138041 / 1958586 @E38:L1975<br>font-submit-return = 245744081500 / 1977529 @E38:L1976<br>font-wait-return = 245745850333 / 1979298 @E38:L1977<br>font-draw-return = 245749879583 / 1983327 @E38:L1978<br>frame-check-begin = 245767288791 / 2000737 @E38:L1979<br>frame-buffers-ready = 245770488333 / 2003936 @E38:L1980<br>frame-scanout-return = 245804605625 / 2038053 @E38:L1981<br>frame-desktop-return = 245797105916 / 2030554 @E38:L1982<br>frame-pixels-ready = 245807942166 / 2041390 @E38:L1983<br>initial-windows-ready = 245765971500 / 1999419 @E38:L1984<br>title-plan-begin = 245766037208 / 1999485 @E38:L1985<br>title-plan-return = 245766064541 / 1999512 @E38:L1986<br>client-chrome-ready = 245914705458 / 2148153 @E38:L1987<br>client-compose-begin = 245914763666 / 2148212 @E38:L1988<br>desktop-ready = 245913223416 / 2146671 @E38:L1989<br>first-composite = 247459383708 / 3692832 @E38:L1990 |
| E38 / 731 | compositor-exec = 249402620958 / 0 @E38:L1991<br>config-ready = 249402708083 / 87 @E38:L1992<br>context-begin = 250375775666 / 973154 @E38:L1993<br>context-ready = 250375779041 / 973158 @E38:L1994<br>desktop-target-ready = 249413140666 / 10519 @E38:L1995<br>background-target-ready = 249418438250 / 15817 @E38:L1996<br>scanout-a-ready = 249421546958 / 18926 @E38:L1997<br>scanout-b-ready = 249426115833 / 23494 @E38:L1998<br>scene-target-ready = 249429284916 / 26663 @E38:L1999<br>targets-ready = 250395427625 / 992806 @E38:L2000<br>startup-font-begin = 249409728291 / 7107 @E38:L2001<br>fonts-ready = 250526751625 / 1124130 @E38:L2002<br>scene-ready = 250381153125 / 978532 @E38:L2003<br>background-ready = 250395423416 / 992802 @E38:L2004<br>chrome-blit-ready = 250527333208 / 1124712 @E38:L2005<br>chrome-taskbar-ready = 250530858166 / 1128237 @E38:L2006<br>chrome-appbar-ready = 250531328625 / 1128707 @E38:L2007<br>chrome-ready = 250531329333 / 1128708 @E38:L2008<br>fills-ready = 250545806666 / 1143185 @E38:L2009<br>pointer-presented = 250547087958 / 1144467 @E38:L2010<br>checks-ready = 250690470083 / 1287849 @E38:L2011<br>font-plan-begin = 250060462000 / 657841 @E38:L2012<br>font-plan-return = 250485823708 / 1083202 @E38:L2013<br>font-submit-begin = 250485825041 / 1083204 @E38:L2014<br>font-submit-return = 250524560166 / 1121939 @E38:L2015<br>font-wait-return = 250526745791 / 1124124 @E38:L2016<br>font-draw-return = 250530850166 / 1128229 @E38:L2017<br>frame-check-begin = 250547092958 / 1144472 @E38:L2018<br>frame-buffers-ready = 250551262416 / 1148641 @E38:L2021<br>frame-scanout-return = 250585067625 / 1182446 @E38:L2022<br>frame-desktop-return = 250585214958 / 1182594 @E38:L2023<br>frame-pixels-ready = 250586681958 / 1184061 @E38:L2024<br>initial-windows-ready = 250545839666 / 1143218 @E38:L2025<br>title-plan-begin = 250545899166 / 1143278 @E38:L2026<br>title-plan-return = 250545954583 / 1143333 @E38:L2027<br>client-chrome-ready = 250712037041 / 1309416 @E38:L2028<br>client-compose-begin = 250712776666 / 1310155 @E38:L2029<br>desktop-ready = 250690474041 / 1287853 @E38:L2030<br>first-composite = 250713755666 / 1311134 @E38:L2031 |
| E39 / 723 | compositor-exec = 257966586541 / 0 @E39:L2025<br>config-ready = 257966679875 / 93 @E39:L2026<br>context-begin = 259017713375 / 1051126 @E39:L2027<br>context-ready = 259017717958 / 1051131 @E39:L2028<br>desktop-target-ready = 257975102875 / 8516 @E39:L2029<br>background-target-ready = 257984604416 / 18017 @E39:L2030<br>scanout-a-ready = 257987686458 / 21099 @E39:L2031<br>scanout-b-ready = 257990794416 / 24207 @E39:L2032<br>scene-target-ready = 257995790625 / 29204 @E39:L2033<br>targets-ready = 259394989500 / 1428402 @E39:L2034<br>startup-font-begin = 257971964083 / 5377 @E39:L2035<br>fonts-ready = 259468879958 / 1502293 @E39:L2036<br>scene-ready = 259069551166 / 1102964 @E39:L2037<br>background-ready = 259394987750 / 1428401 @E39:L2038<br>chrome-blit-ready = 259664298916 / 1697712 @E39:L2039<br>chrome-taskbar-ready = 259714213125 / 1747626 @E39:L2040<br>chrome-appbar-ready = 259715645916 / 1749059 @E39:L2041<br>chrome-ready = 259715647541 / 1749061 @E39:L2042<br>fills-ready = 259973035333 / 2006448 @E39:L2043<br>pointer-presented = 259975249458 / 2008662 @E39:L2044<br>checks-ready = 260118560750 / 2151974 @E39:L2045<br>font-plan-begin = 257992071875 / 25485 @E39:L2046<br>font-plan-return = 258895062791 / 928476 @E39:L2047<br>font-submit-begin = 258895064416 / 928477 @E39:L2048<br>font-submit-return = 259467124208 / 1500537 @E39:L2049<br>font-wait-return = 259468872916 / 1502286 @E39:L2050<br>font-draw-return = 259714204041 / 1747617 @E39:L2051<br>frame-check-begin = 259975251833 / 2008665 @E39:L2052<br>frame-buffers-ready = 259978687916 / 2012101 @E39:L2053<br>frame-scanout-return = 260005919458 / 2039332 @E39:L2054<br>frame-desktop-return = 260013207625 / 2046621 @E39:L2055<br>frame-pixels-ready = 260014688583 / 2048102 @E39:L2056<br>initial-windows-ready = 259973054250 / 2006467 @E39:L2057<br>title-plan-begin = 259973114916 / 2006528 @E39:L2058<br>title-plan-return = 259973141916 / 2006555 @E39:L2059<br>client-chrome-ready = 260119892750 / 2153306 @E39:L2060<br>client-compose-begin = 260122921583 / 2156335 @E39:L2061<br>desktop-ready = 260118564666 / 2151978 @E39:L2062<br>first-composite = 261685953250 / 3719366 @E39:L2063 |
| E39 / 736 | compositor-exec = 264222406416 / 0 @E39:L2064<br>config-ready = 264222632208 / 225 @E39:L2065<br>context-begin = 265274083250 / 1051676 @E39:L2066<br>context-ready = 265274086625 / 1051680 @E39:L2067<br>desktop-target-ready = 264234998125 / 12591 @E39:L2068<br>background-target-ready = 264239729458 / 17323 @E39:L2069<br>scanout-a-ready = 264243170041 / 20763 @E39:L2070<br>scanout-b-ready = 264246619416 / 24213 @E39:L2071<br>scene-target-ready = 264252620458 / 30214 @E39:L2072<br>targets-ready = 265393298166 / 1170891 @E39:L2073<br>startup-font-begin = 264229690000 / 7283 @E39:L2074<br>fonts-ready = 265826110958 / 1603704 @E39:L2077<br>scene-ready = 265324597541 / 1102191 @E39:L2078<br>background-ready = 265393296625 / 1170890 @E39:L2079<br>chrome-blit-ready = 265946889625 / 1724483 @E39:L2080<br>chrome-taskbar-ready = 266093529291 / 1871122 @E39:L2081<br>chrome-appbar-ready = 266094522000 / 1872115 @E39:L2082<br>chrome-ready = 266094523125 / 1872116 @E39:L2083<br>fills-ready = 266237524875 / 2015118 @E39:L2084<br>pointer-presented = 266241112500 / 2018706 @E39:L2085<br>font-plan-begin = 264252951958 / 30545 @E39:L2086<br>font-plan-return = 265148281208 / 925874 @E39:L2087<br>font-submit-begin = 265148282416 / 925876 @E39:L2088<br>font-submit-return = 265823081583 / 1600675 @E39:L2089<br>font-wait-return = 265826104625 / 1603698 @E39:L2090<br>font-draw-return = 266093520791 / 1871114 @E39:L2091<br>frame-check-begin = 266241118916 / 2018712 @E39:L2092<br>frame-buffers-ready = 266243992833 / 2021586 @E39:L2093<br>frame-scanout-return = 266278149083 / 2055742 @E39:L2094<br>frame-desktop-return = 266278142083 / 2055735 @E39:L2095<br>frame-pixels-ready = 266279569458 / 2057163 @E39:L2096<br>initial-windows-ready = 266237554166 / 2015147 @E39:L2097<br>title-plan-begin = 266237619250 / 2015212 @E39:L2098<br>title-plan-return = 266237669958 / 2015263 @E39:L2099 |

## Exit-stage table

Searched the retained serials for `wm-exit-phase`, `wm-reap`, and raw five-column pipeline rows whose stage is 17–22. No such row is present in these captures. The source writes stages into a guest ring; a successful reap does not export that ring (`userspace/programs/src/wm_suite/desktop.rs:93–116`). Thus the passing 403f5501 capture does **not** contain the six timestamps the brief expected. It contains a kill log and census, which carry no monotonic stage time. Its relevant source excerpt is retained below; the serial SHA-256 above pins the negative-search input.

| Capture | Stages 17–22 / kill-request-return / reaped timestamp |
| --- | --- |
| E01 | not exported |
| E02 | not exported |
| E03 | not exported |
| E04 | not exported |
| E05 | not exported |
| E06 | not exported |
| E07 | serial absent |
| E08 | not exported |
| E09 | not exported |
| E10 | not exported |
| E11 | not exported |
| E12 | not exported |
| E13 | not exported |
| E14 | not exported |
| E15 | not exported |
| E16 | not exported |
| E17 | not exported |
| E18 | not exported |
| E19 | not exported |
| E20 | not exported |
| E21 | not exported |
| E22 | not exported |
| E23 | not exported |
| E24 | not exported |
| E25 | not exported |
| E26 | not exported |
| E27 | not exported |
| E28 | not exported |
| E29 | not exported |
| E30 | not exported |
| E31 | not exported |
| E32 | not exported |
| E33 | not exported |
| E34 | not exported |
| E35 | not exported |
| E36 | not exported |
| E37 | not exported |
| E38 | not exported |
| E39 | not exported |

## Line 185 instrumented measurement (2026-11-11)

At 30b39d0a, normal build and disk staging preceded the headless HVF/SPICE GL
measurement `20261111T180908Z-qemu-89034`. The built suite and disk copy were
byte-identical (SHA-256 `79dcc01b012df95211610b7d6b2a408c2260afc090843fcc4a68ca247895c490`),
with the five required label/format strings. The boot kernel SHA-256 was
`47b0fcdb92f2c1c158a95f0e7c2996b6a73b2358d3b8e51f0c4f9dfd2dc5a636` and contained
`retained-writer-fallback`; disk SHA-256 was
`a9268c386f9f4cfdbec1796fd76ce41dd4f583ff449672c34dfb71bf8ad8e89f`.
The serial SHA-256 is `190e7ee14aeb33721ce80013fdf95dcccc3c85939313ac899bf1f93e847348c5`; the load file is retained alongside the Vigil capture. The
sampler ran at two-second intervals, with one-minute load ranging 4.62–8.24;
its first sample was 6.53. Guest monotonic timestamps and host load samples have
no recorded shared epoch mapping, so exact case-specific loads cannot be asserted.

The serial contains the documented export families, including observer waits,
readback, buffer counts, startup and detail, reap and exit phases, completion,
and frame endpoints. The suite reported 78/81: all-minimized failed its 1000 ms
click acknowledgment; stack-restored and supervisor-restarts hit the outer 10000 ms
watchdog. Restart-adopts and frame-floor passed. MEMORY reclaimed 10 pages
(allowance 48), READY followed DONE and MEMORY, and ANGLE/Metal was accepted.
This is measurement evidence, not an acceptance verdict.

- **Restart-adopts:** PID 708 kill returned at 263790665375 ns and wait returned
  at 263795930541 ns, 5.265166 ms against 500 ms, margin 494.734834 ms. Leader
  TID 924 entered stage 17 at 263791795208 ns, 1.129833 ms after kill returned.
  Other exported group stages include prior thread exits, so they cannot be
  assumed to have been alive at kill. No captured thread syscall/wait state
  explains the earlier stale boot's 893 ms pre-stage-17 interval; that cause and
  its race/slow-path classification remain unknown.
- **Supervisor-restarts:** client 83 commit 4 observer wait ran from
  292907243708 to 294914370416 ns, 2007.126708 ms against 2000 ms, margin
  -7.126708 ms. Refusals include claim-baseline, claimant 801, epoch 133,
  first/current serial 2315, busy 0 and completed 2315, ack 0. This identifies
  the admission branch, not why the replacement had not advanced the serial.
  PID 801 exec was 292903369791 ns; targets ready at 1497.490 ms and fonts ready
  at 1605.140 ms after exec. The outer watchdog killed the case while diagnostics
  were still being emitted; its complete observer export and eventual first
  client receipt are unavailable. No causal attribution to a particular startup
  phase or diagnostic output cost follows from these observations alone.
- **Frame-floor:** client 120 interval 419776255250–424776269125 ns had first
  419778058000 and last 424775316250 ns. Endpoint gaps were 1.802750 ms and
  0.952875 ms, giving margins 14.863917 ms and 15.713792 ms against 16.666667 ms.
  3113 intervals in 4997.258250 ms measured 622 Hz against 60 Hz. This one passing
  interval does not establish the cause of the earlier endpoint failure.
- **Stack-restored:** PID 756 reap took 0.004917 ms after kill return; the next
  desktop observer 281703683166–284329208458 ns passed and exported completion
  receipts for client 81. The 10000 ms case watchdog interrupted subsequent
  diagnostic output. This ARM observation does not identify the mechanism of
  the earlier native EAGAIN signature; shared causality is unproven.
- **All-minimized:** the click was unacknowledged within 1000 ms. The capture
  does not establish why; a pass/fail timing margin cannot be inferred.

The measurement therefore does not establish the mechanisms required by line
176. In particular, the question remains which wait or syscall held each live
bwm thread at SIGKILL in a delayed reap, and why PID 801 remained at its claim
baseline until the observer deadline. Establish a clock correlation for host
load and bounded diagnostic export that survives the outer watchdog before
claiming case-specific loads or a cause. A forcing regression check, repair, acceptance boot, board update or native boot
is not justified by this measurement.
