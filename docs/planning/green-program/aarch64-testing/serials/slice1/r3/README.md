# Round 3 (R158) anti-vacuity mutations — TTBR0 shadow reconciliation

Three ratchet gaps were closed this round. Each closure carries one recorded
mutation: a scratch edit to a kernel source, one run of
`cargo test --test ttbr0_shadow_reconciliation_structure`, the verbatim
assertion text the run produced, a byte-copy restore, and a green re-run.
The byte copies were taken before each edit and the restore was verified by
SHA-256 rather than by eye.

Every mutation below was applied to a working tree that was otherwise clean,
and the tree was clean again before the next one.
claim-lint:ok: 3 of 3 mutations reddened, 3 of 3 restores matched the
pre-edit SHA-256, and the green re-run is `restored-green.txt` (exit 0,
20 passed).

| file | what it records |
|---|---|
| `mutation-n003-sequence-stripped.txt` | N-003, exit 101 |
| `mutation-n004-next-cr3-rearmed.txt` | N-004, exit 101 |
| `mutation-n005-quiesce-shadows-dropped.txt` | N-005, exit 101 |
| `restored-green.txt` | the suite at the restored tree, exit 0, 20 of 20 passed |

## N-003 — the install sequence outside the discipline module

Scratch edit, `kernel/src/arch_impl/aarch64/context_switch.rs`, inside
`switch_ttbr0_if_needed`: the three lines `"tlbi vmalle1is",`, `"dsb ish",` and
the `"isb",` that follows them were deleted from the install `asm!` block,
leaving `dsb ishst` / `msr ttbr0_el1` / `isb`.

```
$ cargo test --test ttbr0_shadow_reconciliation_structure
exit: 101
test result: FAILED. 19 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out

every_non_primitive_ttbr0_install_performs_the_install_sequence panicked:
these TTBR0 installs do not run ["dsb ishst", "msr ttbr0_el1", "isb", "tlbi vmalle1is", "dsb ish", "isb"] in order, so a stale translation can survive the install or the root can be taken before it is visible: ["kernel/src/arch_impl/aarch64/context_switch.rs::switch_ttbr0_if_needed"]
```

Before and after the edit the file hashed
`ae6e020a6e332d57a0ce27b3b5cf9974d98726e2d2d04212981749ed59dfb582`.

Under round 2's ratchets this mutation was invisible: the sequence check ran
over `kernel/src/arch_impl/aarch64/ttbr0.rs` only, and this install is not in
that file.

## N-004 — the LAST write to each shadow word

Scratch edit, same file, same function: `Aarch64PerCpu::set_next_cr3(next_ttbr0);`
was appended immediately after the existing `Aarch64PerCpu::set_next_cr3(0);`,
so the body still clears the word and then arms it again with the root it just
installed.

```
$ cargo test --test ttbr0_shadow_reconciliation_structure
exit: 101
test result: FAILED. 18 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out

the_dispatch_ttbr0_switch_settles_both_shadows panicked:
the dispatch switch must also retire the pending switch it consumed: a `next_cr3` left armed is installed FIRST on the next return to EL0

every_ttbr0_install_settles_the_per_cpu_shadows panicked:
these TTBR0 installs leave one or both per-CPU shadows naming another root: ["kernel/src/arch_impl/aarch64/context_switch.rs::switch_ttbr0_if_needed"]
```

Before and after the edit the file hashed
`ae6e020a6e332d57a0ce27b3b5cf9974d98726e2d2d04212981749ed59dfb582`.

Under round 2's `settles_both_shadows`, which read the FIRST occurrence of each
accessor, this mutation passed.

## N-005 — the callers of the kernel-root install

Scratch edit, `kernel/src/arch_impl/aarch64/ttbr0.rs`: the
`set_saved_process_cr3(0)` / `set_next_cr3(0)` pair was deleted from
`quiesce_ttbr0_for_exit`, leaving it as a bare call to
`switch_ttbr0_to_kernel()`.

```
$ cargo test --test ttbr0_shadow_reconciliation_structure
exit: 101
test result: FAILED. 19 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out

every_caller_of_the_kernel_root_install_settles_the_shadows panicked:
these aarch64 callers install the kernel root and leave the per-CPU TTBR0 shadows naming another one, so the next return to EL0 may reinstall a root this CPU has just left: ["kernel/src/arch_impl/aarch64/ttbr0.rs::quiesce_ttbr0_for_exit"]
```

Before and after the edit the file hashed
`bb259857e44bbdd77ef5365dbf7011b4bb5dd080129d3b4a726b9046814ff3e4`.

Under round 2's ratchets no check scored the callers of
`switch_ttbr0_to_kernel`: the primitive-caller census skips the discipline
module, and `switch_ttbr0_to_kernel` is not a mechanism primitive by that
census's definition, so it was not one of the calls that census looks for.
claim-lint:ok: 0 of the 3 callers listed above were scored by any round-2
check; the round-2 suite is `serials/slice1/r2/structure-suites.txt`.

## The census each new check reaches

Observed by temporarily raising each coverage floor so the assertion printed
its own list, then restoring the byte copy. Recorded here because a census that
silently shrinks is the way a ratchet goes quiet.

`every_non_primitive_ttbr0_install_performs_the_install_sequence`, 5 sites:

```
kernel/src/arch_impl/aarch64/context_switch.rs::switch_ttbr0_if_needed
kernel/src/arch_impl/aarch64/syscall_entry.rs::restore_ttbr0_after_failed_exec
kernel/src/arch_impl/aarch64/ttbr0.rs::switch_ttbr0_to_kernel
kernel/src/arch_impl/aarch64/ttbr0.rs::adopt_process_ttbr0
kernel/src/syscall/time.rs::ensure_current_address_space
```

`every_caller_of_the_kernel_root_install_settles_the_shadows`, 3 callers:

```
kernel/src/arch_impl/aarch64/syscall_entry.rs::sys_exit_aarch64
kernel/src/arch_impl/aarch64/syscall_entry.rs::sys_exec_aarch64
kernel/src/arch_impl/aarch64/ttbr0.rs::quiesce_ttbr0_for_exit
```

2 of those 3 -- `sys_exit_aarch64` and `quiesce_ttbr0_for_exit` -- clear both
shadow words to 0 themselves; the remaining 1, `sys_exec_aarch64`, is the exec
shape, whose interrupt-masked window is pinned by
`validate_aarch64_failed_exec_ttbr0_rollback` in
`tests/context_restore_structure.rs` and
`validate_sys_exec_releases_process_manager` in
`tests/exec_lock_order_structure.rs`.
