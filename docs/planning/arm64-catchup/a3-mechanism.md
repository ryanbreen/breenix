# A.3 recovery mechanism analysis — a3-u10-mechanism

## Coordinator 221 measurement at a3996d06: lock contention observed, acceptance blocked

The single measurement is Vigil `20261111T231422Z-qemu-21633`, at clean runtime
head `a3996d060fc6b9b1bb18e1d8af1f7bcf4613a06d`. The unchanged 21.7 selection
scorer reports **79/81 FAIL**: recovery/stack-restored and
recovery/supervisor-restarts each hit the 10000 ms outer watchdog. This is a
measurement, not a proof. The board cell is unchanged. No runtime repair, proof
boot, R240 boot or merge follows: the per-stall mechanism required by coordinator
line 221 remains incomplete.

### Build, artifacts and transport

Freshly fetched main `ac3cdf6c` was already an ancestor; no merge was needed.
The normal userspace, disk and production ARM kernel build ran through
`boot-interactive.sh`, without `--no-build`, with four jobs and background build
policy. It passed, with the existing G35(2) pinned-core advisory and no other
compiler warnings/errors; the FP/SIMD guard reported 0 forbidden/32 allowlisted.
The earlier pre-execution failure at ledger 224 had not executed QEMU. This
attempt's local gate corrected its architecture error: the two GPU journal labels
are required in ARM only, matching `window/mod.rs:23-24`; the preceding shared
x86 label checks are retained. No repository check was changed.

The pre-exec gate checked built/disk-resident suite equality, the existing labels
and u20 labels, head identity, headless SPICE GL, HVF, tracing, and absence of a
debugger. It recorded these SHA-256 values and the prediction in the ARM ledger
before executing QEMU:

| Artifact | SHA-256 |
| --- | --- |
| Suite, also identical on the boot disk | `c2132c520a01970fb662bdf1f43af7b7c3e45e8627b09a72f79f6da82426cca9` |
| ARM kernel | `86d622f573af4504daa97374f0f4799b02d5840468bdfe57420946819261ba69` |
| Disposable disk before execution | `d857a2c8b42c5fe3467e46c06ee1a8b4a91a4c9082d3f3e36bc93005ad81f6da` |
| Previously built x86 kernel checked by the gate | `634df8b8b1e74a462ef2d7778e5ee13f336ce7908075eb9deedc0829c23f064b` |
| ARM client | `d978abff9335fe68e44a3031f00db3eb835f446a968a8274a56bc437e042dc0c` |

Load1 was 3.92 at the gate (0.006 s wait). The two-second load sampler covered
the boot, linked in Vigil as `host-load.jsonl`. Subsequent samples range
4.33–60.46; recovery case logic intervals have ranges 10.64–11.05
(clients-notified), 9.77–10.11 (restart-adopts), 9.62–10.02
(interaction-after-restart), 9.29–9.75 (stack-restored), and 9.68–9.99
(supervisor-restarts). The transient 60.46 sample was outside recovery. No lane
build or heavy work ran during QEMU; the load history does not identify competing
work or establish descheduling.

Before causal analysis, the timestamped GPU records and successful nonempty
recovery sample call graphs were confirmed and recorded in the ledger. Final
trace counts include 92941 context submissions, 219291 fence-control and 219291
fence-response events, plus 74038 read handles, 87230 read/write completions,
87488 request completions and 175437 MMIO IRQ records. Eight case samples exited
0, for the three click cases and five recovery cases. The selected executable
has no `virgl*` family or generic command-finish event; its `virtio_gpu_*` events
cover the context-submit path. This satisfies line 221's (a)/(b) transport gate;
it does not supply the missing guest attribution described below.

DONE is 79 passed/2 failed/0 skipped/81 total. MEMORY reclaimed passes at
10 pages against 48, and standalone READY follows DONE and MEMORY (U19).
The launcher's renderer check accepted ANGLE/Metal, 987 records. Vigil records
clean commit a3996d06, ended, exit status 1, display=false, architecture aarch64;
its serial is byte-identical to the local capture. No window or debugger opened.

Final capture hashes:

- Serial: `18c6801bcc76606e7bacbbf36d964c40918e5630edfd96e89102bcb9aaa2b04a`.
- QEMU trace: `11780b382266909b38c1e2c7421961907ade5b024d533447ab50f1da8c784989`.
- Serial receipt-clock sidecar: `acc2a1dc6e3ba0502720583dd19e861b48a9586257e44c243aec8f081ca53875`.
- Load: `e12d1c139e78fab2d63df171d1d5958dd10fab799c31d25c7ccbd11264a26122`.

The gate, raw files and analysis intermediates are retained locally under
`/tmp/a3-acceptance-221-a3996d06`, with trace, samples, load and gate linked from
Vigil. Raw host sample headers are not committed.

### What the stacks establish, and what they cannot establish

