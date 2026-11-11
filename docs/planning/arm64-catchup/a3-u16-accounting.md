# A.3 restart accounting — a3-u16-accounting

This unit implements coordinator ledger line 198(1)(a)–(c), building on the
u13 stall ring. It adds observation records only. Scheduling decisions, disk
serialization, application behavior, case assertions, deadlines and manifests
are unchanged. No boot, deployment, PR creation or merge is performed here.
The unresolved mechanisms in a3-mechanism.md remain unresolved until measured.

The check-first commit is `abe8b2a4`. Its two host checks failed on the missing
accounting journal and producer hooks, before production changes. After the
implementation they pass 2/2. The numeric test executes the production journal
with explicit timestamps: a 100 ns window partitions the compositor leader
into 40 ns dispatched, 30 ns runnable and 30 ns blocked (20 ns on waitqueue 99,
10 ns on completion 88). It includes a worker born inside the window, a
competing process, queue samples, file entry/return stamps, a disk lifecycle,
and an unfinished window/request/syscall readable after the observer times out.
Overflow is retained as a count. This is a check of record collection and
arithmetic, not a model claiming to explain the earlier 1921 ms stall.

## Collection boundaries and interpretation

`window::exec_done_current` recognizes the committed bwm image and opens the
window before returning to that image. This is a kernel exec boundary, earlier
than bwm's userspace `compositor-exec` mark. The current owner is recorded as
on-CPU even though exec commit temporarily leaves its scheduler row Ready.
`window/scene.rs::finish` closes the window on that claimant's first successful
composite. A subsequent bwm exec truncates an unfinished predecessor with
`complete=0`; a watchdog read also exports an unfinished window through its
snapshot time with `complete=0`.

Scheduler transition sites publish numeric state through `Thread::set_state`;
initial scheduler publication seeds the thread's prior state. The journal
retains that state outside collection windows, so a thread already queued or
blocked at exec is accounted from the boundary. Closed intervals and the open
intervals copied at completion/export are clipped to the window; overlapping
copies are unioned before summation. Newly created threads start at their
publication, and terminated threads stop contributing at termination.

`account-thread` reports the scheduler's dispatch/state intervals in nanoseconds:
`on_cpu_ns`, `runnable_ns` and `blocked_ns`. `account-wait` partitions blocked
intervals by numeric wait kind and object identity. Kinds 1–5 match u13
(completion, waitqueue, futex, poll set, child selector); 6 is timer, 7 signal,
8 an otherwise unnamed scheduler wait. A timer has no separate object pointer.
Kernel work has owner PID 0 and remains distinguishable by TID/name. IRQ work
inside a dispatch interval is charged to its interrupted thread, as with the
existing thread CPU clock; these records do not measure host vCPU descheduling
separately from guest execution.

Thread identities include the raw owner PID, TID, thread group and up to 24
bytes of scheduler name (whitespace becomes underscores). Clone workers retain
their group ID, so a worker's raw owner PID need not equal the replacement PID.
Identity rows appear on that thread's first transition in the window; the exec
owner is also identified at the boundary. Threads that stay blocked throughout
can have time rows without a new identity row. Other processes that dispatch
in the window get their own identity/time rows.

The window records the guest online vCPU count. Run-queue samples come from
ARM's existing dispatch trace point and the parent's procfs polling. Dispatch
samples use `cpu=18446744073709551615` for the total queue length; polling
samples name an individual CPU, including empty queues. On x86 a busy owner
returns `len=18446744073709551615`, meaning unavailable, rather than an invented
length. Samples have their actual guest timestamps; the parent poll is not a
fixed-frequency sampler.

## Disk and file timing

The ARM MMIO and x86 PCI virtio block transports allocate a request ID before
acquiring the shared disk gate. A request retains submitter PID/TID, device,
operation, sector and byte count, then these timestamps:

