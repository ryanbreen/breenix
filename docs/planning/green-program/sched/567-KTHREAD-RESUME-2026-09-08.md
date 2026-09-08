# 567: pre-userspace x86 resume witnesses

Branch: `sched/567-kthread-resume-context`. Investigation base:
`4394409fca3932296f3468914b5be325ce0d48a6`.

The repaired API mechanism is selection without CPU dispatch: the former public
x86 scheduling entry could set current_thread=B while the CPU still executed
thread A with RIP=A.rip and RSP=A.rsp. The next interrupt could then write A's
RIP/RSP/RFLAGS into B's owned CpuContext. Round 2 restricts selection to the
interrupt-return dispatcher and makes the public entry request rescheduling.
It also restores four scheduling loopback tests and adds a register witness.
The Tier-2 change is in context_switch.rs at the selection boundary and removes
inherited output. Tier-1 source was not modified. Historical attribution of
567 remains unresolved; matching debugger samples do not identify an original
corrupting instruction.

## Mechanism examined

The boot thread is a boxed `swapper/0` thread, registered as both current and
idle (`kernel/src/main.rs:559`, `kernel/src/main.rs:578`). Its active stack is
the separately allocated kernel stack selected before
`kernel_main_on_kernel_stack`, not the bootloader stack
(`kernel/src/main.rs:478`, `kernel/src/main.rs:520`).

On x86, `scheduler::yield_current` only sets `NEED_RESCHED`
(`kernel/src/task/scheduler.rs:6365`). The x86 switch runs in
`check_need_resched_and_switch` on interrupt return
(`kernel/src/interrupts/context_switch.rs:156`); `schedule_from_kernel` is
an aarch64 entry, not this x86 path. The kernel-thread save copies the
registers and five frame fields into the thread's owned `CpuContext`
(`kernel/src/interrupts/context_switch.rs:706`). It does not retain a pointer
to the interrupted stack frame. The idle dispatch can restore that context
before userspace starts (`kernel/src/interrupts/context_switch.rs:845`).
`setup_kernel_thread_return` copies it into the outgoing interrupt frame
and `SavedRegisters` (`kernel/src/interrupts/context_switch.rs:1265`).

The stack register order in `kernel/src/task/process_context.rs:95` matches
the pushes in `kernel/src/interrupts/timer_entry.asm:40`: fifteen 8-byte
registers, followed by RIP, CS, RFLAGS, RSP and SS. The five-word frame is
consumed by IRETQ (`kernel/src/interrupts/timer_entry.asm:378`).
`CpuContext` uses a different, explicit field order
(`kernel/src/task/thread.rs:89`); the save/restore performs field copies,
not a cast between these layouts. `requeue_refused_dispatch`
(`kernel/src/task/scheduler.rs:6651`) changes queue membership, with guards
for idle, current, absent, non-Ready and already-queued threads. It does not
write a register snapshot.

Commit `6d17b83a3` changed the dispatch context read and removed a thread-name
allocation. Its 791 RCA, at
`docs/planning/green-program/sockets/787-REGRESSION-RCA-2026-09-04.md`, establishes
an IF=0 spin on a mutex held by a preempted thread. That is a deadlock finding;
it does not establish the cause of 567's mismatched RIP/RSP/RFLAGS. Blame
shows the RIP/RSP field assignments predate that commit. No historical
reverted-diff reproduction was performed in this continuation, so no specific
historical commit is credited with repairing 567.

## Rechecked inherited debugger evidence

Artifacts are under `serials/567/`. The `inherited-` prefix denotes captures
made before this continuation, subsequently parsed and checked here. Absolute
lane paths were normalized to `<repo>` and `<lane-tmp>`; register values and
addresses were retained. These are partial debugger boots, not full gate
passes. The common source base is the revision above plus the inherited
oracle, registry and placement edits; the captures are not represented as
pristine base-revision boots.

`serials/567/inherited-loopback-snapshots.jsonl` contains 1,603 switch-away,
1,598 restore-input, 1,595 IRET-frame and 1,595 resumed records. The recorded
difference dictionaries are empty. The counts are unequal, so this is not
1,603 complete comparisons. The 1,595 resumed records are loopback-window
samples, not the oracle's distinct register-witness samples.

