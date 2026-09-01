# #721 closure round -- re-prove at merged bytes

Branch `fix/721-x86-exec` @ `9e5f47f5` (B-2 merge-forward of `origin/main`
`9bfcf0b5` onto prove2's `56132364`, plus B-1's README correction).

## B-2 -- merge-forward

`git merge origin/main` from `56132364` produced a clean auto-merge (only
`tests/teardown_structure.rs` needed auto-merging, no conflict markers, no
manual resolution). Merge commit `01cbbf55`. `kernel/src/memory/layout.rs`'s
`is_valid_user_range` verified byte-identical before/after the merge (diff
empty) -- confirms the reviewer's trial-merge finding and falsifies
fix2-notes.md's m8 conflict-risk rationale for staying behind.

## RE-PROVE: full `tests/*structure*.rs` family, enumerated from disk

21 files (20 pre-merge + `mmap_floor_structure.rs`, added by the merge).
Ran individually via `cargo test --release --test <name>`:

```
block_request_lifetime_structure           12 passed
context_restore_structure                  97 passed  (incl. clone_publication_lifecycle_is_closed
                                                         and clone_publication_lifecycle_guard_census_
                                                         reddens_when_either_arch_guard_removed)
coreproof_component_h_structure             5 passed
coreproof_coverage_structure                4 passed
coreproof_mutation_register_structure       5 passed
coreproof_sites_structure                   4 passed
degenerate_transfer_fd_validation_structure 4 passed
dma_and_log_sink_structure                  4 passed
exec_lock_order_structure                  42 passed
exit_tally_structure                        6 passed
loopback_pump_structure                    63 passed
masked_binary_load_structure                4 passed
mmap_floor_structure                        9 passed  (new, from main's #742/#744)
net_lock_structure                         19 passed
preempt_bracket_structure                   8 passed
serial_line_atomicity_structure             9 passed
signal_eintr_predicate_structure            2 passed
strand_handoff_structure                   38 passed
syscall_return_register_structure           6 passed
teardown_structure                         81 passed  (was 79/81 pre-merge -- main's
                                                        32a8171f driver_h census fix
                                                        confirmed to close both)
tty_oracle_structure                       16 passed
-----
TOTAL                                     438 passed, 0 failed
```

**81/81 on `teardown_structure`, 0 failures anywhere.** Confirms the
reviewer's trial-merge claim exactly: main already fixed the two
`teardown_structure` failures (`v3_structural_closures_are_exact`,
`deliberately_broken_variants_fail_the_ratchet`) that fix2-notes.md/
fix2-prove.md both carried as "pre-existing, unrelated" at 79/81.

## Zero-warning builds (x86, at merged bytes)

- `cargo build --release --bin qemu-uefi` (zero-feature): 0 warnings, 0 errors.
- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`: 0 warnings, 0 errors.

## x86 production-profile gate on beast (post-merge)

`breenix-x86` Incus container, synced to `9e5f47f5`, fresh
`userspace/programs/build.sh --arch x86_64` rebuild (145 binaries), then
`docker/qemu/run-x86-prod-profile-boot-test.sh`.

**GATE EXIT=0, PASS.** Full log: `prod-gate-post-merge-9e5f47f5.log`. Every
`#721` marker confirmed post-merge:

```
exec smoke launch (#721):      1
exec smoke target enter argc=2 (#721): 1
exec smoke target ok (#721):   1
exec smoke launcher exit code=0 (#721): 1
exec smoke spawn failed (must be absent, #721): 0
exec smoke exec failed (must be absent, #721): 0
exec smoke target argv fail (must be absent, #721): 0
exec lock order first commit (#721 K7): 1
exec lock order PM-held violation (must be absent, #721 K7): 0
exec lock order unpinned violation (must be absent, #721 K7): 0
exec lock order no-sched-thread violation (must be absent, #721 K7): 0
```

`FIRST_COMMIT=1` and all three violation counters `=0`, exactly as required.

## Disposition

Both B-2 (merge-forward) and the RE-PROVE requirement are satisfied at the
merged bytes. No red anywhere. Proceeding to land.