The vCPU host threads are 97621931–97621934. QEMU's loop is thread 97621985;
the macOS application main thread is 97621907. The vCPU stacks explicitly contain
`hvf_cpu_thread_fn -> hvf_vcpu_exec -> bql_lock_impl -> qemu_mutex_lock_impl ->
_pthread_mutex_firstfit_lock_wait -> __psynch_mutexwait`. These are observed
waits for QEMU's global lock, not merely an inference from a trace gap.
The loop's graphics stacks contain `virtio_gpu_gl_handle_ctrl ->
virtio_gpu_process_cmdq -> virtio_gpu_virgl_process_cmd ->
vrend_decode_ctx_submit_cmd -> vrend_draw_vbo`. Its long branches include
`vrend_select_program -> add_shader_program -> GL_GetProgramiv ->
gl::Program::resolveLinkImpl -> MainLinkLoadEvent::wait`, and shader-compilation
waits. This supports the coordinator's GL/global-lock hypothesis during the
sampled windows, rather than explaining those observed lock waits solely as
vCPU descheduling.

The counts below are aggregate sampled stacks, **not milliseconds**, lock-held
durations, or a synchronized per-stall timeline. Split rows for the same loop
thread/dispatch queue are combined. The reports carry no mutex-owner identity
or per-stack timestamp. They do not identify which thread owned BQL at each gap.

| Case | Snapshots per vCPU | BQL waits, threads 31/32/33/34 (suffixes above) | Loop VirGL snapshots | Loop program-link / shader-compile waits |
| --- | ---: | --- | ---: | --- |
| stacking/click-raises | 97 | 46 / 20 / 12 / 6 | 75 | 49 / 4 |
| stacking/all-minimized | 95 | 52 / 4 / 14 / 4 | 76 | 42 / 8 |
| focus/pair-click | 108 | 42 / 29 / 15 / 5 | 82 | 49 / 11 |
| recovery/clients-notified | 92 | 42 / 6 / 29 / 1 | 52 | 28 / 5 |
| recovery/restart-adopts | 97 | 61 / 25 / 15 / 7 | 74 | 38 / 6 |
| recovery/interaction-after-restart | 124 | 84 / 18 / 9 / 5 | 101 | 63 / 11 |
| recovery/stack-restored | 98 | 57 / 14 / 5 / 14 | 78 | 44 / 8 |
| recovery/supervisor-restarts | 98 | 76 / 22 / 41 / 12 | 90 | 48 / 10 |

Crucially, sampling starts at CASE START for three seconds, while these recovery
cases create a second compositor later. Stack-restored's sample child stops at
host 1794439449.062569, but its replacement exec is bounded by
1794439450.053318125–1794439450.053330625, already 990.749–990.762 ms later.
Supervisor's sample child stops at 1794439473.052439, while replacement exec is
1794439474.935997667–1794439474.936008250, 1883.559–1883.569 ms later. Sampling
itself ends before child exit/symbolication. Neither sample covers replacement
startup, so its initial-compositor stacks cannot be assigned to replacement stalls.

### Intervals over 100 ms

[The interval table](serials/a3-u21-lock-stalls.tsv) lists 102 trace gaps over
100 ms in the eight cases' logic intervals, and 28 retained read-request
submit-to-used intervals over 100 ms. These are two overlapping observations,
not 130 independent stalls. Trace gaps alone do not establish a guest stall or
its cause. Case membership uses the child budget stamps and nearby disk-derived
clock offsets; offsets drift by milliseconds across the boot, so a single
boot-wide fixed offset is not assumed. Click-raises uses the nearby
all-minimized window; this is approximate grouping, not a causal clock bound.
Killed cases stop at the 10 s watchdog, excluding later diagnostic output.

| Case | Trace gaps >100 ms | Largest trace gap, ms |
| --- | ---: | ---: |
| stacking/click-raises | 9 | 398.836 |
| stacking/all-minimized | 11 | 450.710 |
| focus/pair-click | 11 | 371.218 |
| recovery/clients-notified | 10 | 305.453 |
| recovery/restart-adopts | 14 | 459.246 |
| recovery/interaction-after-restart | 15 | 412.552 |
| recovery/stack-restored | 16 | 386.871 |
| recovery/supervisor-restarts | 16 | 985.906 |

For each row, the table states the preceding/following host events, last GPU
context/resource and size when the trace provides them, the measured interval,
and the attribution limits. The `sample_window` column only means enclosure by
the nominal three seconds after sample launch; it does not assign an aggregate
stack to that interval. Guest submitter and exact lock owner are marked unknown,
rather than substituted from the case name or an adjacent sample. Read rows
also give host handle-to-complete and guest IRQ-entry-to-used time. Retention
and split serial records prevent claiming an exhaustive inventory of guest stalls.

A particularly direct recurrence of u19's signature is request **67893**:

- Worker PID 759/TID 993, window 67, sector 173764, 4096 bytes.
- Guest submit 611258402833 ns; used completion 611532669708 ns:
  **274.266875 ms**. CPU 0 enters IRQ at 611258447000 ns, leaving
  **274.222708 ms** inside the handler before used completion.
- Host handle 1794439475.321463, completion 1794439475.321476:
  **0.013 ms**, then IRQ raise at .321480.
- Context `0x15b` submits 61572 bytes at .321510, then 19660 bytes at .321567.
  The next event is its type `0x207` fence-control at .595624:
  **274.057 ms** without trace events, followed by block IRQ clear at .595701.
- Sector/length ordered matching gives 228/228 reads in window 67 in monotonic order;
  causal clock-offset width is 10.583 microseconds. Each of the 28 long read
  rows has one retained device-0 IRQ entry between submit and used completion.

The handler code at `block_mmio.rs::handle_interrupt` still has status/ACK MMIO,
a barrier and ring reads, without a blocking lock or polling loop. The trace
localizes this pause to the graphics submit and delayed MMIO/used observation,
but there is no sample at .321–.595 to establish its lock owner. Nor is there a
guest command row identifying the GPU submitter. Context ID alone is insufficient.

### Why the command attribution is missing

Every exported GPU journal snapshot retains the same **six** commands,
`overwritten=0 lost=0`, spanning guest 43.787041–47.995833 ms, transport 1,
PID/TID 0/0, types 256, 257, 262, 259, 261 and 260. There is no retained type
`0x207` (519) or transport-2 command, despite 92941 host context-submit events.
This is not journal eviction.

The instrumentation is in `drivers/virtio/gpu_mmio.rs` and the legacy
`drivers/virtio/gpu_pci.rs` submission helpers. The active core path instead
publishes requests through `gpu/virgl/mod.rs::Device::send` (lines 399–416),
then uses `Device::doorbell` (449) and `gpu/mod.rs::ring_doorbell` (265).
`Device::send` has no `gpu_command_trace` call. The u20 presence checks and
artifact labels therefore did not exercise the path the suite runs.
A correct future observation needs attribution from the originating GPU request,
carried through core submission; recording only the service thread would not
identify its client. No instrumentation or runtime behavior is changed here.

### Deadlines and stop

- Both recovery watchdogs fail at 10000 ms. Supervisor replacement PID 758's
  userspace first-composite is **2201.362 ms** after exec, **201.362 ms beyond
  2000 ms**. Its client observer spans 2004.326958 ms and fails; first composite
  arrives another 190.677375 ms after that observer ends. These have different
  starting points and are not interchangeable deadlines. Font config-to-file
  takes 1962.101375 ms. The kernel exec-to-first-composite interval is
  2204.733291 ms.
- Restart-adopts reap is **16.976417 ms / 500 ms**, margin **483.023583 ms
  (96.60%)**. This one observation does not establish the earlier intermittent
  delay's cause.
- Animation passes at **15571 ms / 20000 ms**, only **4429 ms (22.145%)**
  headroom, below 25%. Its internal latency median/max are 17015/30076 us
  against 33000/100000 us. A passing case is not proof of the required margin.
- Frame-floor reports 2071 intervals over 4998306500 ns, **414 Hz** against
  60 Hz. Endpoint stamps are retained. The complete recovery and click margin
  contract still fails; this faster steady-state rate does not repair startup.
- Matched delivery-to-observer click intervals reach 183.227750 ms in pair-click
  and 185.809125 ms in interaction-after-restart. These exclude pre-delivery
  replay. `click-request` stores the replay sequence/buttons, whereas delivery
  and observed store window-event serials; equating those serials would invent
  an end-to-end margin. No end-to-end margin is claimed.

**Stop under coordinator 221.** The observed BQL waits and loop shader work
support that hypothesis in sampled initial-compositor windows. Its required
per-stall ownership, active guest command submitter and replacement-startup
coverage are unproven, so the full hypothesis is neither confirmed nor rejected.
Host descheduling cannot be excluded for unsampled intervals. A forcing check
for a chosen repair would encode an incomplete causal assumption.

The coordinator decision requested in the ledger is whether to authorize a new
observation unit and one measurement after covering the active core submission
path and replacement startup. Its specific question would be: during each long
replacement IRQ/MMIO interval, is the loop in the identified client's shader or
transfer command while the affected vCPU waits for BQL, or is that vCPU not being
scheduled? It needs interval-correlated stacks/ownership, not an aggregate sample
from before the replacement exists. No further boot is authorized by this result.
PR #2874 stays draft/unmerged; issue #2872 stays open; the ARM board is unchanged.

This continuation changes this analysis and its interval table only. No host
suite or build is repeated for documentation. Claim-lint and whitespace results
are recorded in the ledger with the report commit.


## Accounting measurement 20261111T202836Z-qemu-17664: completion latency located, cause unresolved

Coordinator line 198 authorized this one measurement. At launch, fetched main
was still `728da925`, already contained in clean runtime head `472b7fc1`.
The preceding `202257` launch exited before QEMU execution. Its wrapper was rejected
before execution; renaming that interpreter script would also fail the
launcher's Mach-O check. This measurement used an external Python command
shim to check the artifacts **before** the unchanged `qemu-arm64.py` and
`qemu-ui-launch.sh` executed the real GL QEMU. No launcher policy was bypassed.
The normal launcher built userspace, disk and production ARM kernel, with four
jobs and its background build policy; it did not use `--no-build`. The kernel
guard passed, with no compiler warnings apart from the previously documented
pinned-core future-compatibility advisory. No lane build ran during QEMU.

The immediate gate verified the disk suite byte-for-byte against the release
build, the line-185 and u13 stored labels, the u16 labels in both kernel ELFs,
and the client handling label. The prediction and these hashes were appended
to the ledger before QEMU execution:

| Artifact | SHA-256 |
| --- | --- |
| ARM kernel | `e2d0bd40bcef4b6f63a521ce7dc0b628e78e4078589cc165e383cbfae014628c` |
| Release suite and disk `/sbin/suite-window-manager` | `acfefbf0b31bc05022fcbc95b008cc72812b7bd8be1fe343044681067ed1a052` |
| Disposable disk before execution | `b81140fc9cb7224a909d73fb1fe9be6b435886cb84937e49c41bd34b28417b4e` |
| x86 dependency-artifact kernel | `faf9cfd330f1976044ea4f1d7c5a8436ed84a1bffe9e61e22f54f6c892256b58` |
| ARM client | `d978abff9335fe68e44a3031f00db3eb835f446a968a8274a56bc437e042dc0c` |
| Final serial, identical to Vigil's serial | `e06c55190e3f58e3c0b98c0c098d6333e3da84d2c4dac612329efdb8d0f5115a` |

The launch was headless HVF, four vCPUs, SPICE GL, without a window or debugger.
The load gate passed immediately at 5.79. The sampler retained 399 samples at
two-second intervals, covering build and boot, with one-minute load 4.26–8.12.
The load file, artifact gate and serial receipt-clock sidecar are discoverable
beside the Vigil capture. At receipt of the three failed CASE records, nearby
load samples were 5.60–5.95 (all-minimized), 7.76 (stack-restored), and 8.12
(supervisor-restarts). The supervisor observer export's receipt was bracketed
by load 7.86; its later phase export by 7.86–7.95. These are **receipt-time**
loads: buffered diagnostics and the watcher's polling interval prevent treating
them as exact loads at the embedded guest event time.

The selected 21.7 verdict is **78/81 FAIL**, with all 81 CASE records present.
The generic verdict has exactly the twelve planned-empty category refusals
permitted by line 143, but the selection has three real failures:

| Case | Measurement |
| --- | --- |
| stacking/all-minimized | No click acknowledgment within 1000 ms; case total 4745 ms. No success timestamp supplies a positive margin. |
| recovery/stack-restored | Outer watchdog at 10000 ms. Its client-81 observation exported `passed=false`; no accepted restored frame. |
| recovery/supervisor-restarts | Client 83 commit 4 observer 488636877166–490645970791 ns: 2009.093625 ms against 2000 ms, margin **-9.093625 ms**. Refusals are claim-baseline until startup advances. |

DONE reports 78 passed, 3 failed, 0 skipped; MEMORY reclaimed passes at 10 pages
against 48; standalone READY follows both. The unchanged host-renderer checker
accepted ANGLE/Metal with 984 records. Vigil has ended/exit-status 1 and the
complete serial; its cached summary still describes only 16 early cases, so
that stale summary is not a full-suite verdict. The existing selection scorer
was applied to the complete, unfiltered serial. Raw accounting families are
present, so this is not another missing-label or non-executed launch.

### Replacement window and file/request accounting

The [retained telemetry](serials/a3-u16-measurement.txt) includes window 67's
thread, wait, identity, file-call and disk rows, startup phases, and 81/81 CASE
outcomes. Its L references use normalized `splitlines()` indices, stated in
the index. Replacement PID 762's kernel exec boundary is 488628443666 ns and
first successful composite boundary is 490924054083 ns: **2295.610417 ms**.
The userspace marks are slightly inside those boundaries: exec 488630134791 ns,
first composite 490924052750 ns, or **2293.917959 ms**. Neither is substituted
for the observer's separate deadline interval.

The font read is observed, rather than inferred from the old request model:
worker owner PID 763/TID 997 enters read(63) at 488646199083 ns and returns
757076 bytes at 490694642125 ns, **2048.443042 ms**. The next read returns 0 bytes
in 1.241209 ms; close takes 0.000625 ms. The display-file open takes 6.714209 ms
and fstat 0.001833 ms. The earlier calls on this worker load the configuration.
Its config-ready→file-ready interval is **2056.434458 ms**. Planning then takes
4.886000 ms, submission 20.333000 ms, and fonts-ready is 2093.322875 ms after
the userspace exec mark. Main joins the font worker from 1290.759709 to
2093.371375 ms after that mark.

The whole kernel window has this elapsed-time partition:

| Consecutive interval | ms |
| --- | ---: |
| Kernel exec→font data-read entry, with parallel target/claim work | 17.755417 |
| Font data-read syscall | 2048.443042 |
| Read return→first successful composite, including remaining font/GPU/check work | 229.411958 |
| Total | 2295.610417 |

The 189 requests enqueued by TID 997 during that data-read syscall transferred
761856 bytes including metadata. Summing their nonoverlapping request stages
accounts for that syscall as follows; the residual includes work between
requests, copying and other syscall work, not an invented cache-hit count:

| Component | ms |
| --- | ---: |
| Enqueue→gate acquisition | 332.112746 |
| Gate acquisition→submit | 0.009043 |
| Submit→completion IRQ observation | 1710.236915 |
| IRQ observation→wait return | 3.724877 |
| Wait return→gate release mark | 0.232628 |
| Outside those request lifetimes | 2.126833 |
| Total read | 2048.443042 |

Across the whole window, that worker has 198 requests, 338.336369 ms gate
wait and 1712.363122 ms submit→IRQ. Competing **bterm PID 751/TID 982** has 17
requests, 1472.458208 ms gate wait and 333.842331 ms submit→IRQ. The gate delay
has a named predecessor: font request 68490 waits 331.243167 ms while bterm
request 68489 is in flight (its submit→IRQ is 277.020917 ms); bterm then has
further serialized requests before the font gets the gate. In the other
direction, bterm request 68467 waits 1124.229292 ms behind font requests.
The font's longest individual submit→IRQ is request 68472, **301.702916 ms**;
other font requests take 128.777250 and 124.574541 ms. This is not explained by
189 uniformly cheap transfers. The 198/198 font-worker request rows have the later wake,
resume and release stamps; missing timestamps are not filled in.

### Scheduler records and their limits

The journal reports four vCPUs, 6602 queue samples in window 67, and maximum
non-sentinel queue length four. The following are its **reported state
intervals**, in ms, not independently measured host execution time. Worker
roles follow the read result/startup source; exported `tgid`/names alone do not
group them under 762 (the font row says `tgid=763`, `clone-child-997`).

| PID/TID | On-CPU label | Runnable label | Blocked label | Wait breakdown |
| --- | ---: | ---: | ---: | --- |
| 762/995, leader | 338.942134 | 2.422115 | 1954.246168 | completion 7.708378; unnamed kind 8/object 0 **1946.537790** |
| 763/997, font | 941.482589 | 2.664755 | 1141.966406 | completion 799.830492; device gate 336.579125; unnamed 5.556789 |
| 764/998, startup worker | 57.980293 | 5.500076 | 1050.567256 | unnamed kind 8/object 0 |
| 765/999 | 0.074833 | 0.011209 | 0 | short-lived worker |
| 766/1000 | 19.292334 | 0.021541 | 14.971709 | unnamed kind 8/object 0 |
| 767/1001 | 19.585125 | 0.023667 | 14.902583 | unnamed kind 8/object 0 |
| 768/1002 | 5.154167 | 0.023958 | 30.425125 | unnamed kind 8/object 0 |
| 769/1003 | 12.374418 | 0.106624 | 20.639333 | unnamed kind 8/object 0 |

The leader sums exactly to 2295.610417 ms. Workers' intervals start at their
publication and end at termination; they must not each be extended to the full
window or added as consecutive wall time. Other named competing work reports
suite child 741/969: 1410.647189 ms on-CPU, 7.058637 runnable, 877.904591 blocked;
bterm 751/982: 342.685498/4.849587/1948.075332; kgpu 0/20:
153.303324/232.907012/1909.400081; render 0/11:
57.779189/48.743939/2189.087289. All exported identities and wait rows are
retained in the index. These small compositor runnable totals do not demonstrate
CPU starvation by that work.

There are material limits to calling this the requested physical CPU and
blocked-by-object accounting. Each of the four swapper rows reports the entire
2295.610417 ms as running, alongside the other threads. Including them yields
12546.724262 ms on-CPU against only **9182.441668 ms** of four-vCPU capacity.
`scheduler.rs` deliberately skips idle in outgoing state publication; the
journal observes `Thread::set_state`, rather than physical ownership boundaries
(`thread.rs`, `accounting_hooks.rs`). Idle CPU cannot be read literally from
these rows. Most of the leader's blocking is unnamed kind 8/object 0, so its
join/GPU waits cannot be partitioned by object from this export. Finally,
`accounting_hooks.rs` explicitly charges host vCPU descheduling and IRQ work
inside the current dispatch interval to that thread: an on-CPU label does not
separate actual guest work from host scheduling delay.

At the window-67 export, the fields are `lost=0 state_lost=0 disk_lost=0
unavailable=0 state_unavailable=0`. Historical event overwrites are 4523075 and
disk overwrites 64519; window 67's start/end and complete request sequence are
still retained. Other windows are absent, so global overwrite counts do not
establish startup coverage. The export-truncated marker count is 0. Producer
loss counters alone do not repair the accounting limits above.

### Other margins, variance, and the remaining decision

Restart-adopts reaped in 17.561875 ms, margin **482.438125 ms** against 500;
interaction-after-restart's reap was 5.418916 ms and stack-restored's reap
6.019875 ms. None explains the older 893 ms pre-exit delay. Frame-floor's
5000.542625 ms window has first/last gaps 1.632958/1.509542 ms, margins
15.033709/15.157125 ms against 16.666667 ms. All seven performance cases pass.
The failed click and stack-restored cases still prevent acceptance, whatever
their passing counterparts did in older captures.

**Measured lead:** disk-gate serialization and especially long submit→IRQ
observation intervals account numerically for this replacement's slow font
read. **Mechanism not established:** those stamps cannot distinguish device/
host queue service from delayed guest IRQ consumption. `block_mmio.rs` stamps
submit before publishing the ring and IRQ when software consumes the used
descriptor; it has no independent device-completed or IRQ-pending timestamp.
The retained 374498 µs maximum masked-IRQ record is from much earlier guest
time, not this window, and cannot explain request 68472. Nor do the exported
state labels identify named CPU starvation or the unnamed blocked objects.

This capture resembles F11's slow-read placement rather than F10's fast-read,
slow-plan/submit placement. It does **not** explain why the dominant phase
moved across the earlier 1.12–2.52 s captures: they lack these request stamps,
and this measurement lacks the independent completion/IRQ and physical CPU
boundaries needed to make that distinction. A cache resize, request concurrency
change, or startup reorder would therefore still select a hypothesis rather
than repair an established cause.

Under line 198(4), stop without a behavior fix, acceptance boot, R240 boot or board
cell update. The decision for the coordinator is whether to authorize repair
of the observation gaps and one specifically instrumented measurement:
**during a long request such as 68472, did QEMU complete the request promptly
while guest IRQ handling was delayed, or did host/backend service itself take
the interval?** The same capture needs physical dispatch ownership and named
join/GPU waits to account for the startup as required. No second boot is
authorized by this report. PR #2874 stays draft and unmerged; #2872 remains open.
Main advanced during the measurement and was merged conflict-free afterwards;
the measurement remains evidence for `472b7fc1`, not for the merged tree.

## A.3 u14 font trace: request serialization is identified; 1921 ms remains unexplained

This analysis answers coordinator ledger line 194(1–4) at source head
`7eafecbe` (runtime unchanged from `3d9201e8`). **The code and capture identify
repeated synchronous disk I/O and actual contention for the disk request gate.
They do not numerically explain the whole 1921.085125 ms interval.** No runtime
repair or boot is claimed. The remaining measurement is named below.

The [font evidence index](serials/a3-u14-font-trace.txt) retains disk inspection,
ELF symbols/disassembly and numbered raw serial rows. `F01`–`F11` name its
hashed captures; `F11:L40954` means physical LF line 40954 of
`20261111T190334Z-qemu-43440/serial.txt`, not line 40954 of the index. The earlier
capture inventory's E-keys remain unchanged. All times below use guest
monotonic stamps, not serial receipt time. <!-- claim-lint:ok: serials/a3-u14-font-trace.txt indexes 11 SHA256-pinned captures with physical LF source lines. -->

### (1) Display-font identity on the shipped image

`FontConfig::load` reads `/etc/fonts.conf` (or its configured root), parses
`display.font`, and falls back to `/usr/share/fonts/DejaVuSans.ttf` if loading
fails (`libs/breengel/src/font.rs:7,38–42,60,85–98`). The shipped image's
`/etc/fonts.conf` explicitly selects that same file, at display size 14.
The disk inspection in the evidence index reports inode **56687**, size
**757076 bytes**, ext2 block size **1024**, **740 data blocks** and **4 indirect
blocks** (1488 sectors including metadata). The last data block has 340 useful
bytes; the extent reader transfers 757760 data bytes, 684 beyond EOF internally,
and returns 757076. The raw inode/indirect walk matched the check's physical
map 740/740. Font SHA-256:
`7da195a74c55bef988d0d48f9508bd5d849425c1770dba5d7bfc6ce9ed848954`.

The inspected `target/ext2-aarch64.img` hashes
`feac6807e0e4838e2b86aa5f9b800b9268b8d7b130fb9127dcf77bb64e1dc610`,
matching the measurement's staging record above. The launcher copies it to a
writable image (`scripts/boot-interactive.sh:351–352,373`). These are shipped
image facts, not an inspection of the departed guest's RAM or writable copy.
The recovery setup has no font-config override or font-selection edit in
`userspace/programs/src/wm_suite`; both parsed disk configuration and the
fallback select this path. The snapshot itself has no pathname/FD argument.

### (2) From std::fs::read to ARM virtio-blk

The pinned Rust fork is `666252fd61839b9e9b1bc9e777a0c84c5cf3dc6d`
(`rust-fork.toml:5`). In that fork, `rust-fork/library/std/src/fs.rs:283–291`
opens the file, gets metadata, reserves its size, then calls
`default_read_to_end`. The sized vector supplies room for this whole file;
the loop's initial limit rounds size+1024 up to its buffer-size multiple
(`rust-fork/library/std/src/io/mod.rs:418–420,472–484`). With a stable regular
file and successful metadata, this is:

| File operation | ARM syscall | Content result |
| --- | --- | --- |
| open | openat, 56 | descriptor |
| metadata | fstat, 80 | size 757076 |
| read into sized vector | read, 63, count 757076 | 757076 bytes |
| EOF probe | read, 63, normally count 32 | 0 bytes |
| drop File | close, 57 | descriptor released |

Thus **two read syscalls, five file syscalls**, rather than one syscall per
sector. The EOF probe is at `io/mod.rs:424–440,455–464`; FileDesc's reads call
libc once (`rust-fork/library/std/src/sys/fd/unix.rs:96–104,175–188`), and
metadata calls fstat64 (`rust-fork/library/std/src/sys/fs/unix.rs:1204–1206`).
Breenix libc maps open/read/fstat64/close to the kernel
(`libs/libbreenix-libc/src/lib.rs:210–216,262–271,343–355,453–455`). This is the
code-derived normal pattern, not a measured syscall count: allocator syscalls,
EINTR retries, failed metadata, or a changing file could add calls. The marks
also enclose the configuration `print!` before open (`bwm.rs:165–168`).

The kernel path is:

- Namespace resolution/open goes through namei and ext2, installing a regular
  file handle, mount ID and position (`kernel/src/fs/namei.rs:31–37`,
  `kernel/src/syscall/fs.rs:332–350,428–436`,
  `kernel/src/ipc/fd.rs:73–78`). The generic VFS file API is not an intervening
  data-cache layer on this descriptor's read path.
- `sys_read` clones the handle/position and releases the process-manager lock,
  takes the root ext2 read guard, verifies the handle, reloads its inode, then
  calls `read_file_range_coherent` for the requested count; it copies the
  returned vector to userspace and advances the position
  (`kernel/src/syscall/handlers.rs:1373–1387,1418–1463`). No 4 KiB syscall
  cap is applied here. An inode reload reads the containing 512-byte sector
  through the mount cache (`kernel/src/fs/ext2/inode.rs:154–181`).
- `read_file_range_coherent` first reads the range from disk, then overlays any
  resident mapped pages (`kernel/src/fs/ext2/mod.rs:612–641`). The mapping
  cache therefore does **not** skip the underlying disk read. Ordinary read
  does not populate those file-mapping pages; `MapState::overlay` only copies
  pages already resident (`kernel/src/memory/file_map.rs:642–664`).
- The extent reader clips EOF, caches the single-indirect pointer array for
  that invocation, looks up double-indirect pointers, and groups consecutive
  data blocks into at most 64 KiB calls
  (`kernel/src/fs/ext2/file.rs:279–331,338–362`). On this disk's seven extents,
  the 14 run lengths in KiB are
  `16,16,32,64,64,64,64,64,64,64,64,64,64,36`, totalling 740 KiB.
- `read_ext2_blocks` converts to 512-byte sectors (`file.rs:31–52`). The
  mounted device first checks the sector cache; on a partial miss it rereads
  the **whole requested run**, not just its missing sectors
  (`kernel/src/fs/ext2/mount_device.rs:75–95`,
  `kernel/src/fs/ext2/sector_cache.rs:125–149`). The ARM wrapper calls
  `read_sectors` (`kernel/src/block/virtio.rs:197–205`).
- ARM virtio-blk splits a miss into transfers of at most 4096 bytes (or a
  smaller negotiated SIZE_MAX), takes a per-device exclusive request gate,
  submits one request, waits, copies the result, releases the gate, then
  proceeds to the next chunk (`kernel/src/drivers/virtio/block_mmio.rs:498,
  797–808,1057–1111`). There is **one in-flight request per device**, even
  between unrelated callers. Gate contenders sleep on its waitqueue
  (`block_mmio.rs:57–105`).
- Each submitted request synchronously waits for its completion, with a
  one-second failure-check slice, **not a mandatory one-second sleep**
  (`block_mmio.rs:405–427`). The block IRQ consumes the used ring and calls
  `completion.complete`; the completion primitive buffers the waiter wake
  for scheduler application (`block_mmio.rs:1362–1399`,
  `kernel/src/task/completion.rs:4–24,367–375`). IRQ arrival, wake processing
  and redispatch contribute to elapsed wait. No debugger was used.

**Request arithmetic, with conditions.** At a 4096-byte transfer limit,
`sum(ceil(run_KiB/4)) = 185` data requests. The production reader issues 945
indirect-block lookups: one single-indirect plus twice for each of 472
logical blocks beyond 267. With an initially empty, allocated cache and no
other access interleaving, those become **four** device requests for the four
unique pointer blocks, yielding **189 content-path requests**. An immediate
reread in the same isolated fixture makes **188** (185 data + 3 metadata).
The EOF content call makes **0**; sys_read still reloads the inode first.
The regular read adds at most one inode-sector read when that sector misses,
so its cold content+inode count is 190, before any relatime work. Open/fstat
can also read directory/inode metadata. Conditional relatime refresh can
acquire an exclusive mount guard and read/write the inode
(`handlers.rs:1454–1456`, `ext2/mod.rs:2854–2860,1110–1118`).

These are conditional counts, **not 189 measured requests in F11**. A hot
complete run can remove its data requests; concurrent eviction can increase
metadata misses, overlapping writes can prevent fills, and allocation failure
can remove the cache. Without cache, these same content operations issue
`185 + 945 = 1130` device requests at the 4 KiB limit. A smaller negotiated
transfer limit also increases counts; the retained initialization rows do not
export SIZE_MAX. The ring does not record actual request counts or cache hits.

### (3) What a restart can reuse, and what the snapshot identifies

A normal mount enables a **512 KiB**, 8-way set-associative LRU sector cache
(`kernel/src/fs/ext2/mod.rs:159–164,198`,
`kernel/src/fs/ext2/sector_cache.rs:25–28,45–65,110–149`). It is shared across
processes and survives the original bwm's exit. It holds 1024 sectors, whereas
this font's data alone spans 1480 sectors. Even without intervening traffic,
reading again from the beginning evicts later cached data before reaching it:
the production-code fixture gets no whole data-run hits on its immediate
second pass (185 data requests again). It is incorrect both to promise a
cache-resident restart and to say Breenix has no data cache here. The original's
parsed `Font` and Vec are process memory; the replacement creates fresh ones
and executes the same `std::fs::read` (`userspace/programs/src/bwm.rs:162–188`).
The original bwm's read does not leave a persistent parsed-font service.

Other intervening I/O includes executable/name lookup and other GUI clients'
font loads; bterm's window construction loads the system fonts
(`userspace/programs/src/bterm.rs:338`; `libs/breengel/src/font.rs:130–148,
190–197`). Cache access order, replacement, concurrent writes and gate
contention vary with scheduling. The capture has no cache census that would
assign a precise hit/miss count to F11's font read.

The snapshot is more specific than the earlier report stated:

| F11 row | Recorded state at the supervisor wait midpoint |
| --- | --- |
| L52813 | snapshot 279432301125 ns; wait began 278369891583 ns |
| L52821 | bterm owner PID 772 / TID 1003, syscall **79 = newfstatat**, BlockedOnIO, wait=completion, object `0xffff000040ee6bc0`, off-CPU age **26.718500 ms** |
| L52827 | replacement leader PID 781 / TID 1014 running in GPU syscall 416 |
| L52829 | worker owner PID **782** / TID **1016**, syscall 63, BlockedOnIO, wait=waitqueue, object `0xffff000040ee6c10`, off-CPU age **26.712708 ms**, no timer wake |

The PID 782 value is the raw row's owner PID, not a second replacement
compositor. Clone assigns a child owner PID and separately retains the thread
group (`kernel/src/syscall/clone.rs:478,541`); the old prose's “PID 781 TID 1016”
was shorthand for the group and not the literal snapshot. bterm's creation is
F11:L39603–39612. The snapshot lacks the read FD/path, but its read syscall and
startup placement are consistent with the font worker's source path.

The inspected kernel ELF hashes
`bf2c5a822cc849e57052e9a98ce4d97982be4fcb5cc8a826c393a23f812e702c`, matching
this boot. Its symbols identify `COMPLETIONS` at `0xffff000040ee6bc0` and
`REQUEST_GATES` at `0xffff000040ee6c10`. The retained gate disassembly shows
`x1 -> x20 -> x0` passed unchanged to `WaitQueueHead::prepare_to_wait`
(addresses `0xffff00004083b188,1d8,1e0`); the waitqueue is at gate offset zero.
These are device 0's objects; root/home mount waitqueues have different
addresses. `waitqueue.rs:78–83` publishes the object's own address, while
`completion.rs:373` publishes the completion address. This resolves the
font worker's wait to **request-gate contention**, with bterm on that device's
completion at the same snapshot. The producer that should release the font
worker is the preceding submitter finishing/copying its result and dropping
its gate guard (`block_mmio.rs:102–104,1104–1110`), ultimately after device IRQ
completion. A gate wake is distinct from a device completion wake. <!-- claim-lint:ok: docs/planning/arm64-catchup/serials/a3-u14-font-trace.txt retains symbols and the gate-pointer disassembly from the hash-matched ELF. -->

The 26.7 ms figures are recorded off-CPU ages, **not per-request service times
or syscall durations** (`kernel/src/task/thread.rs:2101–2115`). The header and
thread rows are sampled sequentially. The bterm completion timer is consistent
with the one-second completion slice; it does not establish that the hardware was
still unfinished rather than its wake not yet applied. No read-entry/return,
LBA, byte count, submit/IRQ/redispatch timestamps or gate-owner history is
exported for these I/O calls. The wm-readback ring's begin/end fields measure
SCANOUT_READBACK, not filesystem read or virtio-blk requests; using those
latencies for this font would measure another operation.

### (4) Arithmetic and today's startup variance

For the latest replacement (F11:L40917,L40954–40956,L40928,L40961):

```text
font config -> file = (280293164166 - 278372079041) / 1e6 = 1921.085125 ms
file -> parse       = (280293522250 - 280293164166) / 1e6 =    0.358084 ms
file -> fonts-ready = (280320528791 - 280293164166) / 1e6 =   27.364625 ms
exec -> config      = (278372079041 - 278360054583) / 1e6 =   12.024458 ms
exec -> fonts-ready = 12.024458 + 1921.085125 + 27.364625 = 1960.474208 ms
exec -> composite   = (280520028541 - 278360054583) / 1e6 = 2159.973958 ms
observer end -> composite = (280520028541 - 280375564458) / 1e6 = 144.464083 ms
```

The observer's measured interval is
`(280375564458 - 278369888166)/1e6 = 2005.676292 ms`
(F11:L40203); its arm timestamp differs by 3417 ns. The startup stamps therefore
locate the dominant interval and its consequence, but do not partition it:

```text
1921.085125 = config-print + open/metadata + allocation/copies/cache CPU
             + sum(font request gate waits)
             + sum(font completion/wake/redispatch waits)
             + conditional relatime + EOF/close + other off-CPU time
