# A.4 unit 11: arena filesystem cost

Disposition: stop under ARM ledger line 396's existing-kernel-path clause.
The suspected eager file-size work is absent at `a051a767`. This unit adds
one regression guard under U29 and changes no kernel or arena behavior.
The focus scenario's acceptance margin remains unresolved in #2915.

## Source trace at a051a767

`/tmp/uitest` is on the root ext2 filesystem. ARM selects its VirtIO block
root in kernel/src/fs/ext2/mod.rs:2575-2581 and registers `/` as ext2 at
2635. Name lookup switches root children only for virtual devices and the
home mount (kernel/src/fs/namei.rs:662-673); a generation's mutable `tmp`
binding still names an inode on that ext2 mount
(kernel/src/fs/ext2/generation.rs:11,29-31).

| Checkpoint operation | Kernel work and its size dependence |
|---|---|
| Create and extend | Arena::create calls create_new, set_len and writes the 16-byte format header (libs/libuitest/src/arena.rs:63-70). The libc ftruncate wrapper issues one syscall (libs/libbreenix-libc/src/lib.rs:862-866). sys_ftruncate calls resize_inode then begin_resize_file_as (kernel/src/syscall/fs.rs:5278; kernel/src/fs/ext2/mod.rs:1028). resize_file changes the size words; its only possible data I/O zeros one allocated boundary block for an unaligned old EOF (kernel/src/fs/ext2/file.rs:1191-1210). A new inode has old EOF 0: no data block allocation or clearing over the new extent. Growth of its mapping cache moves EOF without populating the new range (kernel/src/memory/file_map.rs:1056-1077). |
| Map the new headers | populate reads only the requested page range, clipped to EOF, not the whole file (kernel/src/memory/file_map.rs:1292-1324). Arena::map requests three header pages; screen/frame payload mapping is deferred (libs/libuitest/src/arena.rs:83-87). |
| Rename over a mapped file | The plan reads directory images and source/victim inode metadata (kernel/src/fs/ext2/rename.rs:135-189). commit_writes applies directory/inode changes; the unlinked victim is queued, or held by its remaining handles (373-382). It does not copy, clear or write the victim's data. Directory work depends on directory size, not arena file length. |
| Close and munmap | Arena fields drop in declaration order: File, headers, payloads (libs/libuitest/src/arena.rs:55-60). close drops the regular-file reference (kernel/src/syscall/pipe.rs:198-201); mappings retain their handles. unmap_range removes PTEs and bindings (kernel/src/syscall/mmap.rs:955-990; kernel/src/memory/file_map.rs:1623-1649). Binding::drop queues cache work without I/O (file_map.rs:441-468); the last orphan FileHandle queues finalization (kernel/src/fs/ext2/live_inode.rs:151-158). Unmap work scales with resident mapped leaves and VMA bookkeeping, not sparse EOF. |
| Final inode cleanup | The writeback cohort excludes orphan inodes (kernel/src/fs/ext2/live_inode.rs:296-299). finalize_pending_orphans discards their unobserved cache before disk reclamation (kernel/src/fs/ext2/mod.rs:329-344), including dirty frames, with no data write (kernel/src/memory/file_map.rs:904-913). Reclamation detaches pointers and frees allocated blocks in bounded batches (kernel/src/fs/ext2/reclaim.rs:105-149,192-226). Its work depends on allocated blocks, not sparse logical length. |
| Linked-file background writeback | writeback_snapshot selects dirty pages (kernel/src/memory/file_map.rs:702-720); mapped writes skip identical disk bytes (kernel/src/fs/ext2/file.rs:937-942). A writable shared mapping retains conservative dirty custody while stores can continue (file_map.rs:787-799,1918-1927). This is distinct from writing an unobserved orphan before freeing it. |

The retained boot `20261112T212413Z-qemu-36325` attributes 11651.428 ms to
16 calls of next_checkpoint. The timed region wraps the entire call
(userspace/programs/src/uitest_suite/proof.rs:736-738,761-763), including
creation, extension, the header write/maps, rename and old-arena drop.
Those records do not separate these operations, lock waits or block-device
latency. Assigning those milliseconds to one suboperation would be an
unsupported inference. No new boot, instrumentation or timing sample was
used. The source trace and regression guard do not establish an eager-file-size defect to repair here.

## Deterministic checks

`python3 tests/ext2_sparse_extension_test.py` passes on the unchanged
`a051a767` implementation. It executes the production resize, addressing
and range-read functions, substituting in-memory inode fields and a counted
block-device boundary. Extensions to 4096, 33832960 (desktop arena size),
and 1073741824 bytes leave the block count and 15 pointers at 0, transfer
0 sectors, and return 4096 bytes valued 0 from three distant page reads without
accessing the disk. It does not claim to exercise the complete syscall,
namespace, allocator or scheduler path. Compilation uses `-Dwarnings`.
This is an already-passing U29 regression guard, not a red-first repair.

`python3 tests/file_map_orphan_cache_test.py` passes both existing checks:
bounded dirty-cache discard refuses live bindings, and unobserved cache
retires before disk I/O, including when reclamation must retry.

The initial ext2 host runner could not compile: its namespace fixtures at
tools/ext2-host-tests/src/ext2.rs:585 and :605 lack `mutable`. Both that file
and kernel/src/fs/ext2/generation.rs match origin/main `5ee353ad`. #2917
tracks this separate fixture defect. The new check was moved into the
scoped host harness with its allocation and hole-read assertions retained.
The scoped harness's initial private-interface compilation errors were
corrected by matching fixture type visibility; no warning was suppressed.

## Validation and handoff

Check-only commit: `916b2ee4`.

The ARM64 testing-kernel release build and scoped x86 testing-kernel release
build exited 0, with `CARGO_BUILD_JOBS=4` under `taskpolicy -b`. The x86 build
emitted no warnings. ARM emitted no kernel diagnostic; Cargo reported a
future-compatibility notice from the pinned toolchain's `core` crate about
its AArch64 NEON intrinsics. No production source changed in this unit.
The full build commands and local log paths are in the effort ledger.

Lint invocations:

```text
claim-lint: python3 scripts/claim-lint.py --base a051a767 -> exit 0
claim-lint: python3 scripts/claim-lint.py --files /tmp/a4-u11-harness-issue.md -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg /tmp/a4-u11-check-message.txt -> exit 0
claim-lint: python3 scripts/claim-lint.py --files /tmp/a4-u11-report.md -> exit 1, then 0 after correcting prose
```

Final changed-hunk lint, report commit-message lint and push identities are
recorded in the effort ledger. There is no runtime fix or acceptance claim. No boot,
PR creation, merge, rebase, force push or issue closure belongs to this unit.
A coordinator decision on how to distinguish create/rename/drop and their
waits is needed before another performance change or acceptance attempt.