For sequence 1 in that file, RIP is `0x100001dde61`, RSP is
`0xffffc90000100e88`, RFLAGS is `0x292` (IF=1, DF=0), CS is `0x8` and SS is
`0x10`. The snapshot resides at `0x4444444dff00`; the incoming frame slot is
`0xffffc90000100e58` and the outgoing frame slot is `0xffffc90000284e98`.
The snapshot slot is outside those stacks. The timer entry passes its current
RSP as the register-block pointer (`kernel/src/interrupts/timer_entry.asm:119`);
it does not switch to a per-CPU scheduler stack. The later outgoing frame
is on the thread stack active at the return-to-boot dispatch, not the owned
snapshot storage. Save, restore-input, IRET-frame
and actual-resume values agree in this sample. The capture normalizes
RFLAGS bit 1 to its architectural fixed value before comparing the IRET
frame; RFlags conversion can omit that reserved bit. The capture script reads
GS+24 for the idle-thread pointer, then adds the binary's `0x130` context
offset. See `serials/567/inherited-capture.py`; its offsets belong to that
binary and must not be reused for another binary without checking disassembly.

The oracle-first ordering reached its 32-cycle PASS and five loopback PASS
markers in `serials/567/inherited-oracle-first-serial.log`. The debugger
command stream is `serials/567/inherited-oracle-first-debug.jsonl`.
The other retained ordering runs loopback before the oracle and includes
an intentional saved-flags mutation, described below; it is not counted
as a clean oracle pass.

## Fresh debugger capture at ed03fd9b

The committed code and ratchets are `ed03fd9b9b406e8cce3e59723f35708ff0fa539a`.
`serials/567/final-source.tar.gz` was compared byte-for-byte with the eight
files in that commit. Source citations were re-derived after the commit in
`serials/567/source-citations.txt`.

`serials/567/fresh-debug/fresh-snapshots.jsonl` records 32 switch-away,
32 restore-input and 32 actual-resume samples, with 32/32 comparison sets
matching before mutation. In sequence 1, RIP is `0x10000187dbc`, RSP is
`0xffffc900001014e0`, RFLAGS is `0x247` (IF=1, DF=0), CS is `0x8`, and SS is
`0x10`. The source interrupt-frame slot is `0xffffc900001014b8`, register
block is `0xffffc90000101440`, and the boot Thread address is
`0x4444444dfdd0`. Its owned snapshot is at Thread+`0x130` =
`0x4444444dff00`. The restore-input frame is `0xffffc90000488eb8`.
The register constants, R10=RSP and R11=RIP agree at save, restore-input
and CPU resume. These samples do not show a layout mismatch or a snapshot
living in a reused stack frame.

After recording sequence 32 at the resume label, the debugger changes RDX
from `0x567e` to `0x566e`. The fresh serial reports
`[BOOT_RESUME_ORACLE:x86:cycles=32:mismatch=1:FAIL]`. This changes a witness
register in the running guest before its assembly comparison. It does not
change source or an ELF. The guest was stopped with owned-PID cleanup before
the subsequent gates; the mutation did not survive that guest.

The command stream is
`serials/567/fresh-debug/fresh-gdb-session/output.jsonl`, the capture code is
`serials/567/fresh-debug/fresh-capture.py`, and the full serial and driver
log are alongside them. The bootloader printed base `0x10000000000` and
`resync-symbols` verified it. GDB also exposed relative minimal-symbol
addresses; the capture adds the verified base before reading instructions
or setting its comparison breakpoints. The earlier unrelocated breakpoint
and disassembly attempts are not used as machine-state evidence.

Two earlier debugger starts are retained under this directory's `evidence/`
subdirectory. One launcher waited on a stdout pipe inherited by a background
session; the second rejected a quoted symbol name before guest execution.
The lane-local adapter redirects startup output to a file and cleans up
owned PIDs. Its changes relative to the repository tools are recorded in
`serials/567/fresh-debug/debug-tools/adapter.patch`; the adapter/driver
sources reflect the final corrections, while the command transcript records
the manual relocation correction actually used in this run. Neither earlier
start is counted as a boot or comparison sample.

## Oracle and mutation

