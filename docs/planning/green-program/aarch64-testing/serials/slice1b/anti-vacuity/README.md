# Slice 1b anti-vacuity: does retiring the Tier-1 exemption change anything?

Every run below is `cargo test --test ttbr0_shadow_reconciliation_structure --
--nocapture`, from the worktree
`/Users/wrb/fun/code/breenix/.claude/worktrees/wf_c5cad9be-af5-1`, on
2026-09-04/05 ET. What varies between them is two files, and both variations
are stated per run:

* `kernel/src/syscall/time.rs` — either `origin/main`'s (`git show
  origin/main:kernel/src/syscall/time.rs`), a mutated copy of `origin/main`'s,
  or this branch's repaired version;
* `tests/ttbr0_shadow_reconciliation_structure.rs` — either `origin/main`'s,
  which exempts Tier-1 files from three censuses by printing rather than
  pinning, or this branch's, which retires that exemption.

| file | time.rs | census file | exit | result |
|---|---|---|---|---|
| `00-control-origin-main-both-files.txt` | `origin/main` | `origin/main` | 0 | 20 passed, 0 failed |
| `01-pre-fix-main-time-rs-new-censuses.txt` | `origin/main` | this branch | 101 | 17 passed, **2 failed** |
| `02-mutation-sequence-truncated-old-censuses.txt` | `origin/main` + mutation | `origin/main` | 0 | 20 passed, 0 failed |
| `03-mutation-sequence-truncated-new-censuses.txt` | `origin/main` + mutation | this branch | 101 | 16 passed, **3 failed** |
| `04-post-fix-green.txt` | this branch | this branch | 0 | 19 passed, 0 failed |

## 00 — the control

`origin/main`'s tree is green. That is the state the exemption produced: the
site was real, the censuses reached it, and all three declined to pin it.
claim-lint:ok: 20 of 20 tests pass in `00-control-origin-main-both-files.txt`.

## 01 — the pre-fix run the brief asks for

`origin/main`'s `time.rs` under this branch's censuses. Two of the three
censuses go red and both name the site verbatim:

```
thread 'every_ttbr0_install_settles_the_per_cpu_shadows' panicked at tests/ttbr0_shadow_reconciliation_structure.rs:578:5:
these TTBR0 installs leave one or both per-CPU shadows naming another root: ["kernel/src/syscall/time.rs::ensure_current_address_space"]
```

```
thread 'no_ttbr0_installer_claims_it_touches_no_memory' panicked at tests/ttbr0_shadow_reconciliation_structure.rs:979:5:
these TTBR0 installs are declared `nomem`, so the compiler may move the surrounding shadow and page-table stores across the barriers: ["kernel/src/syscall/time.rs::ensure_current_address_space"]
```

The third census — `every_non_primitive_ttbr0_install_performs_the_install_sequence`
— **stays green in this run**, and that is not a defect in the run: `main`'s
`time.rs` install block already ran `dsb ishst` / `msr ttbr0_el1` / `isb` /
`tlbi vmalle1is` / `dsb ish` / `isb` in order, so it was never a member of that
census's out-of-order list. Its Tier-1 filter was inert against this site.
Retiring it there is a structural change that `main`'s own source does not
exercise, so runs 02 and 03 exercise it with a mutation instead.
claim-lint:ok: 2 of 3 censuses fail in `01-pre-fix-main-time-rs-new-censuses.txt`
and the third's out-of-order list is empty there.

## 02 and 03 — the mutation that makes the third census's exemption load-bearing

Mutation, applied to `origin/main`'s `time.rs` only: delete `"tlbi vmalle1is"`,
`"dsb ish"` and the trailing `"isb"` from the install block, leaving
`dsb ishst` / `msr ttbr0_el1` / `isb`.

Under `origin/main`'s censuses (02) the suite is still green — the exemption
swallows it, and all the run says about it is a line on stderr:
claim-lint:ok: 20 of 20 tests pass in
`02-mutation-sequence-truncated-old-censuses.txt`, which carries the line below.

```
TTBR0 installs still out of sequence behind the Tier-1 rule: ["kernel/src/syscall/time.rs::ensure_current_address_space"]
```

Under this branch's censuses (03) the same tree fails, naming the same site:

```
these TTBR0 installs do not run ["dsb ishst", "msr ttbr0_el1", "isb", "tlbi vmalle1is", "dsb ish", "isb"] in order, so a stale translation can survive the install or the root can be taken before it is visible: ["kernel/src/syscall/time.rs::ensure_current_address_space"]
```

That pair is what shows the third census's retirement is not decorative.

## 04 — at this branch's head

Green, 19 tests. The suite lost one test relative to `origin/main`'s 20:
`the_tier_one_exemption_matches_the_project_rule`, which existed to check the
exemption list against CLAUDE.md's prohibited-sections table. With no exemption
list left it had nothing to validate, so it was removed rather than left to
assert about a constant nothing reads.
claim-lint:ok: 19 of 19 tests pass in `04-post-fix-green.txt`, one fewer than the
20 in `00-control-origin-main-both-files.txt`.

The census the run prints at this head, 6 functions, zero unreconciled:
claim-lint:ok: 6 of 6 censused functions are classified in `04-post-fix-green.txt`
and 0 of 6 as unreconciled.

```
TTBR0 install census (6 functions): [
    "kernel/src/arch_impl/aarch64/context_switch.rs::switch_ttbr0_if_needed (reconciles inline)",
    "kernel/src/arch_impl/aarch64/paging.rs::write_root (parameter-borne)",
    "kernel/src/arch_impl/aarch64/syscall_entry.rs::restore_ttbr0_after_failed_exec (reconciles inline)",
    "kernel/src/arch_impl/aarch64/ttbr0.rs::switch_ttbr0_to_kernel (the discipline)",
    "kernel/src/arch_impl/aarch64/ttbr0.rs::adopt_process_ttbr0 (the discipline)",
    "kernel/src/memory/arch_stub.rs::write (parameter-borne)",
]
```

## What these runs do not show

They are file-shape checks. Nothing here observes a boot, a return to EL0, or a
TTBR0 register value on hardware or in QEMU; the boots that ran at this head are
recorded separately, starting with `../prove/strict-1.txt`.
claim-lint:ok: 0 of 5 runs in this directory launches QEMU; the 6 that do are
under `docs/planning/green-program/aarch64-testing/serials/slice1b/prove/`.
