# E24 24.15p: failures before the metadata latch

Coordinator E24 ledger line 509(p) requests attribution or an explicit finding
where code and retained captures cannot establish the cause. This report uses
acceptance `20261118T003826Z-qemu-34808` at `c9426482` and source at `d184be0b`.
No runtime repair is claimed in this unit. Acceptance remains open in #3030.

## Scheduling: unresolved causal attribution

The breenfs-root capture reports `scheduling/sched-yield` FAIL: 176 yielding
passes against 63 spinning passes. The unchanged condition requires more than
four times the spinning count. Main's ARM capture
`20261115T045525Z-qemu-73586` at `ac4f2781` reports PASS in 804 ms, but does not
record either successful pass count.

`userspace/programs/src/suite_processes.rs::ring_passes` forks sixteen processes
on this four-CPU configuration, waits 50 ms, then measures each mode for 300 ms.
The measured loop only touches the shared token and optionally calls
`sched_yield`; it does no filesystem I/O. The processes are not CPU-pinned.

The additional competitor can be the commit worker. Its `init_service` loop in
`kernel/src/fs/breenfs/vfs.rs` calls `commit_due`, then parks on
`CommitSignal::wait`. Ruling 488's `preemption_point` only schedules when
`need_resched` and `can_yield` are both true. It releases and restores the
single driver entry count, and excludes idle/boot continuation and interrupt
contexts using the scheduler's identity (24.15m). It does not call
`note_sched_yield` or rewrite the user ring's comparison. The existing
`cpu_bound_commit_yields_at_its_next_point` check also checks that a cleared
reschedule request does not cause another yield. That check is not rerun here.

**Disposition:** no deterministic scheduling defect established. The captures
have no worker run intervals, queue ordering, CPU placement of token holders,
or dispatch trace across the two measurement windows. Source can establish
where the worker may compete, but cannot recover whether it competed in the
spinning window, the yielding window, or neither. A sequential host model with
an invented dispatch schedule would assume the proposed cause. The worker's
effect on the observed ratio therefore remains unresolved; neither a scheduler
patch nor a change to 488's points follows from this evidence.

## libc: setup cost isolated; guest phase unresolved

Main's ARM capture `20261021T080152Z-qemu-76656` at `831aeba0` reports
`dirent-glob/readdir-many` PASS in 298 ms. Acceptance-m reports its 10000 ms
timeout and SIGKILL of the C helper, without a setup/enumeration phase marker.

The C case in `userspace/libc-suite/dirent_glob.c::readdir_many` first creates
300 names. Each `h_put_file` in `harness.c` opens with O_CREAT|O_TRUNC, writes
one byte, then closes. Only after that does it enumerate the directory.
24.15k's enumeration-only count check does not cover that setup.

<!-- claim-lint:ok: tools/vfs-host-tests/tests/large_directory_cost.rs::prelatch_libc_create_write_profile calls resize with length 0 and never calls sync. -->
The new `large_directory_cost::prelatch_libc_create_write_profile` drives
create_open, zero resize, one-byte write, release and an 8192-byte getdents
sink through the production VFS. It does not insert periodic fsync calls.
The optional `PRELATCH_ROOT_IMAGE` selects a retained disposable capture read
into host memory; the default uses the existing small image. It does not run
the C library, syscall/path lookup, concurrent worker or guest scheduler.

The final check passed once against acceptance-m's retained root in 11.16 s.
Its counters, reset after each row, were:

| Created files | Interval size | Node decodes | Decoded items | Pressure commits | Device calls |
|---|---:|---:|---:|---:|---:|
| 1-75 | 75 | 14404 | 1361026 | 1 | 69 |
| 76-150 | 75 | 19430 | 2102299 | 1 | 76 |
| 151-300 | 150 | 38816 | 4620561 | 3 | 239 |

<!-- claim-lint:ok: E24 24.15p libc-capture-profile.log and libc-profile.log in the local evidence directory record the READDIR_COST counters, retained with the report on #3030. -->
Enumeration then used three calls, twelve decodes against the unchanged
node/height/call bound of fifteen, and zero device calls. Setup therefore
accounts for 72650 decodes and five pressure publications in this replay.
The earlier small-image profile passed in 3.98 s with 32333 setup decodes and
two pressure publications; its enumeration used thirteen decodes, bound fifteen.
Host wall time is reported, not compared with a guest performance threshold.

`txn.rs::View::read_node` decodes private bytes on each read;
`btree.rs::Edit::load` also decodes the encoded node for mutations.
`MountState::retain_open` scans retained VNodes when opening or reading inode
metadata. These are CPU-cost candidates for setup. Their presence does not
identify the guest's last completed operation or time spent parked.

