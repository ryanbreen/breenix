# A.4 unit 14: mount-lock holders during mapped file I/O

Issue #2915; source baseline `44ea4d0c`; coordinator ARM ledger line 408(1)-(2).
This unit makes no boots. Acceptance and the scenario deadline margins remain
for the acceptance unit.

## Finding and evidence boundary

The background mapped-file service is a concrete mount-lock holder that can
exclude a concurrent create for the duration of another inode's data transfers.
At the baseline it acquires the exclusive mount guard at
`kernel/src/fs/ext2/writeback.rs:71-79`, then calls `finalize_writeback`, which
calls `MapState::service_writeback` while retaining that guard
(`kernel/src/fs/ext2/mod.rs:283-327`). Each service batch can copy 64 dirty pages,
write their ext2 blocks and flush the device before releasing the guard
(`kernel/src/memory/file_map.rs:53-54,691-784,801-863`). The data loop reads existing
blocks, initializes new blocks and waits for block writes at
`kernel/src/fs/ext2/file.rs:885-997`.

This establishes a blocking mechanism, not its duration in the retained boot.
The retained `20261112T222818Z-qemu-60202` record reports 11616.545 ms elapsed
and 892.690 ms thread CPU across 16 focus-case opens (ledger line 407).
Those records do not identify the actual holder of any individual wait.
The deterministic device check below exercises the exclusion and the repair
without assigning the captured 10723.855 ms off-CPU interval to this holder.

## Baseline holder census

Line numbers in this section refer to `44ea4d0c`. Root accessors and the
`namei::Mount` wrappers use the same `ROOT_EXT2` guard:
`kernel/src/fs/ext2/mod.rs:2547-2566,2719-2770` and
`kernel/src/fs/namei.rs:64-90`. A cached device read can avoid backend I/O;
the listed scopes also cover a cache miss or an actual write/flush.