The boot_tests+x86_64 oracle runs immediately after the direct loopback
wrapper, after the retirement cohort and before the exec cohort. With IF
masked, the boot thread publishes a cycle request, unparks a peer and asks
for rescheduling. A naked assembly function sets thirteen distinct general
register constants, R10=RSP, R11=resume-label address and RFLAGS=`0x47`.
STI;HLT allows dispatch. At the resume label, PUSHFQ captures flags before
CLI/CLD and comparisons. Expected RFLAGS is `0x247` (IF=1, DF=0). Rust runs
again with IF=0 and DF=0. A cycle completes only after the peer's matching
ACK; additional interrupt resumes are checked as well. `mismatch` counts
failing witness returns, rather than individual differing fields. The peer uses
`kthread_park_if` to recheck REQUEST after publishing sleep intent, avoiding
a lost unpark in the test driver.

Grammar: `[BOOT_RESUME_ORACLE:x86:cycles=<decimal>:mismatch=<decimal>:PASS|FAIL]`.
`scripts/score-boot-resume.py` accepts one result with cycles=32, mismatch=0,
PASS, and requires one START then PASS for each of the five named loopback
tests. Missing, duplicate, short-cycle, mismatch and FAIL fixtures are
rejected through the scorer CLI by `tests/boot_resume_structure.rs`. The existing userspace wake
gate remains separately scored; boot-window counters are not substituted
for userspace execution evidence.

`serials/567/inherited-flags-mutation-debug.jsonl` stops in the actual save
path for the witness resume RIP `0x100002836d8`. After the context copy,
it reads `CpuContext.rflags` at snapshot+`0x88`, changes `0x247` to `0x647`,
and stops at the resume label. Actual RFLAGS is `0x647` (IF=1, DF=1), with
RSP `0xffffc900001014e0`, CS `0x8`. The oracle's PUSHFQ/CLI/CLD sequence
captures the difference and clears DF before Rust executes.
`serials/567/inherited-flags-mutation-serial.log` then contains
`[BOOT_RESUME_ORACLE:x86:cycles=32:mismatch=1:FAIL]`. This is a saved-state
mutation in guest memory, not a source edit or an exception dump. Terminating
that guest discards it. It establishes detection of this mismatch class,
not historical causation.

The archived `serials/567/structure-base-red.log` reports exit 101;
`serials/567/loopback-structure.log` reports 130 passing tests and
`serials/567/resume-structure.log` reports two passing tests. These three
standalone captures did not record a source revision or source hashes. Their
exact input trees cannot be authenticated retrospectively, so they are excluded
from revision-specific baseline and landing proof. A later HEAD-stamped run
does not establish the archived base-red input identity.

## Validation record

The initial full x86 boot-tests invocation passed at the investigation base
plus the source archive `serials/567/first-gate/source.tar.gz`. Its driver
log records the base revision, tracked diff SHA256 and new-file SHA256s.
Launch load was 5.50. Preflight passed 70/70 suites; context_restore_structure
first timed out at 300 seconds at load 9.87, then passed its built-in retry.
Kernel builds emitted no project warning/error diagnostics. Image packing
reported the missing optional BusyBox toolchain and skipped coreutils; those
messages are retained in the log and are not Rust source diagnostics. Both serials
have no CPU exception markers; the oracle reports cycles=32, mismatch=0,
PASS, and five loopback tests pass. This run preceded the peer's conditional
park correction and the scorer-CLI fixture refinement, so it is recorded as
an initial result, not final-tree validation.

The corrected-peer run launched at load 4.37 with the 900-second suite
limit. Its 70/70 preflight and the 32-cycle resume oracle passed, as did
five loopback tests. A later timer-latency oracle reported overrun_ms=479
against bound_ms=100 and FAIL. The retained post-failure load reading at 13:24:20 UTC was 0.45,
which does not establish load at the measurement. This failure is not
attributed to host contention or to the resume change. Its serials are in
`serials/567/corrected-timer-failure/`; its source is in
`serials/567/corrected-source.tar.gz`. The failed invocation is retained.