**Disposition:** the setup/enumeration guard already passes, retained under U29.
It proves successful admission and exact name enumeration on current code,
including pressure publication after 24.15n. It does not reproduce the timeout.
The capture cannot tell whether the killed helper reached opendir, nor
separate mutation CPU work, worker/mount-lock waits and syscall overhead.
A mutation/cache optimization selected from these counts alone would not be a
proven causal repair. The timeout stays unresolved; 24.15k's linear enumeration
check and the guest's deadline and comparison remain unchanged.

## UI: phase delays established; producer causes unresolved

Main's ARM capture `20261115T165620Z-qemu-89567` at `99bd25dd` passes the
following cases that fail before acceptance-m's observed-metadata latch:

- input/type-shifted;
- tree/names-values, focus-flag, focus-ring-pixels, enabled-flag, state-flags,
  textfield-value, terminal-grid, commit-coupled, wm-tree, hidden-nodes,
  list-scroll, permission and text-pixels;
- frames/window-capture, capture-while-drawing and capture-resize-race.

The seventeen failures are retained individually in the local comparison
artifact. `frames/screen-capture` is the latch boundary, not included in this
pre-latch set. Later EROFS failures are not attributed to these seventeen.

The retained UI_CONTROL records localize delays:

| Case / wait | Main baseline | Acceptance-m |
|---|---:|---:|
| input/type-shifted Configure | 20 ms | 567 ms |
| input/type-shifted InputDelivery total | 2309 ms | 4904 ms |
| tree/names-values Configure | 16 ms | 5427 ms |
| tree/names-values CompositorRegistration | 85 ms | 2584 ms |
| tree/focus-ring-pixels DiscardReply | 2 ms | 7420 ms |
| tree/focus-ring-pixels SupervisorTeardown | 192 ms | 195 ms |

Configuration in `libs/libuitest/src/supervisor_runtime.rs` prepares and verifies
the session fixtures, creates the mapped trace and configures the evidence
arena before replying. `session.rs::wait_window_until` requires both a matching
window registry entry and its composite observation before the absolute
existing deadline. Seeing a later buffered bwm discovery message does not
establish that observation was published in time.

The collector path in `supervisor_runtime.rs` runs descendant teardown if
needed and removes the configuration tree before replying. The separate
SupervisorTeardown timer does not include that tree removal. Thus a short
teardown timer and a long DiscardReply timer are compatible. Neither says
which filesystem operation or scheduling interval delayed the reply.

Acceptance-m's whole-suite page_cache count rises from 2 to 4041 pages, but
there is no per-case ownership census before the latch. That cannot establish
when the pages accumulated or which pre-latch failure they affected. 24.15o
repairs ENOMEM/latch classification and failed-mount retirement; it supplies
no retrospective cause for the earlier delays.

**Disposition:** configuration, input, first-composite/window waits and cleanup
are separate unresolved producer paths. The capture has no per-operation
filesystem/parking spans, pre-latch mapping ownership samples, composite
publication times for the failed waits, or blocked kernel continuations for
the SIGKILL survivors. Host UI logic and MapState stand-ins cannot reconstruct
those missing observations. In addition, `startup_phase_record` deliberately
omits phase export on discard except the two start-session cases, so the
shared mapped startup stamps are not a retained phase trace for these failures.
No UI, cleanup, scheduler or mapping patch is
claimed from aggregate elapsed waits. Limits, cases and comparisons stay as
captured. No boot is run or requested by this report.

## Scope and validation

This unit adds one host guard and this attribution report. It changes no
kernel, guest case, deadline, reserve, latch or comparison. No source from
another operating system or C library was consulted. The x86-metal acceptance
and ARM64 runtime acceptance remain pending; ARM still lacks a passing
breenfs-root processes/libc/UI capture after 24.15n/o and parity evidence for
the later persistent-root milestones. No PR or merge is performed here.

Validation for this unit:

- Check commit `efb5bea0`: captured-root host profile 1/1 PASS, 11.16 s;
  host compilation emitted no warnings. The U29 result is recorded as a guard.
- x86-64 production kernel build: exit 0, 116.11 s, no warnings.
- ARM64 production kernel build: exit 0, 0.37 s, cached artifacts; no new
  warnings, with the existing pinned-core future-compatibility notice.
- `scripts/claim-lint.py`: exit 0; explicit file and commit-message invocations
  also exit 0. Logs are retained in the local 24.15p evidence directory.

No tests are repeated after the check commit, report commit or push. There is
no runtime fix to accept in this unit, and no guest result is inferred from the
host profile. The three causal findings remain open for the coordinator.
