# B4's `is_valid_user_range` swap breaks EVERY aarch64 userspace process's first write() -- NEW, BLOCKING

> **CORRECTION (fix round 3, commit `a1b7eaf7`):** The "Root cause" section
> below, and the #729 reopening comment that repeated it verbatim, is
> **wrong**. It attributes the failure to `is_valid_user_stack_range`'s
> hardcoded 1 MiB aarch64 window, and claims that window is "too narrow for
> a real process's stack" -- but every aarch64 process's stack top is fixed
> at `USER_STACK_REGION_START` with a 64 KiB (`USER_STACK_SIZE`) extent
> (`process/manager.rs`), comfortably inside 1 MiB, so init's first
> `print!()` cannot have been failing on a stack address at all. The
> closure review (`sweep3/fix2-review.md`, finding B4-b) caught this: no
> address from the actual failing boot was ever quoted in this document --
> the mechanism was inferred from reading the predicate, then labeled
> "traced."
>
> The real cause, confirmed by decoding the actual failing address: this
> libc's `malloc` is implemented as `mmap`
> (`libs/libbreenix-libc/src/lib.rs`), so `print!`'s heap-allocated
> `LineWriter` buffer is an mmap'd address, landing at
> `~0x7ffffdf86000` -- just below `MMAP_REGION_END`
> (`0x7FFF_FE00_0000`). `kernel/src/memory/vma.rs` hardcoded that region's
> bounds with **no `#[cfg]` at all** (unconditionally the x86_64 numbers),
> and those are exactly what `sys_mmap`/`Process::new`'s `mmap_hint`
> seeding actually consume on **both** arches. Meanwhile
> `layout::is_valid_user_range`'s mmap arm validated against a second,
> disjoint aarch64-only window (`aarch64_const::MMAP_REGION_START/END`,
> `[0x1_0000_0000, 0xFF_FE00_0000)`) that no allocator ever used. The
> validator was refusing an address class the allocator had always
> legitimately handed out on aarch64; x86_64 was accidentally fine only
> because its two copies of the mmap region happened to agree.
>
> The stack arm's hardcoded 1 MiB window was real (M-a in the review) --
> it disagreed with the demand-paged growth handler's own
> `MAX_USER_STACK_SIZE` constant on x86_64, and used an unexplained
> literal instead of `USER_STACK_SIZE` on aarch64 -- but it was a
> **secondary, non-blocking** defect, not the cause of this boot failure.
> Both defects are now fixed: the mmap region is one arch-generic
> definition consumed by both the allocator and the validator, and the
> stack arm on both arches derives from the real per-arch constant that
> governs its allocator/growth-handler instead of a hardcoded literal. See
> `kernel/src/memory/layout.rs` and the commit message on `a1b7eaf7` for
> the full account.

Found while running the prove round's mandated regression leg
(`./docker/qemu/run-aarch64-full-test.sh --boot-tests-only`, per fix2-notes.md's
own claim that aarch64 verification was "clean build, zero warnings" -- a
COMPILE check only, never an actual boot of this feature profile).

## Symptom

`init` (PID 1) panics on its very first buffered `print!()` call --
literally the first line of `main()`, `print!("[init] Breenix init starting
(PID {})\n", pid)` -- with `Bad address (os error 14)` (EFAULT). init exits
134, no other userspace process is ever spawned (init spawns everything:
`block_eintr_oracle`, `futex_handoff_oracle`, `bsshd`, etc.), so every phase
after the kernel-internal `[BOOT_TESTS:PASS]` marker times out waiting for
markers that will now never appear.

```
[TESTS_COMPLETE:109/109]
[BOOT_TESTS:PASS]
[boot] Reset 4 idle thread contexts (CPUs online: 4)
EL0_SYSCALL: First syscall from userspace (SPSR confirms EL0)
[ OK ] syscall path verified
[STAGE:user:ADVANCE]
...
[TESTS_COMPLETE:109/109]
[BOOT_TESTS:PASS]

thread '<unnamed>' panicked at /Users/wrb/fun/code/breenix/rust-fork/library/std/src/io/stdio.rs:1165:9:
failed printing to stdout: Bad address (os error 14)
[syscall] exit(134) pid=1 name=init
```

Reproduced identically twice (`run-aarch64-full-test.sh --boot-tests-only`,
independent boots), both times failing at `Phase 1a: block EINTR oracle
marker absent (30s timeout)` because init never got far enough to spawn it.

## Root cause (traced, not guessed)