| Holder / operation | Guard and transfer chain | Can overlap creation of the next arena? |
| --- | --- | --- |
| Background linked-file writeback | `ext2/writeback.rs:71-79` -> `ext2/mod.rs:283-327` -> `file_map.rs:801-863,691-784` -> `ext2/mod.rs:699-709,782-905` -> `ext2/file.rs:885-997`. Exclusive guard spans dirty-page data reads/writes, allocation metadata and flush. | Yes. Binding drops request writeback and eviction at `file_map.rs:441-470`; this service is independently scheduled. The linked-inode selector excludes already orphaned objects (`ext2/live_inode.rs:289-312`), but a linked object's transfer can start before rename/unlink can acquire the guard. |
| Foreground mapped writeback: fsync, msync and synchronous unmap | `file_map.rs:98-133` takes `write_mount` for each 64-page batch and its final barrier; `syscall/fs.rs:4927-4949` and `syscall/mmap.rs:1030-1105` reach it. Same data path as background writeback. | The case's own calls are sequential, but another thread/process can perform a mapped sync concurrently. Synchronous unmap writes only dirty pages through the same filter (`file_map.rs:705-722`). |
| mmap population, the prefill for later faults | `syscall/mmap.rs:540-559` holds a read guard through `file_map::populate`; `file_map.rs:1283-1337` loops over missing pages and calls `Ext2Fs::read_file_range` -> `ext2/file.rs:249-391`. | Yes for another mapper. The focus case also maps its arena's headers and payloads. Sparse holes supply cleared bytes; existing file data can issue block reads under this guard. |
| Actual file page fault / MAP_SHARED dirtying | `file_map.rs:1818-1927` uses the resident cache or allocates cleared pages and the map lock; `1900-1912` supplies a cleared page for a hole and `1918-1922` marks shared writable pages dirty and advances their generation. | Runs during captures, but takes no mount guard and performs no device I/O. It is not a mount-lock I/O holder. |
| Poisoned-page repair after a failed write or resize | `ext2/mod.rs:770-777,1056-1066` -> `file_map.rs:1097-1155`; reloads the inode, then reads each covered poisoned page under the mutator's exclusive guard. | Possible following a file mutation failure. The successful arena path does not create poison. This error-recovery path remains synchronous in this unit. |
| Orphan reclamation | `ext2/writeback.rs:90-99` -> `ext2/mod.rs:329-424,455-512`; `ext2/reclaim.rs:105-164` reads/detaches pointer blocks and persists the inode; `171-258` performs the detachment barrier, releases bitmap bits and writes counts. Batches use 64 tail blocks and a 128-transition guard budget. | Yes. Arena replacement unlinks the old inode; its last handle queues reclamation. Dirty unobserved cache pages are discarded before disk reclamation (`ext2/mod.rs:334-352`), rather than written as file data. The device work here is pointer, inode, bitmap/count metadata and barriers. |
| Deferred / foreground truncate suffix work | `ext2/mod.rs:427-453,2430-2452,2774-2848` and `ext2/reclaim.rs:105-258`. A foreground shrink ticket or the service advances bounded metadata batches; guards are released between batches. | Possible after another file's truncate, not the arena's sparse extension. Partial-block tail zeroing at `ext2/file.rs:1185-1212` is synchronous data I/O for a shrink. |
| Global sync, per-file barrier without dirty pages, shutdown | `syscall/fs.rs:4950-4977,5153-5190`; `ext2/mod.rs:998-1006,3041-3082`. Global sync stages dirty handles then flushes under the exclusive guard; shutdown also finalizes metadata, marks the superblock clean and fences writes. | Another process can call sync/fsync during a case. Shutdown is after user processes have been reaped, not concurrent with an ordinary live focus case. |
| Ordinary read/pread and executable loading | `syscall/handlers.rs:1392-1441,4087-4124` take read guards for coherent ranged reads; `1962-1977` holds an exclusive guard through a coherent executable copy. `ext2/mod.rs:592-694` reaches `read_file` / `read_file_range`, following direct and indirect block pointers. The exec loop reads 64 KiB chunks while retaining the guard. | Possible from other processes; app startup can read executables. These ordinary synchronous syscall readers are separate from mapped-cache writeback/population and remain unchanged. |
| Ordinary write/pwrite, truncate and namespace operations | `syscall/handlers.rs:708-787,4215-4244` -> `ext2/mod.rs:755-905`; truncation is `syscall/fs.rs:5228-5372` -> `ext2/mod.rs:1028-1107`. Create/open takes the write guard at `syscall/fs.rs:770-774`; namespace lookup/directory/rename/link/unlink and attributes use `namei::Mount` / `write_mount`. `ext2/mod.rs:531-590,944-979,1110-1758`, `ext2/create_node.rs:80-192`, `ext2/rename.rs`, `ext2/directory_io.rs`, `syscall/metadata.rs:135,209,239,254` contain the inode, directory, bitmap and ordered metadata transfers. | Yes in general. The arena's own extension stays sparse and its calls are sequential. Ordinary synchronous write syscalls and namespace transaction I/O retain their existing mount guard in this unit. |
| Accounting, proc state and health probe | `ext2/mod.rs:260-267,2806-2848,3025-3035`; `syscall/fs.rs:1520-1566,1723-1755` includes metadata and directory reads; `drivers/watchdog_round.rs:59-64` reads the uncached superblock/group table and root inode. | The health probe can run independently; a process can read proc state or directories concurrently. These are metadata reads or reclaim/barrier work, not a multi-MB dirty data transfer. |
| Mount installation, boot configuration, special probes and tests | Mount installation holds the raw root write guard across `activate` at `ext2/mod.rs:2700-2710` (`activate` is `211-220`). Read-only boot/config consumers are `boot/target.rs:176-201`, `main_aarch64.rs:45-65,2201-2245`, `gpu/mod.rs:5890-5895`, `gpu/gk208/c2/runtime.rs:273-278`, `drivers/watchdog.rs:179-184`, `drivers/xhci/fault.rs:51-57`. The watchdog fault-intent consumer reads/truncates/flushes at `drivers/i6300esb.rs:306-320`. `fs/fault_inject.rs:587-590` and `test_framework/registry.rs:8595` are explicit test consumers. `fs/ext2_lock_race.rs:125-127` deliberately waits under a read guard but does not issue file data I/O. | Installation and startup consumers precede the live case; the x86 GPU consumers are outside the ARM build. Diagnostic/test paths require their respective modes. An interactive file reader is possible in desktop mode. Their existence is not evidence that they held the lock in the retained suite run. |

The census includes both multi-block file work and the shorter metadata holders;
it does not claim that this unit removes mount locking from ordinary read/write
syscalls, namespace transactions, reclamation metadata or explicit metadata
barriers.

## Repair

`ext2/mapped_io.rs` owns a pinned inode and device reference plus a per-inode
transfer lease. Opening it, reserving allocation bits and publishing pointers
uses short namespace scopes. File block reads, comparisons, initialization,
writes and mapped-data barriers happen outside those scopes.

A batch reserves data allocation bits before releasing the namespace. Other
creators therefore see those bits occupied. The block image is initialized in
full before publication of a new pointer. On a data failure, completed blocks
may be linked, but uncompleted reservations are released without linking their
partially written bytes. An uncertain metadata publication fences the mount.
Pointer allocation and its free-count changes remain serialized with concurrent
namespace changes; publication reloads the inode to preserve their link counts
and attributes. The transfer retains kill custody until reservation outcomes
have been settled, including after a pending SIGKILL.