| Field | Observation point |
| --- | --- |
| `enqueue_ns` | Request scope created before waiting for the gate |
| `gate_ns` | Gate compare-exchange succeeds |
| `submit_ns` | MMIO: just before publishing avail.idx; PCI: successful add_chain, before releasing the IRQ-safe queue lock |
| `irq_ns` | Completion IRQ consumes the used descriptor, before notifying the completion primitive |
| `wake_ns` | First blocked-to-ready/running scheduler publication after that request's IRQ |
| `resume_ns` | Submitter observes a successful completion wait return |
| `release_ns` | Guard is about to release its gate after completion/copying |

Absent stages retain timestamp 0. In particular, an IRQ consumed without a
parked waiter has no scheduler wake; `resume_ns` records the submitter's
observation separately. A gate refusal, abandoned/wedged request or failed
submission can be distinguished by missing later stages. A no-op PCI flush
can acquire/release the gate without a submit stamp. The release stamp precedes
the unlock store by the instrumentation call's cost. These are clock samples
at software observation points, not a claim to timestamp the device's internal
execution. Read/write/flush decisions, wait policy and wedge behavior remain
unchanged.

MMIO device identities are indices; PCI identities are kernel device-object
addresses. These are guest object identities, like the wait objects, rather
than host site configuration. Requests are collected outside startup windows
as well, preserving a competing request that began before exec. The exporter
selects lifetimes overlapping each window. Request ID, device and gate
acquire/release times identify the request ahead of a waiting submitter.

The syscall provider records open/stat/read/close family entry and return
stamps during the window, with raw architecture ABI number, PID/TID and signed
return value. Competing submitters are retained too. ARM includes openat 56,
close 57, read 63, pread64 67, newfstatat 79, fstat 80 and statx 291; x86 includes
read 0, open 2, close 3, stat 4, fstat 5, lstat 6, pread64 17, openat 257,
newfstatat 262 and statx 332. Entry and return are separate rows, so an
unfinished bwm call survives a watchdog. Read return values supply actual
byte counts, including EOF, rather than assuming the earlier conditional
189-request calculation occurred in this boot.

## Retention, export and artifact gate

Storage is fixed: 32768 numeric events, 1024 current thread slots and 4096
request slots. Producers use atomic words and a one-attempt cell publication;
contended cells contribute loss counts. Formatting and interval summation run
in the parent-triggered procfs task context. The journal header reports event
and disk overwrites, producer losses, thread-slot losses and cells unavailable
during the snapshot. Historical overwrite counts can include earlier windows;
a window's start record must still be retained to interpret its accounting.
The export has an 8 MiB ceiling plus its last row/truncation marker. A
truncation marker or loss requires inspection before claiming whole-window
accounting; missing stamps are not inferred.

`/proc/breenix/window-stall` appends these rows to u13's snapshot export. Procfs
captures the content at open, so a multi-read userspace copy sees one snapshot.
The suite parent exports after capturing each case outcome (on pass or fail),
before watchdog SIGKILL, and at its existing finale before READY. Completed
windows have an export cursor to avoid printing them at each later case; their
kernel storage remains retained until overwritten. Open windows remain
exportable at a later watchdog/finale. The case child does not own the data.
No Tier 1 or Tier 2 file is edited.

The suite constructs serial labels as `VALUE wm-` plus a kernel row. The full
`wm-...` string need not be stored contiguously in an ELF. The acceptance gate
should inspect the following stored forms in **both kernel ELFs**:

| New serial label | Stored kernel form |
| --- | --- |
| `wm-account-buffer` | `account-buffer` |
| `wm-account-window` | `account-window` |
| `wm-account-thread` | `account-thread` |
| `wm-account-wait` | `account-wait` |
| `wm-account-runqueue` | `account-runqueue` |
| `wm-account-identity` | `account-identity` |
| `wm-disk-request` | `disk-request` |
| `wm-file-syscall` | `file-syscall` |
| `wm-account-export-truncated` | `account-export-truncated` |

Both suite ELFs retain `VALUE wm-`, `/proc/breenix/window-stall` and
`/proc/breenix/window-stall-sample`; the new suffixes belong to the kernel,
not bwm or the suite. The u13 and line-185 label inventories still apply. This
unit does not stage the disk: the separately directed measurement must verify
byte identity after its normal build, then use the new kernel label inventory
alongside the existing disk-suite gate.

## Validation