`sys_write` -> `copy_from_user(buf_ptr, count)`
(`kernel/src/syscall/handlers.rs:346-367`) -> `is_valid_user_range`
(`kernel/src/memory/layout.rs`), the exact predicate B4 introduced.

`is_valid_user_range`'s stack arm, aarch64:

```rust
#[cfg(target_arch = "aarch64")]
const fn is_valid_user_stack_range(addr: u64, last: u64) -> bool {
    const MAX_STACK_REGION_SIZE: u64 = 1024 * 1024; // 1 MB
    let region_bottom = USER_STACK_REGION_START.saturating_sub(MAX_STACK_REGION_SIZE);
    addr >= region_bottom && addr < USER_STACK_REGION_START
        && last >= region_bottom && last < USER_STACK_REGION_START
}
```

This hardcodes a **1 MiB window** below `USER_STACK_REGION_START`
(`0x0000_FFFF_FF00_0000`) as the only aarch64 addresses ever accepted as
"stack". Compare the x86_64 arm, which accepts the **entire** declared stack
region with no artificial narrowing:

```rust
#[cfg(target_arch = "x86_64")]
const fn is_valid_user_stack_range(addr: u64, last: u64) -> bool {
    addr >= USER_STACK_REGION_START && addr < USER_STACK_REGION_END
        && last >= USER_STACK_REGION_START && last < USER_STACK_REGION_END
}
```

Before B4, `copy_from_user`/`copy_string_from_user`/`copy_to_user` and the
`userptr.rs` root primitives called `userptr::validate_user_buffer`, a
**broad** bound check (`[USER_SPACE_START, USER_SPACE_END)`) that never
consulted this narrow 1 MiB stack window at all -- so a real aarch64 stack
address always passed. B4's redo (this branch, `a6679e7c`) is what newly
routes every one of those call sites through `is_valid_user_range`, which
for the first time makes the pre-existing-but-never-load-bearing 1 MiB
window an actual gate on the syscall hot path -- and it is too narrow for a
real process's stack (`MAX_USER_STACK_SIZE = 2 MiB`, arch-generic, already
twice this window) once demand-paged growth or a per-process stack-slot
offset puts a live address more than 1 MiB below
`USER_STACK_REGION_START`.

## A/B proof: isolates the defect to this branch's own redo, not environment

Built and booted THREE trees, byte-identical userspace ELF/ext2 artifacts
(borrowed from the main checkout's Aug-30 build -- unaffected, since neither
B4 nor M1 touch `libs/libbreenix` or userspace ABI) held constant across all
three:

| tree | `is_valid_user_range` present? | result |
|---|---|---|
| `1e88427e` (pre-#729 entirely; old `is_valid_user_address` per-byte check) | no | **Phase 1a PASS** -- `[BLOCK_EINTR_ORACLE:PASS:...]`, full boot through Phase 5 PASS |
| `a6679e7c` (this branch, current HEAD) | yes (B4's redo) | **FAIL** -- init dies on its first `print!()`, reproduced 2/2 |

Same Mac, same QEMU invocation (`run-aarch64-full-test.sh --boot-tests-only`),
same ext2/ELF artifacts, only the kernel tree differs. This rules out the
borrowed-artifact reconstruction as the cause and isolates the regression to
this branch's `is_valid_user_range` swap.

Full clean-tree (`1e88427e`) transcript: `b4-aarch64-preB4-control-clean-boot.txt`
in this directory (`Phase 1: PASS (109/109 tests)` through `Phase 5: PASS`).

## Disposition

This is genuinely new: neither the review nor fix2-notes.md's own aarch64
verification (a `cargo build` compile check only, never a boot under
`--features boot_tests`/`--boot-tests-only`) exercised this path. The
compile-time proof block added for B4 (see
`b4-aarch64-mutation-falsification.txt`) proves the predicate refuses kernel
addresses and accepts ONE representative code/data-region address on both
arches -- it does not, and structurally cannot, catch a too-narrow *stack*
window, because it never asserts on a stack address at all.

**BLOCKING for aarch64.** The x86_64 leg of B4's own claim (kernel address
refused, heap refused, user address accepted) holds and is separately
confirmed (see `b4-x86-gate-pass-*.txt` and #729/#739 fix2-notes.md's own
beast evidence) -- this finding does not touch that. But `is_valid_user_range`
is the single arch-generic predicate now gating every `copy_from_user`/
`copy_to_user`/`copy_string_from_user` call on BOTH architectures, and on
aarch64 it currently makes the kernel unable to boot to a working shell at
all under the `boot_tests`/`full` feature profile this gate exists to
protect. Filed as a new issue is the fix round's/orchestrator's call; this
document is the evidence.
