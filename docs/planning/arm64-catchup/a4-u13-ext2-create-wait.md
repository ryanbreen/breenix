# A.4 unit 13: ext2 create I/O and ARM completion waits

Scope: coordinator ARM ledger line 404 (1)-(3), starting at `6b346c36`.
No boot belongs to this unit. Issue #2915 remains open for acceptance.

## Source count

Read-only inspection of the retained `20261112T220527Z-qemu-10255` disk
confirms 312320 ext2 blocks, 1024-byte blocks, 39 groups, 2000 inodes per
group and 256-byte inode slots. `/tmp/uitest` occupies one direct block.
The image's descriptor table occupies two blocks: ceil(39 * 32 / 1024).
The corresponding serial mount record reports the same block size and block
count. This is existing evidence, not a new run.

For a regular create with room in that directory, `create_file` reads the
parent and directory, checks the name, then calls `create_entry`
(kernel/src/fs/ext2/mod.rs:963-986). `create_node_prepared` follows the same
sequence (kernel/src/fs/ext2/create_node.rs:76-89); the host fixture executes
that production entry point, including `create_entry`, directory parsing,
inode transfers, descriptor serialization and rollback. Directory growth,
pathname resolution and mount-lock acquisition are outside this fixture.

The count below is the cold, uncached ext2 insertion path, with a changed
parent timestamp. ARM's 512-byte backend batches reads but uses the trait's
sector-at-a-time default for writes (kernel/src/block/virtio.rs:197-223;
kernel/src/block/mod.rs:380-389). Counts are requests and
transferred sectors, not elapsed time.

| Work | Read requests / sectors | ARM write requests / sectors at baseline |
|---|---:|---:|
| Parent inode | 1 / 1 | 0 / 0 |
| One directory block | 1 / 2 | 0 / 0 |
| Inode bitmap claim | 1 / 2 | 2 / 2 |
| Replaced inode | 1 / 1 | 0 / 0 |
| New inode read-modify-write | 1 / 1 | 1 / 1 |
| Changed directory block | 0 / 0 | 2 / 2 |
| Parent inode read-modify-write | 1 / 1 | at most 1 / 1 |
| Whole group-descriptor table | 0 / 0 | 4 / 4 |
| Superblock read-modify-write | 2 / 2 | 2 / 2 |
| Total | 8 / 10 | at most 12 / 12 |

The bitmap search skips groups whose in-memory free count is 0 before
reading a bitmap; it returns after the first eligible free bit
(kernel/src/fs/ext2/block_group.rs:302-320,332-385). With accurate
counts and no quarantine exhaustion, filling earlier groups therefore does
not add device reads. Keeping an orphan allocated also keeps its bitmap bit
set; it does not require rereading previous groups during allocation.
A quarantine-exhausted group could require another bitmap read; this unit
does not claim to change that separate path.

The inode transfers cover only the sector containing the 128-byte inode
(kernel/src/fs/ext2/inode.rs:170-184,450-478). Block transfers use the device
multi-block methods (kernel/src/fs/ext2/file.rs:21-78). The superblock reads
and rewrites its two sectors (kernel/src/fs/ext2/superblock.rs:191-229).

Mounted ext2 has a bounded, write-through sector cache:
read hits issue no device request, and successful writes populate it
(kernel/src/fs/ext2/mount_device.rs:46-58,65-98). Thus the table is an
uncached upper count for this insertion, not a claim that an open necessarily issues
eight hardware reads. Additional pathname/pin/open-inode reads occur in
sys_open_write_path and sys_open_from (kernel/src/syscall/fs.rs:372-403,
425-428,602-624,770-774). Their cache state and time waiting for the mount
cannot be recovered from the per-call capture.

The concrete growth defect is counts publication: baseline write_table
serializes and writes the entire descriptor table, although an empty-file
create changes one descriptor. With G groups and 1 KiB blocks, it issues
2 * ceil(G / 32) ARM sector writes for that table alone. The total insertion
writes are at most 8 + 2 * ceil(G / 32): 10 at one group, 12 at the A.4
39-group shape, 26 at 257 groups. It grows with unrelated filesystem size.

## ARM wait and wake path

ARM uses block_mmio, not the PCI block driver
(kernel/src/block/virtio.rs:10-12,197-205). read_chunk acquires the request
gate, prepares a fresh completion token before notifying the device, then
waits for it (kernel/src/drivers/virtio/block_mmio.rs:1082-1107).
Writes follow the same protocol (1218-1252).