```

If 189 requests alone consumed that interval, their required average would be
**10.164472 ms/request** (188 gives 10.218538). Neither is a measured latency.
Subtracting even the observed gate off-CPU age leaves
`1921.085125 - 26.712708 = 1894.372417 ms` unallocated; this subtraction is an
illustration of the missing accounting, not a disjoint measured partition.
The worker's `cpu_ns=199254000` is cumulative lifetime CPU at the midpoint,
not CPU used between the two font marks. It cannot fill the missing terms.

Here are the retained supervisor-replacement timelines from today's ARM
captures with phase stamps, through F11. Durations are from absolute ns;
fonts-ready is relative to exec. Pre-plan includes read/parse and earlier
startup, including operations other than disk reads. Older captures without font stamps cannot
supply the file interval. The table excludes the native run and keeps original
versus replacement distinct in the accompanying indexed rows.

| Capture / replacement | fonts-ready ms | config→file ms | exec→plan ms | plan ms | submit ms | Raw citations |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| F01 120852 / 15 | 1360.123 | unavailable | 975.490 | 8.518 | 373.398 | L377,L388,L398–401 |
| F02 121843 / 837 | 1384.467 | unavailable | 991.551 | 8.905 | 381.616 | L1989,L2000,L2010–2013 |
| F03 124428 / 636 | 1384.265 | unavailable | 941.128 | 8.057 | 431.996 | L1872,L1883,L1893–1896 |
| F04 134512 / 665 | 1809.238 | unavailable | 29.063 | 1144.490 | 633.463 | L1892,L1903,L1913–1916 |
| F05 142945 / 728 | 2053.970 | unavailable | 1685.614 | 342.459 | 23.696 | L2056,L2067,L2078–2081 |
| F06 154755 / 731 | 1124.131 | unavailable | 657.841 | 425.362 | 38.735 | L1991,L2002,L2012–2015 |
| F07 161134 / 736 | 1603.705 | unavailable | 30.546 | 895.329 | 674.799 | L2064,L2077,L2086–2089 |
| F08 165116 / 728 | 2523.903 | unavailable | 2483.322 | 7.287 | begin absent | L2113,L2124,L2133,L2134,L2138 |
| F09 173538 / 720 | 2094.712 | 2052.695 | 2062.927 | 4.604 | 23.488 | L1959,L1970,L1979–1982,L1995–1996 |
| F10 180908 / 801 | 1605.141 | 17.221 | 26.639 | 899.685 | 632.237 | L41186,L41197,L41207–41210,L41223–41224 |
| F11 190334 / 781 | 1960.474 | 1921.085 | 1933.483 | 2.750 | 22.683 | L40917,L40928,L40938–40941,L40954–40955 |

The often-quoted 1.7–2.5 s is not the full range of these replacement stamps:
these records span **1124.131–2523.903 ms**, across different commits and
instrumentation. Font-ready also includes GPU atlas work (`bwm.rs:181–187`).
F09 was the previously identified stale-suite measurement, retained as evidence,
not proof of current instrumentation. Most decisively, F10's 17.220584 ms file
interval coexists with 899.684959 ms planning and 632.236709 ms submit, whereas
F11's planning/submit are 2.749500/22.682667 ms after a 1921.085125 ms file
interval. In F11 the original bwm's config→file was also **1962.267000 ms**
(F11:L40887–40888), not a uniquely cold replacement read.

Source-supported sources of variation are cache residency/interleaving,
competition for the single disk gate (observed), host/device/IRQ scheduling and
guest wake/redispatch, plus the separate font/GPU startup work. The fixed
request-count mechanism alone does **not** explain the run-to-run variance or
the 1921 ms. There is no measured correlation assigning the missing duration
to host load, cache misses, lost wakes or starvation. Today's older stamps
cannot retrospectively supply those measurements.

### Check and stop condition

Check-first commit `126a6dec` adds `tests/a3_font_trace_test.py`. It compiles
production `read_file_range`, block lookup/read helpers, and the sector cache's
read/replacement methods against this disk's pointer fixture. Its only device
substitution counts misses at a 4096-byte DMA boundary; it does not emulate
latency, scheduling or IRQ correctness. The cold/read-again/EOF assertions pass
on the existing implementation: **1/1 host check, rustc -Dwarnings**, under
`taskpolicy -b`, `CARGO_BUILD_JOBS=4`. This is a **U29 regression guard**, not a
red-first repair. Fixture extraction/comment/visibility errors were corrected
before that commit; no production code or expected count changed.

**Missing measurement / coordinator line 194 blocker:** one interval accounting
for this read's actual request count and cache hits, gate enqueue/acquire/release
with owner, request submit/IRQ/wake/redispatch, and file-syscall entry/return
(including open/stat/relatime). That would distinguish many ordinary serialized
requests from a few long device waits or wake/gate delays. The existing ring
resolves one wait identity and its waker path, not this accounting. No new boot
or speculative forcing test/fix follows from this analysis. Earlier delayed
reap and native stack-restored mechanisms remain open under issue #2872.

The unit changes this report, its retained evidence and the host check only.
Kernel/userspace artifacts were inspected, not rebuilt; the diff reaches no
runtime suite. No ARM or R240 boot, deployment, PR creation, rebase, force push
or merge was performed. The existing branch already contains fresh origin/main
`728da925`. Validation commands: `python3 tests/a3_font_trace_test.py` passed 1/1 before
the check-first commit; `python3 scripts/claim-lint.py --base 126a6dec` and
`git diff --check` exit 0 for this report/evidence change. No production
build or repeated host-test run is needed for the subsequent prose edits.

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
forcing check, behavior fix, proof boot, R240 boot or board update is justified. <!-- claim-lint:ok: docs/planning/arm64-catchup/serials/a3-u14-font-trace.txt F11:L52225 retains the ring counts; F11:L52813-L52829 retains the failed supervisor snapshot. -->

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

## a3-u22-sample-join — coordinator line 239, retained capture only

The retained capture confirms the **GL/BQL contention mechanism in sampled
replacement work**, rather than supporting a descheduling-only explanation of
those BQL stacks. It does not identify the originating guest command for the
four window-65 submits, and it does not provide host stacks during either
failed click wait or the frame-floor measurement. Some trace gaps have a
completed GPU command before them and require a different explanation. The
confirmation below concerns the mechanism; it is not attribution of the five
case failures to one cause or suite acceptance.

This analysis implements coordinator ARM ledger line 239(1)–(3), using
`20261112T000736Z-qemu-70293` at `c581c2b2`. The unit adds an offline analyzer,
its check and this report; it changes no kernel, guest program, launcher,
deadline or verdict. The unit performs no boot. Existing issues #2872 and
#2890 retain the unresolved runtime work.

### Inputs and clock interpretation

[The reproducible join](serials/a3-u22-sample-join.json) contains the 38
distinct trace gaps intersecting the five requested case/replacement intervals,
the narrower failed-wait/measurement joins, aggregate per-thread counts and
source line references. `S:L` below is physical LF line L in this capture's
`serial.txt`; `Q:L` is line L in `qemu-trace.log`; `J:L` is line L in
`qemu-samples/samples.jsonl`. Sample P/R/S/T line numbers refer to their raw
`.sample.txt` files. These inputs remain in the Vigil boot record; its
`qemu-samples`, trace and host-clock links resolve to the retained measurement.
Raw sample headers and their host paths are not copied into the repository.

| Input | SHA-256 |
| --- | --- |
| `serial.txt` | `6b11155144993a5a536eccf3993363465bf0a5b039246f78dd44ef06e0f40707` |
| `qemu-trace.log` | `bc36a91e8c208a337666294ceaf6499ae1ddb9a9a0b854549fe8b01f9e165eee` |
| `serial-host-clock.jsonl` | `0fd81ffd6712678238ffe8e0d5a1fff634d4abd14351a69d24a6cca80f6bc773` |

The host clock sidecar records **receipt**, not guest execution. For example,
pair-click's FAIL was received at 1794442414.271610 s (sidecar/S:L527022),
after the following case had already begun in guest time. Using that receipt
as the click-failure time would assign the wrong sample. The six receipt rows
and byte offsets are included in the JSON.

Instead, the join brackets a guest clock offset using matched block
submit/host-handle/host-complete/guest-used records, or exact wire fence IDs
shared by host `virtio_gpu_fence_ctrl/resp` and guest core command stamps.
For a fence, `host_response - guest_return <= offset <= host_control -
guest_submit`. Short requests bound the mapping without substituting their
service duration for a trace gap. The nearest endpoint anchor is an estimate
at that endpoint, not a constant boot-wide clock offset. The JSON gives the
source guest stamp and both offset bounds, allowing its distance to be computed; an anchor distant
from an endpoint is not a precise synchronization measurement there.

| Interval | Guest begin–end ns | Source | Host estimate, seconds minus 1794442000 | Anchor uncertainty widths, µs |
| --- | --- | --- | --- | --- |
| window-65 | 741425177375–743732410875 | S:L1066717 | 799.584289292–801.890981459 | 44.750 / 30.000 |
| window-67 | 777366017958–779932516458 | S:L1121021 | 835.525413625–838.091826500 | 41.750 / 298.334 |
| focus/pair-click | 340174592333–345596444125 | S:L506644 | 398.330403291–403.752417229 | 315.083 / 102.791 |
| focus/keys-to-focused | 354864261416–359963623666 | S:L528707 | 413.020349416–418.119728333 | 255.750 / 138.334 |
| performance/frame-floor | 1059198302625–1068686517208 | S:L1436078 | 1117.360016979–1126.847921000 | 53.125 / 913.416 |
| click-wait-461 | 344425664333–345428878416 | S:L506624 | 402.581710208–403.584961562 | 322.917 / 443.625 |
| click-wait-476 | 358710808916–359714885291 | S:L528683 | 416.867111104–417.871165020 | 765.208 / 456.459 |
| frame-floor-measurement | 1063256757375–1068256814750 | S:L1434681 | 1121.418471729–1126.418528625 | 53.125 / 63.750 |

Window 65 is replacement PID 722, 2307.233500 ms; window 67 is replacement
PID 751, 2566.498500 ms. The complete window-67 row at S:L1121021 supersedes
its earlier `complete=0` export at S:L1095258. The 38 gaps are adjacent events
in host timestamp order, strictly greater than 100 ms. They are gaps in the
enabled trace families, not an inventory of time in which the whole guest did
no work. Boundary-intersecting gaps retain their full length in the tables;
the subject table below identifies partial overlaps.

### What the covering reports say, with counts

P = `focus-pair-click.sample.txt`; R = `recovery-stack-restored.0002.sample.txt`;
S = `recovery-supervisor-restarts.0002.sample.txt`;
T = `recovery-supervisor-restarts.0003.sample.txt`.

| Report | Header time–sample child exit, host seconds minus 1794442000 | Nominal three-second end | Journal stop row | SHA-256 |
| --- | --- | --- | --- | --- |
| P | 398.583000000–401.966065000 | 401.583000000 | J:L6 | `200f6819c9578c08fcd7e9ba974b9d7dada7bb6dde8bb0d873e94c96c9ee8155` |
| R | 798.675000000–802.736189000 | 801.675000000 | J:L17 | `c2a96b386d4340f6d7bce6d96040d7e64820a04182af59c6274ffeff00205e80` |
| S | 833.082000000–837.685596000 | 836.082000000 | J:L37 | `b80f444fbb96143a14203815b28e98e6c3644434e0545eb81f3b82b789e34bbd` |
| T | 837.747000000–841.281459000 | 840.747000000 | J:L39 | `7677a403f58c92c5688a8423eb055a85a9154d8558411d5a73e2adb98c5610b2` |

The child exit includes symbolication. `L` in the gap tables means only
enclosure by that lifetime, extending beyond the nominal three-second sample;
`N` also fits the nominal three-second window. Neither is a per-stack timestamp.
The sample stderr explicitly says “Sampling completed, processing symbols”.
The sampler starts the next child only after reaping the preceding child
(`scripts/qemu-case-sample.py:90–150`), so back-to-back children do not establish
uninterrupted stack sampling. P has ended before the pair-click failure wait;
keys-to-focused and frame-floor are not sampler triggers
(`scripts/qemu-case-sample.py:13–16,62–65`).

The QEMU main loop is thread **98089364**, distinct from the application main
thread 98089322. Its disjoint classifications follow paths through
`qemu_main_loop -> main_loop_wait -> virtio_gpu_gl_handle_ctrl ->
virtio_gpu_process_cmdq -> virtio_gpu_virgl_process_cmd ->
vrend_decode_ctx_submit_cmd`. Link paths include `vrend_draw_vbo ->
vrend_select_program -> GL_GetProgramiv -> gl::Program::resolveLinkImpl ->
MainLinkLoadEvent::wait`. Compile includes ANGLE shader/Metal compilation
work or its wait; transfer includes `vrend_renderer_transfer_send_iov`.
Link/compile counts already beneath draw are classified once, not added to
their draw ancestor. “Other” includes select/poll and other loop work.

| Report | Loop stacks | Link | Compile | Draw excluding link/compile | Transfer | Other VirGL | Other | Raw leaf lines |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| P | 97 | 46 | 19 | 1 | 12 | 0 | 19 | L717–L887 |
| R | 93 | 45 | 14 | 0 | 12 | 1 | 21 | L744–L934 |
| S | 98 | 31 | 17 | 0 | 10 | 2 | 38 | L854–L1070 |
| T | 103 | 0 | 0 | 0 | 6 | 0 | 97 | L403–L469 |

The four vCPU host threads are 98089331–98089334. BQL rows contain
`hvf_cpu_thread_fn -> hvf_vcpu_exec -> bql_lock_impl -> qemu_mutex_lock_impl
-> _pthread_mutex_firstfit_lock_wait -> __psynch_mutexwait`. This identifies
the waiting lock; these reports do not record the mutex owner or stack times.
G means `hv_vcpu_run`/`hv_trap` in the guest execution path; it is not measured
guest instruction retirement. I is Hypervisor `wait_for_interrupt` or QEMU
idle/condition wait; O is other exit handling, MMIO or host work. There are
0 mutex-other classifications in these four reports. Counts are stack
observations, not milliseconds or fractions of the trace gap.

| Report | vCPU thread | Total | BQL | G | I | O | Raw leaf lines |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| P | 98089331 | 97 | 69 | 10 | 8 | 10 | L77–L102 |
| P | 98089332 | 97 | 22 | 10 | 65 | 0 | L112–L121 |
| P | 98089333 | 97 | 8 | 17 | 71 | 1 | L131–L143 |
| P | 98089334 | 97 | 25 | 13 | 59 | 0 | L153–L162 |
| R | 98089331 | 93 | 59 | 5 | 27 | 2 | L77–L113 |
| R | 98089332 | 93 | 28 | 11 | 54 | 0 | L123–L132 |
| R | 98089333 | 93 | 6 | 14 | 73 | 0 | L142–L151 |
| R | 98089334 | 93 | 12 | 31 | 50 | 0 | L161–L170 |
| S | 98089331 | 98 | 46 | 7 | 45 | 0 | L76–L85 |
| S | 98089332 | 98 | 33 | 10 | 55 | 0 | L95–L104 |
| S | 98089333 | 98 | 14 | 8 | 76 | 0 | L114–L123 |
| S | 98089334 | 98 | 25 | 8 | 65 | 0 | L133–L142 |
| T | 98089331 | 103 | 7 | 57 | 35 | 4 | L79–L106 |
| T | 98089332 | 103 | 7 | 87 | 0 | 9 | L115–L145 |
| T | 98089333 | 103 | 0 | 55 | 13 | 35 | L154–L175 |
| T | 98089334 | 103 | 2 | 50 | 33 | 18 | L184–L216 |

For an unobserved gap (`U`), the number of covering reports for the loop and
each of the four vCPUs is 0: their state is **unknown**, not idle, absent or
descheduled. A report reused by several gap rows is the same aggregate sample;
its counts must not be summed once per gap. R's 59/93 vCPU-98089331 BQL stacks
and 45 link plus 14 compile loop stacks, during the four named replacement
submits, support the lock mechanism. S has the same pattern in its aggregate
window, but its first four replacement gaps extend beyond nominal sampling.
T is different: loop 97/103 other, 6/103 transfer, with guest-path counts
57/87/55/50 across the vCPUs. That report does not support labelling B5 a
shader-link/BQL stall.

### Per-gap command, origin and sample join

For 0x207 (`SUBMIT_3D`), size is the trace's command-stream byte count,
excluding the header. Where guest stamps survive, their header is 32 bytes.
For 0x206 (`TRANSFER_FROM_HOST_3D`) the trace has a resource ID but lacks
context and byte count. A matching wire fence can supply the guest header
context, origin and 72-byte header; `payload_bytes=0` is not the amount of
resource data transferred. These types are defined at
`kernel/src/gpu/virgl/wire.rs:26–32`; the core journal records the request
header's context and constructor origin at
`kernel/src/gpu/virgl/mod.rs:412–435`, with initiating-context inheritance at
`kernel/src/gpu/mod.rs:1300–1308`.

The origin column uses the **command's** wire fence, not proximity to a case
name. `unknown` means the matching guest command stamp was evicted. The JSON
also gives the context-create event and surviving guest create stamps where
available. Each sample letter expands to the main-loop and four vCPU counts
above. A question mark context is absent from the host transfer trace; a
guest-stamp context in parentheses is a separate, explicitly joined source.

| Gap / case | Host begin–end seconds minus 1794442000 | ms | Preceding GPU command: type, context/resource, stream bytes | Origin PID/TID; guest command citation | Samples | Trace gap lines / command line |
| --- | --- | ---: | --- | --- | --- | --- |
| A1 window-65 | 800.250909000–800.502439000 | 251.530 | 0x207, ctx 0x153, 19660 | unknown: command stamp evicted | R N | Q:L695226–695227; cmd L695226 |
| A2 window-65 | 800.780572000–800.900498000 | 119.926 | 0x207, ctx 0x153, 37912 | unknown: command stamp evicted | R N | Q:L695326–695327; cmd L695326 |
| A3 window-65 | 800.948977000–801.234729000 | 285.752 | 0x207, ctx 0x152, 19660 | unknown: command stamp evicted | R N | Q:L695372–695373; cmd L695372 |
| A4 window-65 | 801.507423000–801.637732000 | 130.309 | 0x207, ctx 0x152, 37912 | unknown: command stamp evicted | R N | Q:L695467–695468; cmd L695467 |
| B1 window-67 | 835.928052000–836.171954000 | 243.902 | 0x207, ctx 0x15c, 19660 | 753/987; S:L1146981 | S L | Q:L945960–945961; cmd L945960 |
| B2 window-67 | 836.441359000–836.566982000 | 125.623 | 0x207, ctx 0x15c, 37912 | 753/987; S:L1147009 | S L | Q:L946053–946054; cmd L946053 |
| B3 window-67 | 836.824759000–837.095425000 | 270.666 | 0x207, ctx 0x15b, 19660 | 751/984; S:L1147065 | S L | Q:L946162–946163; cmd L946162 |
| B4 window-67 | 837.357828000–837.457933000 | 100.105 | 0x207, ctx 0x15b, 37912 | 751/984; S:L1147093 | S L | Q:L946256–946257; cmd L946256 |
| B5 window-67 | 837.747922000–837.955760000 | 207.838 | 0x206, ctx ?, res 0xf31, bytes absent (guest ctx 0x15c, header 72 bytes) | 751/984; S:L1147655 | T N | Q:L948072–948073; cmd L948041 |
| C1 focus/pair-click | 399.070970000–399.353804000 | 282.834 | 0x207, ctx 0x9c, 19660 | 346/463; S:L524518 | P N | Q:L318563–318564; cmd L318563 |
| C2 focus/pair-click | 399.802086000–400.112856000 | 310.770 | 0x207, ctx 0x9d, 19660 | 348/466; S:L524628 | P N | Q:L318746–318747; cmd L318746 |
| C3 focus/pair-click | 400.329959000–400.434211000 | 104.252 | 0x207, ctx 0x9d, 33048 | 348/466; S:L524652 | P N | Q:L318820–318821; cmd L318820 |
| C4 focus/pair-click | 400.434540000–400.574390000 | 139.850 | 0x207, ctx 0x9d, 37912 | 348/466; S:L524656 | P N | Q:L318838–318839; cmd L318838 |
| C5 focus/pair-click | 400.648152000–400.751392000 | 103.240 | 0x207, ctx 0x9c, 33048 | 346/463; S:L524680 | P N | Q:L318883–318884; cmd L318883 |
| C6 focus/pair-click | 400.751839000–400.889527000 | 137.688 | 0x207, ctx 0x9c, 37912 | 346/463; S:L524692 | P N | Q:L318909–318910; cmd L318909 |
| C7 focus/pair-click | 401.269814000–401.704471000 | 434.657 | 0x206, ctx ?, res 0x736, bytes absent (guest ctx 0x9d, header 72 bytes) | 345/461; S:L525262 | P L | Q:L320960–320961; cmd L320941 |
| C8 focus/pair-click | 402.054702000–402.324881000 | 270.179 | 0x207, ctx 0x9f, 19660 | 354/472; S:L526112 | U | Q:L322619–322620; cmd L322619 |
| C9 focus/pair-click | 402.592359000–402.715273000 | 122.914 | 0x207, ctx 0x9f, 37912 | 354/472; S:L526258 | U | Q:L322794–322795; cmd L322794 |
| C10 focus/pair-click | 403.030270000–403.261566000 | 231.296 | 0x207, ctx 0xa1, 19660 | 355/474; S:L526432 | U | Q:L323022–323023; cmd L323022 |
| C11 focus/pair-click | 403.541573000–403.678834000 | 137.261 | 0x207, ctx 0xa1, 37912 | 355/474; S:L526612 | U | Q:L323252–323253; cmd L323252 |
| D1 focus/keys-to-focused | 413.671369000–413.958272000 | 286.903 | 0x207, ctx 0xa2, 19660 | 357/478; S:L547125 | U | Q:L329658–329659; cmd L329658 |
| D2 focus/keys-to-focused | 414.266377000–414.544460000 | 278.083 | 0x207, ctx 0xa3, 19660 | 359/481; S:L547177 | U | Q:L329775–329776; cmd L329775 |
| D3 focus/keys-to-focused | 414.824044000–414.952711000 | 128.667 | 0x207, ctx 0xa3, 37912 | 359/481; S:L547247 | U | Q:L329880–329881; cmd L329880 |
| D4 focus/keys-to-focused | 415.232509000–415.363040000 | 130.531 | 0x207, ctx 0xa2, 37912 | 357/478; S:L547305 | U | Q:L329991–329992; cmd L329991 |
| D5 focus/keys-to-focused | 415.650074000–416.043652000 | 393.578 | 0x206, ctx ?, res 0x770, bytes absent (guest ctx 0xa3, header 72 bytes) | 356/476; S:L547829 | U | Q:L331975–331976; cmd L331904 |
| D6 focus/keys-to-focused | 416.393747000–416.664996000 | 271.249 | 0x207, ctx 0xa5, 19660 | 365/487; S:L548649 | U | Q:L333589–333590; cmd L333589 |
| D7 focus/keys-to-focused | 416.934818000–417.055730000 | 120.912 | 0x207, ctx 0xa5, 37912 | 365/487; S:L548815 | U | Q:L333801–333802; cmd L333801 |
| D8 focus/keys-to-focused | 417.384034000–417.666527000 | 282.493 | 0x207, ctx 0xa7, 19660 | 366/489; S:L549059 | U | Q:L334105–334106; cmd L334105 |
| D9 focus/keys-to-focused | 417.947483000–418.048308000 | 100.825 | 0x207, ctx 0xa7, 37912 | 366/489; S:L549201 | U | Q:L334291–334292; cmd L334291 |
| E1 performance/frame-floor | 1117.975636000–1118.248273000 | 272.637 | 0x207, ctx 0x1d1, 19660 | unknown: command stamp evicted | U | Q:L1506604–1506605; cmd L1506604 |
| E2 performance/frame-floor | 1118.520277000–1118.651373000 | 131.096 | 0x207, ctx 0x1d1, 37912 | unknown: command stamp evicted | U | Q:L1506691–1506692; cmd L1506691 |
| E3 performance/frame-floor | 1118.967679000–1119.244107000 | 276.428 | 0x207, ctx 0x1d0, 19660 | unknown: command stamp evicted | U | Q:L1506819–1506820; cmd L1506819 |
| E4 performance/frame-floor | 1119.513286000–1119.638241000 | 124.955 | 0x207, ctx 0x1d0, 37912 | unknown: command stamp evicted | U | Q:L1506906–1506907; cmd L1506906 |
| E5 performance/frame-floor | 1119.910361000–1120.163277000 | 252.916 | 0x206, ctx ?, res 0x1322, bytes absent | unknown: command stamp evicted | U | Q:L1507924–1507925; cmd L1507853 |
| E6 performance/frame-floor | 1120.169426000–1120.389624000 | 220.198 | 0x206, ctx ?, res 0x1323, bytes absent | unknown: command stamp evicted | U | Q:L1507986–1507987; cmd L1507983 |
| E7 performance/frame-floor | 1120.745701000–1121.029900000 | 284.199 | 0x207, ctx 0x1d3, 19660 | unknown: command stamp evicted | U | Q:L1509493–1509494; cmd L1509493 |
| E8 performance/frame-floor | 1121.307966000–1121.436371000 | 128.405 | 0x207, ctx 0x1d3, 37912 | unknown: command stamp evicted | U | Q:L1509641–1509642; cmd L1509641 |
| E9 performance/frame-floor | 1126.662079000–1126.782793000 | 120.714 | 0x206, ctx ?, res 0x1322, bytes absent (guest ctx 0x1d1, header 72 bytes) | 994/1293; S:L1440058 | U | Q:L1621243–1621244; cmd L1621224 |

For A1–A4 and B1–B4, the gap begins at ctx-submit and ends at the command's
fence-control event. For example A1 is Q:L695226–695227, followed by fence
response L695230; B1 is Q:L945960–945961, response L945964. The stream-size
pattern is reproducible in the retained trace (19660 then 37912 bytes), but
stream size alone does not identify an opcode or establish a repair.

The transfer-associated gaps are materially different. B5's last GPU command
Q:L948041 has fence response Q:L948043 at 1794442837.739552 s, **8.370 ms
before** B5 begins; its guest row is PID 751/TID 984, context 348, type 518,
header 72, payload 0 (S:L1147655). C7, D5, E5, E6 and E9 likewise have a
GPU fence response before the trace gap starts. The JSON records those response
times. Calling that preceding transfer the still-running cause would contradict
its completion record. The gap's immediately preceding event is MMIO IRQ 0
for B5/C7/D5/E5/E9 and virtio_notify for E6.

### Context identity, including what cannot be mapped

Host context-create names in this capture are the generic `breenix`; they do
not encode a PID. Guest type-512 rows can make that mapping precise:

| Context(s) | Host context-create lines | Guest creator PID/TID and serial citation |
| --- | --- | --- |
| 0x9c / 0x9d | Q:L318360 / L318512 | 346/463 S:L524376; 348/466 S:L524480 |
| 0x9f / 0xa1 | Q:L321808 / L322325 | 354/472 S:L525366; 355/474 S:L525852 |
| 0xa2 / 0xa3 | Q:L329416 / L329542 | 357/478 S:L546979; 359/481 S:L547055 |
| 0xa5 / 0xa7 | Q:L332823 / L333317 | 365/487 S:L547933; 366/489 S:L548405 |
| 0x152 / 0x153 | Q:L694847 / L694959 | creator cannot be mapped from retained create stamps |
| 0x15b / 0x15c | Q:L945399 / L945616 | 751/984 S:L1119034; 753/987 S:L1119126 |
| 0x1d0 / 0x1d1 / 0x1d3 | Q:L1505374 / L1505456 / L1508834 | creator cannot be mapped from retained create stamps |

The window-67 startup export identifies PID 751 as the replacement compositor
(S:L1094173 onward); 753/987 is explicitly `clone-child-987`
(S:L1114461). Thus B1/B2 have a named worker command and B3/B4 a named
replacement-leader command, with their exact submit/return times in the JSON.
For window 65, later surviving usage associates ctx 0x152 with 722/949
(S:L1084380), and ctx 0x153 with 722/949 and 724/952
(S:L1084202,L1084208). That is useful evidence of replacement-context use,
but it is not the evicted A1–A4 origin or a recovered create event. The
report leaves those four command origins unmapped.

Likewise later use associates 0x1d0/0x1d1 with the frame-floor compositor
PID 994 (startup S:L1434987 onward), and 0x1d3 with 1002/1302
(S:L1436098); it does not recover the missing creators or E1–E8 command
origins. A process using a context is not necessarily its creator: the same
context can be used by helpers or another initiating client. Resource 0xf31
is attached to both 0x15c and 0x15b at Q:L945870,L945872; 0x736 to both
0x9c/0x9d at L318644,L318646; 0x770 to 0xa2/0xa3 at L329802,L329804;
0x1322 and 0x1323 to 0x1d0/0x1d1 at L1505928–L1505930 and
L1506234–L1506236, later also 0x1d4. Attachment alone cannot choose the
context of a transfer; the matched guest fence can, when retained.

### The failures' own intervals

The click failures are failures of positive click delivery, including
keys-to-focused before its key checks (`cases.rs:596–625`). Their 1000 ms
waits must not be confused with initial desktop startup. The wait records
give 1003.214083 ms for pair-click (S:L506624) and 1004.076375 ms for
keys-to-focused (S:L528683). Delivery occurs later at 345523942041 ns
(S:L506626), respectively 359895621166 ns (S:L528685). No successful
acknowledgment inside those deadlines is inferred.

| Failed subject | Intersecting >100 ms gaps | Intersection qualification | Host stack evidence |
| --- | --- | --- | --- |
| Pair-click wait, 344425664333–345428878416 ns | C9, C10, C11 | C11 crosses the wait's right boundary | 0 covering reports for loop or each vCPU; states unknown |
| Keys-to-focused click wait, 358710808916–359714885291 ns | D7, D8 | both contained | 0 covering reports for loop or each vCPU; states unknown |
| Frame-floor, 1063256757375–1068256814750 ns | E8 | crosses the left boundary; most of the 128.405 ms is before measurement | 0 covering reports for loop or each vCPU; states unknown |

The host trace and retained command rows identify the two click waits'
submissions and originating clients in the gap table. They cannot supply
missing thread stacks. P's high BQL/link counts describe the **earlier**
pair-click desktop preparation, not either failed click wait. No descheduling
conclusion follows from the missing samples.

Frame-floor first/last receipts are 1063285804041 / 1068253781875 ns
(S:L1434681): leading gap **29.046666 ms**, trailing gap **3.032875 ms**.
`rate.rs:21–23` requires each endpoint within 1/60 second, so the leading
endpoint fails by **12.380 ms**. Its largest internal presentation gap is
**7.106916 ms** (S:L1434959). E8 begins before measurement and ends about
17.9 ms after its start under the nearest-anchor mapping; this is a candidate
timing overlap, not host-stack attribution. E1–E7 occur during setup; E9 is
after the measured five seconds. Treating the case's nine gaps as nine stalls
inside its measurement would be incorrect.

### Result and validation

R supplies the replacement-window evidence missing in the earlier measurement:
named host submits on 0x153/0x152, four gaps of 119.926–285.752 ms, and
explicit vCPU BQL waits alongside main-loop link/compile/transfer stacks.
This meets line 239's lock-versus-descheduling comparison at aggregate sample
resolution for that replacement work. It does not identify which sampled
stack occurred in each particular gap or show lock ownership at an instant.
B5 and the unsampled failed waits remain outside that causal conclusion.
Exact A1–A4 originating commands and several frame-floor contexts remain
unmapped because the guest stamps were evicted; generic host context names
do not restore them. A repair targeted at a particular originating client
would need that missing identity. This analysis unit does not implement a
forcing runtime check, preservation change or repair.

Check-first commit `44415973` adds `tests/a3_sample_join_test.py`. Its first
run fails with the missing analyzer; an intermediate run exposed a zero-count
category in the parser; after that correction the scoped check passes 1/1.
The check exercises disjoint stack counting, strict >100 ms selection,
preceding-command retention across another event and refusal to borrow an
uncovering sample. This is a check of the new offline analyzer, not a runtime
forcing check for a speculative GPU repair. The subsequent source edits add
capture extraction and fence/clock joins; their output is the linked JSON.
Kernel/userspace artifacts are unchanged, so no release build or suite rerun
is reached by this unit.

Reproduce the offline extraction with:

```sh
python3 scripts/analyze-a3-sample-join.py CAPTURE_DIRECTORY OUTPUT.json
```

Validation: the scoped parser check passes; claim-lint results and final
commit/push identity are recorded in the ARM ledger. The pre-check commit's
message lint was run retrospectively (exit 0); its file lint ran before the
commit (exit 0). No boot, PR creation, merge, rebase, force push, deployment
or board update was performed in this unit.

CONFIRMED BUT CONTEXT UNMAPPED

## Replacement measurement at 9f665106: observation unavailable

The normal-build measurement `20261112T085617Z-qemu-94194` passed the
artifact gate at host load 7.8. Its guest reported 81/81, memory retained
10 of the allowed 48 pages, and the standalone READY followed DONE and MEMORY.
The host renderer check accepted ANGLE/Metal. The launcher nevertheless exited
1: `suite-verdict.py` refused twelve planned `*-remaining` categories with no
cases. This is a separate acceptance blocker; the scorer was not changed.

<!-- claim-lint:ok: #2872; 20261112T085617Z-qemu-94194; input_latency at 9f665106. -->
The animation observation was explicitly skipped with
`diagnostic-route-absent`. The reported animation maximum, 40.181 ms, covers
only the 100 inputs after the ten warmups: `input_latency` arms observation for
all 110, but adds to the measured sample vector only when `n >= 10`. Thus the
capture cannot establish whether a warmup exceeded 75 ms, nor identify its
acquire subphase, dependency, client wait, or host worker state. Three host
animation samples, the two-second load series and CPU top-10 series exist, but
without the frozen guest observation they do not establish a mechanism. The
measurement is void under the coordinator's observation requirement. No
latency fix or acceptance is claimed.

| Measurement | Observed | Bound | Headroom |
| --- | ---: | ---: | ---: |
| Animation median | 13.533 ms | 33 ms | 59.0% |
| Animation measured max, excluding warmups | 40.181 ms | 100 ms | 59.8% |
| Typing median / max | 7.327 / 28.665 ms | 20 / 50 ms | 63.4% / 42.7% |
| Pointer median / max | 8.179 / 25.149 ms | 20 / 50 ms | 59.1% / 49.7% |
| Present median / max | 1.453 / 11.826 ms | 16 / 33 ms | 90.9% / 64.2% |
| Frame floor | 448 Hz | 60 Hz minimum | 7.47 times minimum |
| Supervisor restart wait | 1113.678 ms | 2000 ms | 44.3% |
| Replacement first composite | 386.787 ms | 2000 ms | 80.7% |
| Slowest reap | 5.471 ms | 500 ms | 98.9% |
| Slowest recorded positive click | 138.988 ms | 1000 ms | 86.1% |
| Slowest recorded click release | 6.025 ms | 1000 ms | 99.4% |

<!-- claim-lint:ok: #2872; retained 20261112T085617Z-qemu-94194, coordinator line 307 supersedes this audit verdict. -->
These are measurements from the void run, not proof. The artifact hashes and
prediction are in the ARM ledger; the full serial, host thread samples, load
and top-10 captures are retained with the Vigil boot and in the local capture.
A coordinator ruling is needed on the missing observation and scorer refusal
before further boots.