The additional run of the same kernel source passed the full gate at
launch load 0.32. Its structure fixture pins expected test names independently
of the production scorer list. The recorded timer observation is
13:42:25 UTC, within the observer's two-second polling interval: load 1.11,
overrun_ms=44, bound_ms=100, PASS. Its 70/70 preflight, Rust builds, resume
oracle, five loopback tests and final gate verdict are in
`serials/567/verified-gate/`. Both serials contain no CPU exception markers.
This passing retry does not explain or erase the failed invocation. Follow-up
is tracked in [981](https://github.com/ryanbreen/breenix/issues/981).

The normal testing-profile build at ed03fd9b passed without project Rust
diagnostics, then the first `bash docker/qemu/run-boot-parallel.sh 5` batch
passed 5/5 at launch load 1.32. Its ten serial files have no CPU exception
markers. See `serials/567/parallel-1/`.

The second batch completed 5/5 at ed03fd9b. Its invocation recorded load
1.42 at 14:05:41 UTC; the log then records a lock wait before launching the
guests, so that reading is not a measurement at guest start.
`serials/567/parallel-2/parallel-2.log` ends with
`GATE_EXIT=0`. Its ten serial files under
`serials/567/parallel-2/evidence/parallel-2/run-{1..5}/` contain no matches
for the exception/crash patterns listed below. The two normal testing-profile
batches do not emit the boot_tests-only resume oracle; their 10/10 aggregate
passes are not ten additional register-witness runs.

The final x86 production-profile invocation at ed03fd9b completed after
batch 2. Its invocation recorded load 3.60 at 14:22:58 UTC; its boot-facts
record gives load_at_start=2.87. Preflight passed 70/70 suites with
timeout_s=900, each on attempt 1. The production Rust build completed
without project warning/error diagnostics.
`serials/567/prod-last/prod-last.log` records the steady-state PASS,
crash markers=0, console prompts 1 -> 2, timer-scale PASS with ms_per_tick=5,
and the teardown/root-custody/bracket censuses at rest, then ends with
`GATE_EXIT=0`. Its two serial files also contain no matches for the
exception/crash patterns below. The production serials have no
BOOT_RESUME_ORACLE result: the call at `kernel/src/main.rs:717` is inside
the boot_tests guard at `kernel/src/main.rs:676`. The boot-tests evidence
remains `serials/567/verified-gate/`, rather than this production run.
The retained `serials/567/prod-last/qemu.log` contains two firmware-image
format warnings; these are QEMU diagnostics, not Rust source warnings.

This finish-only continuation re-read the completed logs and serials; it did
not launch another kernel build or guest. The serial scan matched
`EXCEPTION|PAGE FAULT|GENERAL PROTECTION|DOUBLE FAULT|TRIPLE FAULT|KERNEL PANIC|panic!|soft lockup detected`:
0 matches across batch 2's ten files and production's two files.
New copies normalize lane paths and replace the uptime/user-count prefix
with the UTC time and recorded load averages. Serial CRLF is normalized to
LF for Git storage. Marker text and register values are retained. These observations do not explain the earlier timer failure.

### Final gates and serials index

| Record | Result | Evidence |
| --- | --- | --- |
| Normal testing build, ed03fd9b | BUILD_EXIT=0; no project Rust diagnostics | `serials/567/parallel-1/parallel-build.log` |
| Parallel batch 1, ed03fd9b | 5/5; GATE_EXIT=0 | `serials/567/parallel-1/parallel-1.log`; `parallel-1/evidence/parallel-1/run-{1..5}/serial_*.txt` under `serials/567/` |
| Parallel batch 2, ed03fd9b | 5/5; GATE_EXIT=0 | `serials/567/parallel-2/parallel-2.log`; `parallel-2/evidence/parallel-2/run-{1..5}/serial_*.txt` under `serials/567/` |
| Production last, ed03fd9b | 70/70 preflight; GATE_EXIT=0 | `serials/567/prod-last/prod-last.log`; `serials/567/prod-last/breenix_x86_prod_profile_build.log`; `serials/567/prod-last/serial_kernel.txt`; `serials/567/prod-last/serial_user.txt`; `serials/567/prod-last/gate_boot_facts.txt`; `serials/567/prod-last/qemu.log` |

The production build sublog and boot artifacts share the revision recorded
at the head of `serials/567/prod-last/prod-last.log`. Parallel serials share
the revision at the head of their respective batch logs.

claim-lint: python3 scripts/claim-lint.py -> exit 0

claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/567-code-message.txt -> exit 0

The finish-only recheck ran the scorer on
`serials/567/verified-gate/serial_user.txt` (exit 0) and on
`serials/567/fresh-debug/fresh-gdb-serial.log` (exit 1, expected rejection
of mismatch=1). These are rescoring existing captures, not new guest runs.

claim-lint: python3 scripts/claim-lint.py -> exit 0

claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/567-docs-message.txt -> exit 0

The docs lint checked 45 supported files in changed-hunk mode, skipped 35
unsupported artifact extensions, and excluded 215 pre-existing findings
outside the changed hunks. It does not validate the truth of runtime claims.

## Not claimed

- A reproduction or root-cause attribution of the historical 567 failure.
- That 791's deadlock fix necessarily repaired a corrupted register snapshot.
- Protection against arbitrary RIP/RSP corruption that prevents the checker
  from executing; the external scorer rejects the missing result instead.
- SMP or aarch64 register-resume coverage from this x86-only oracle.
- A full boot pass from either inherited debugger capture.
- The cause of the later timer-latency overrun, or BusyBox/coreutils coverage.

## Round 2: V-2 and V-3

V-2 now has a production API change. The former public x86 `schedule()`
selected another current thread and returned to its caller without installing
an interrupt-return frame. A caller executing with RSP=A could therefore leave
`current_thread=B`, after which the next interrupt could save A's RIP/RSP/RFLAGS
into B's context. The comments beside `yield_current` already describe this
sequence; changing test placement did not constrain that API.

The x86 public entry now only requests rescheduling
(`kernel/src/task/scheduler.rs:5823`). The selecting method is private
(`kernel/src/task/scheduler.rs:2345`), and its wrapper requires an
`InterruptDispatch` whose field is private to the dispatcher
(`kernel/src/task/scheduler.rs:5829`,
`kernel/src/interrupts/context_switch.rs:31`). The dispatcher constructs that
permit after its admission and process-manager checks
(`kernel/src/interrupts/context_switch.rs:285`). The register witness retains
its yield and also calls the public scheduling entry
(`kernel/src/task/boot_resume_oracle.rs:122`). This constrains safe callers
outside the dispatcher/scheduler modules. Writes to `CpuContext` through other
APIs and the historical 567 instruction-pointer failure remain outside this
API regression test's scope.

V-3 removes the dispatch file's 14 inherited logging macros and its two
port-writing helpers. The raw marker calls are removed with them. The
KernelFrame refusal distinction is retained as a relaxed counter
(`kernel/src/interrupts/context_switch.rs:1244`); existing refusal and abandon
control flow remains scored by `tests/context_restore_structure.rs`.
The logging census falls from 119 to 105 sites (wider census: 106), and the
raw-writer census removes the corresponding dispatch anchors. These are
reductions to the permitted inventory, not exemptions for new output.

Boot-stage marker consumers in `xtask/src/boot_stages.rs:422` still need the
schedule-return fact. Dispatch now publishes atomic facts; the existing
thread-context heartbeat invokes their emitter
(`kernel/src/task/dispatch_strand_census.rs:302`). The emitter requires IF=1,
and the userspace smoke marker additionally requires the syscall-side latch
(`kernel/src/task/dispatch_boot_facts.rs:23`). No marker is emitted from the
interrupt-return file. The Tier-2 edit is confined to `context_switch.rs`,
where selection admission and the inherited output live. Tier-1 files were
not edited.

`tests/x86_dispatch_admission_structure.rs:40` pins V-2, including four
in-memory mutations. Its compiler probe executes the real public entry and
rejects an external permit construction with E0451. The V-3 ratchet at
`tests/x86_dispatch_admission_structure.rs:94` rejects five output mutations.
The emitter test at `tests/x86_dispatch_admission_structure.rs:201` executes
the production emitter with IF=0, IF=1 without syscall confirmation, then
with confirmation; observed line counts are 0, 1 and 3, and remain 3 after
another publication. The final V-2 and V-3 ratchets each reject an isolated
copy of unmodified fe491f47 source with exit 101. The working kernel was not
mutated for those controls.

Evidence is under `serials/567/round2/`; copied paths, line endings and trailing
whitespace are normalized. The first local structure run was
68/71: three inherited inventories still required removed output. After
updating those inventories and preserving the KernelFrame counter, the next
run was 71/71. The final local run, including deferred marker publication,
also passed 71/71; `structure-suites.log` retains individual mutation verdicts.
Kernel formatting and diff-whitespace checks passed.

The initial x86 gate ran at ed03fd9b plus `round2-source.tar.gz`, at recorded
launcher load 0.10. It passed 71/71 preflight suites on attempt 1, its Rust
build, five loopback tests, the 32-cycle witness, and the full gate. The timer
observation was overrun_ms=44 with bound_ms=100. This run preceded deferred
marker publication and is not final-source runtime validation. The updated
source archive is `round2-final-source.tar.gz`, with per-file hashes in
`source-sha256.log`. Its x86 boot_tests build passed at launch load 0.65,
without project warning/error diagnostics. Runtime mutation and committed
revision gate evidence is appended below when collected.

claim-lint: python3 scripts/claim-lint.py -> exit 0

claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/567-round2-code-message.txt -> exit 0

### Round 2 not claimed

- Historical root-cause attribution or reproduction of the original 567 faults.
- Protection from arbitrary memory corruption or unsafe fabrication of a permit.
- Aarch64/SMP coverage, or a production-profile runtime result in this round.
- Final-source runtime validation from the initial gate that preceded marker deferral.

## Landing

Deferred code findings supplied for this continuation: [].

V-1: the wrong-slot write is identified mechanically at f416efbd:
`kernel/src/task/scheduler.rs:2705` publishes current_thread=next_thread_id;
the former public wrapper at fe491f47 selected and returned without installing
a CPU frame. If A remained executing while B was published, the destination
selected by `kernel/src/interrupts/context_switch.rs:603` would be B and the
RIP/RSP/RFLAGS assignments at `kernel/src/interrupts/context_switch.rs:622`
would copy A's interrupted state into B. The public entry at
`kernel/src/task/scheduler.rs:5823` now sets NEED_RESCHED, and the selecting
wrapper at `kernel/src/task/scheduler.rs:5829` requires the dispatcher permit.
This identifies the API failure mechanism prevented by construction; it does
not turn the 32 fresh or 1,595 inherited matching samples into a reproduction
of the historical fault. Those samples establish agreement only at their
recorded save, restore and resume observations.

V-4: provenance headers now explicitly mark the three standalone archived
structure captures as source-unverified. Their exact historical source identity
is not claimed, and they are excluded from revision-specific validation. Replacement
landing runs will record the revision they execute.

claim-lint: python3 scripts/claim-lint.py --files docs/planning/green-program/sched/567-KTHREAD-RESUME-2026-09-08.md docs/planning/green-program/sched/serials/567/structure-base-red.log docs/planning/green-program/sched/serials/567/resume-structure.log docs/planning/green-program/sched/serials/567/loopback-structure.log -> exit 1

claim-lint: python3 scripts/claim-lint.py --files docs/planning/green-program/sched/567-KTHREAD-RESUME-2026-09-08.md docs/planning/green-program/sched/serials/567/structure-base-red.log docs/planning/green-program/sched/serials/567/resume-structure.log docs/planning/green-program/sched/serials/567/loopback-structure.log -> exit 0

claim-lint: python3 scripts/claim-lint.py -> exit 0

claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/567-prose-message.txt -> exit 0

The merge of origin/main e664e4bc9fc83ba27f2a62cda987d7a170b282b3 into
4e3d58da completed without conflicts. The incoming strict aarch64 scorer adds
one BSSH_PUBKEY_ORACLE requirement. R182 therefore requires a fresh strict
serial fixture from the merged kernel; the incoming fixture contains an
appended marker and is not treated here as a new runtime capture. The
production scorer has no incoming requirement change.

claim-lint: python3 scripts/claim-lint.py -> exit 0

claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/567-merge-message.txt -> exit 0

At merge commit 1be090fe, the standalone structure suite passed 72/72.
The aarch64 boot_tests build completed but reported two project diagnostics:
the x86 logging import in scheduler.rs was unused, and the scheduler-only
loopback counter writer in net/mod.rs had no aarch64 caller after selection
became x86-only. Matching cfg guards remove those unused aarch64 items.
The rebuild passed with 0 project diagnostics; the pinned core
future-incompatibility notice remains unsuppressed. These guards do not
change x86 instructions or aarch64 ELR_EL1/SP_EL1 context restoration.
Kernel rustfmt and diff-whitespace checks passed.

claim-lint: python3 scripts/claim-lint.py -> exit 0

claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/567-cfg-message.txt -> exit 0