The accounting host checks pass 2/2; u13's retained-stall checks pass 3/3; the
existing disk request lifetime structural guards pass 14/14 with `-Dwarnings`.
The suite runner builds pass for ARM64 and x86 release, with no compiler
warnings. Builds use four jobs under background task policy. The pinned Rust
library verification succeeds; upstream fork freshness was not verified by
the environment's attempted fetch. No source from another C library was read
or incorporated.

Both kernel release builds pass with no compiler warnings. Cargo emits the
existing pinned-core future-compatibility advisory for the ARM kernel. The
ARM kernel FP/SIMD guard passes: 0 forbidden loads/stores, 32 allowlisted.
The nine stored labels above were found in each built kernel; the prefix and
proc paths were found in each suite ELF.

| Built artifact | SHA-256 |
| --- | --- |
| ARM kernel, `target/aarch64-breenix-kernel/release/kernel-aarch64` | `e2d0bd40bcef4b6f63a521ce7dc0b628e78e4078589cc165e383cbfae014628c` |
| x86 kernel, `target/x86_64-unknown-none/release/deps/artifact/kernel-cb6f47ce5ef18fd1/bin/kernel-cb6f47ce5ef18fd1` | `faf9cfd330f1976044ea4f1d7c5a8436ed84a1bffe9e61e22f54f6c892256b58` | <!-- claim-lint:ok: Cargo artifact ELF byte inspection found 9 of 9 accounting labels; this row records its SHA-256. -->
| ARM suite, `userspace/programs/target/aarch64-breenix/release/suite-window-manager` | `acfefbf0b31bc05022fcbc95b008cc72812b7bd8be1fe343044681067ed1a052` |
| x86 suite, `userspace/programs/target/x86_64-breenix/release/suite-window-manager` | `2f6f9bcf1ac2c71dbac36167e2dd724703a5938e1f50e983c89416a8cf880190` |

The x86 command is `cargo build --release --bin qemu-uefi`; its kernel is an
artifact dependency. The top-level `target/x86_64-unknown-none/release/kernel`
still hashes `3bdf67c4...` from the earlier unit and contains 0/9 new labels.
Use the artifact dependency recorded above for this build's hash/label check.
No disk copy or boot-image identity is claimed by this unit.

Validation invocations (release commands use CARGO_BUILD_JOBS=4 and taskpolicy -b):

- `python3 tests/a3_accounting_test.py` -> pass 2/2 after failing 2/2 before implementation.
- `python3 tests/a3_stall_snapshot_test.py` -> pass 3/3.
- `rustc --edition=2021 --test -Dwarnings tests/block_request_lifetime_structure.rs` with the repository CARGO_MANIFEST_DIR, then its binary -> pass 14/14.
- `cargo build --release --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64` -> exit 0.
- `cargo build --release --bin qemu-uefi` -> exit 0.
- Scoped `cargo build --release --target ../../aarch64-breenix.json --bin suite-window-manager` in the programs workspace with build.sh's pinned std environment/link flags -> exit 0.
- The matching x86 suite command, with `--features framebuffer-v1` -> exit 0.
- `scripts/check-kernel-no-neon.sh` on the final ARM ELF -> exit 0.
- `scripts/claim-lint.py --base abe8b2a4` -> exit 0.
- `scripts/claim-lint.py --base origin/main` -> exit 0.
- `scripts/claim-lint.py --files tests/a3_accounting_test.py` -> exit 0 before the check commit.
- `scripts/claim-lint.py --commit-msg /tmp/a3-u16-check-commit.txt` -> exit 0.
- `scripts/claim-lint.py --commit-msg /tmp/a3-u16-implementation-commit.txt` -> exit 0.
- `git diff --check` -> exit 0.

The initial ARM build used a boot-tests-only CPU-count helper; it was replaced
with the architecture's production counter. The initial x86 compile needed a
mutable sample guard. Initial suite commands omitted the forked std root, then
used an unnormalized root path; the corrected commands use build.sh's canonical
pinned library and link configuration. These were compile/command errors,
resolved before the builds reported above; no expectation was changed.

Existing delayed reap/native stack-restored questions remain under issue #2872;
this instrumentation does not close them.