Ordinary writes, pwrite, truncate and O_TRUNC wait for the affected inode's
transfer without holding the namespace. The O_CREAT|O_TRUNC race rechecks the
parent's current entry after a wait. A create of another inode does not take
this lease. Background selection pins a bounded set of inode identities, then
releases the namespace before servicing their dirty pages.

Mmap population reads through the same stable inode/device snapshot and retains
its lease until the binding is published under the mount read guard. Metadata
service skips eviction of an inode with a transfer lease, preventing eviction
between population and binding. The actual page-fault/dirtying path is unchanged.
Poison repair remains with its synchronous mutation error path.

Dirty-page filtering, generations and the writable-shared rule remain in
`file_map.rs`: a writable shared mapping retains dirty custody; a candidate is
cleared only after a barrier and the existing generation checks. A per-inode
write sequence prevents a detached barrier from certifying metadata submitted
while it ran. Successful per-file barriers certify that inode rather than
advancing the mount's epoch for unrelated in-flight writes. Global sync takes
its staged inode leases in inode order and performs its barrier outside the
namespace. Shutdown waits for active leases before its final clean/read-only
transition. No interrupt, scheduler or Tier-1/Tier-2 implementation is changed.

## Deterministic checks

Check-only commit `70527a2a` precedes the repair. Its production service body,
a fake device and a nonblocking namespace acquisition fail on `44ea4d0c` with
`create_new waits for another inode's in-flight data write`; no wall-clock
assertion is involved. The fixture's initially missing ARM preemption stubs
were corrected before that valid red run (`/tmp/a4-u14-red.log`).

<!-- claim-lint:ok: tests/ext2_mount_data_io_test.py asserts the fake device write list is empty after an identical production transfer. -->
`tests/ext2_mount_data_io_test.py` now runs two focused host checks. The service
check follows the production selection/guard scopes. The transfer check runs
the production mapped transfer, allocation, inode, namespace-create and metadata
batch code against an in-memory device. The device holds a data completion
until a production create has finished on the same mount. It checks that the
new data pointer is still absent at that point, then checks the resulting direct
and indirect file data. It also checks a partial data write followed by retry,
release of unlinked reservations, an identical snapshot emitting zero writes,
clearing set-ID bits even for an identical snapshot, and metadata arriving
during a detached flush retaining its durability need.
The final service fixture also rejects the baseline with the same held-data
assertion (`/tmp/a4-u14-final-fixture-red.log`). Kernel thread, scheduler,
cache-frame and mount-access primitives are host
fixtures; this is not guest execution or a scenario-margin measurement.

The focused pre-existing guards cover sparse extension, orphan cache discard,
namespace insertion I/O and allocation rollback, and seven mount-acquisition
properties. They retain their assertions. The new checks and these guards pass;
logs are `/tmp/a4-u14-check.log`, `a4-u14-sparse.log`, `a4-u14-orphan.log`,
`a4-u14-create.log` and `a4-u14-lock.log` in the same temporary directory.

## Release validation

Both architectures use the shared ext2 and mapped-file code. Build commands:

```sh
export CARGO_BUILD_JOBS=4
taskpolicy -b cargo build --release --features testing --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
taskpolicy -b cargo build --release --target x86_64-unknown-none --features testing -p kernel --bin kernel
```

Both commands exited 0. The x86 log has no warning/error diagnostics. The ARM
log has no kernel warning/error diagnostics and retains the pinned toolchain's
previously recorded `core` future-compatibility notice. Logs:
`/tmp/a4-u14-arm-verified.log`, `/tmp/a4-u14-x86-verified.log`.

ELF SHA-256:

- ARM64: `0da0ae3836199195a36888b85481f86358887bfab4e26673cb2769e0f6b3b1b6`
- x86: `0df6ae8767bffffd5598080513e597e54cff96d01a164922f82b8cd99936e6cd`

`claim-lint.py --base 44ea4d0c` exits 0 for this unit's 13 changed files;
commit-message checks and `git diff --check` also exit 0. The tool reports
pre-existing findings outside the changed hunks separately. No expectation,
deadline, scenario count, comparison or suite definition is changed.

No acceptance verdict, board update or issue closure is made by this unit;
the next acceptance unit judges the existing scenario limits. Ordinary
synchronous readers/writers and metadata holders listed above remain separate
possible sources of contention if acceptance still finds insufficient margin.