BlockMmioCompletion::wait_for_completion calls
Completion::wait_timeout_uninterruptible in one-second health-check slices;
completion ends the slice immediately, rather than waiting for its expiry
(block_mmio.rs:405-428). Completion registers the thread and fences before
checking done (kernel/src/task/completion.rs:390-397). The syscall path
atomically checks done and publishes BlockedOnIO with the deadline under the
scheduler lock, checks done again, then schedules off CPU on ARM
(completion.rs:488-535). finish_wait_current takes back a published blocked
state on return (completion.rs:138-152,549-555). Early boot uses the separate
bounded poll path, not this syscall sleep path.

The ARM IRQ dispatcher routes the block IRQ to handle_interrupt
(kernel/src/arch_impl/aarch64/exception.rs:2535-2541). The driver acknowledges
the interrupt, reads the used descriptor and status, then completes its
pending token (block_mmio.rs:1362-1399;453-457). Completion::complete stores
done, fences, emits SEV and calls isr_unblock_for_io for the registered
waiter (completion.rs:699-717). That buffers its wake and sets need_resched
(kernel/src/task/scheduler.rs:10335-10340). IRQ exit checks rescheduling after
EOI (exception.rs:2682-2688); schedule_deferred_requeue drains the wake buffer
before choosing a thread (scheduler.rs:4619-4625). The ARM idle loop also
checks both reschedule and pending wakes before sleeping and after the
interrupt (kernel/src/arch_impl/aarch64/context_switch.rs:7275-7299,7378-7395).

This is already an interrupt-woken protocol. Source reading does not establish
a mandatory tick/timeout delay to repair here. No completion, scheduler,
interrupt, Tier-1 or Tier-2 code changes in this unit. This trace does not
attribute the retained capture's roughly 0.7-second wait to descriptor writes;
the counted defect and the capture's elapsed-time cause are distinct evidence.

## Repair and deterministic check

Check-only commit: `5d447395`. `python3 tests/ext2_create_io_test.py` uses the
existing sparse-extension harness's run helper and counted device boundary,
with production create and metadata functions. After fixture compilation was
corrected, its red run on unchanged `6b346c36` compiled with -Dwarnings and
failed on the 39-group shape: 12 write sectors instead of the required 10.
The red log is `/tmp/a4-u13-red.log`.

write_table_changed now compares planned and saved descriptor images and
writes only changed table blocks (kernel/src/fs/ext2/block_group.rs:125-173).
Write::Counts supplies the reverse images for undo
(kernel/src/fs/ext2/directory_io.rs:143-151). Regular creation changes one
descriptor, so it writes one table block regardless of group count. A node
also allocating a block can change at most two descriptor blocks. In-memory
scanning/copying of the descriptor vector still scales with group count.

The creation plan, its ordering comment and its commit boundary are unchanged
(kernel/src/fs/ext2/create_node.rs:98-103,153-197). Bitmaps precede inode and
name publication; parent and counts remain last. Undo restores the same
changed count blocks, even if a failed write partially landed. Full-table
writers used outside namespace creation retain their existing behavior.

The green run passes (`/tmp/a4-u13-green.log`): 16 sequential creates each on
1-, 39- and 257-group fixtures issue exactly 8 read requests / 10 read sectors
and 10 write sectors. Earlier groups are full and the allocator uses the last
group, exercising table offsets and the partial final descriptor block.
The fixture checks write order and injects writes that land before reporting
an error, in the changed descriptor block and in the superblock; rollback
restores all sector bytes in both cases. It does not use timing assertions.
No expectation was loosened between the check commit and the repair.

## Validation and handoff

Build commands use `CARGO_BUILD_JOBS=4` under `taskpolicy -b`:

```text
cargo build --release --features testing --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
cargo build --release --target x86_64-unknown-none --features testing -p kernel --bin kernel
```

The ARM64 build exited 0 with no kernel diagnostic
(`/tmp/a4-u13-arm-release.log`). Cargo repeated the pinned core toolchain's
previously recorded NEON future-compatibility notice. ARM ELF SHA-256:
`4b8d002473a355ed9f988c5f9620702a79f83fbd60293d8a6a55c2726b6564ad`.

The x86 build exited 0 with no warnings or errors
(`/tmp/a4-u13-x86-release.log`). x86 ELF SHA-256:
`e8f0b29cb7c67f456e032448876951cf8bf003ae488f1a010dbe71e89db96f36`.

The focused host check passed; no test/build was repeated after commit or
push. Final claim-lint and synchronization identities are recorded in the
effort ledger. Initial report lint flagged three absolute-wording sentences;
those were corrected to precise scoped claims.
Acceptance, issue closure and PR work remain with the acceptance unit.
