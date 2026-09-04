# TTBR0 shadow reconciliation — the #786 latent-defect slice, ported to main

`fix/562-761-aarch64-testing-profile` is parked. One of the defects its round-7
RCA found is not specific to that branch's boot sequence: it is a shape present
on `main` at every aarch64 process-root install, where the write to
`TTBR0_EL1` and the two per-CPU words the syscall return corridor reads
disagree about which page-table root a return to EL0 should run on. This slice
ports that repair, and only that repair, onto `main`.
claim-lint:ok: 10 such installs are censused under `kernel/src` at this head;
9 of 10 are routed through the new discipline and 1 of 10 is left behind the
Tier-1 rule. The list is in section 7.

Branch head the port was taken from: `2b3fb187`. Base: `origin/main`
`d6b7a186`.

---

## 1. The mechanism

Two per-CPU words decide which root a return to EL0 runs on, and both are read
by `kernel/src/arch_impl/aarch64/syscall_entry.S`:

| word | per-CPU offset | corridor behaviour |
|---|---|---|
| `next_cr3` | 64 | read first; a value other than 0 is installed and then cleared |
| `saved_process_cr3` | 80 | the fallback arm `.Lrestore_saved_ttbr`, restored when `next_cr3` is clear |

`setup_idle_return_locked` in `kernel/src/arch_impl/aarch64/context_switch.rs`
publishes the KERNEL root into `next_cr3` on every idle dispatch. Nothing on
the idle return corridor consumes it — `boot.S` never reads per-CPU offset 64 —
so once published it stays armed until some later return corridor applies it.
claim-lint:ok: measured on `main` in section 3 -- `next_cr3` reads `0x0` at
init's ERET in 5 of 5 probed boots, and on the parked branch it read the kernel
root in 16 of the 20 probed baseline boots cited in section 4.

A site that installs a process root with a raw `msr ttbr0_el1` and touches
neither word is therefore not merely out of sync with the register: it leaves
the next return to EL0 free to install whatever root the shadows still name.
On the parked branch that is exactly what happened to init, whose first `svc`
returned onto the kernel root and took an instruction abort at its own return
address.

## 2. What was ported

`kernel/src/arch_impl/aarch64/ttbr0.rs` gains `adopt_process_ttbr0(value)`: the
`dsb ishst` / `msr ttbr0_el1` / `isb` / `tlbi vmalle1is` / `dsb ish` / `isb`
sequence, then `set_saved_process_cr3(value)` and `set_next_cr3(0)`. Clearing
`next_cr3` is part of the install: after the call the architectural register is
the decision, so a pending "switch to some other root on the way out" request
is either the same root or a stale one, and applying either on the return path
is wrong.

Nine process-root installs are routed through it:

| site | what it was on `main` |
|---|---|
| `main_aarch64.rs::launch_init_from_elf` | raw `msr`, neither shadow touched |
| `arch_impl/aarch64/syscall_entry.rs::sys_exec_aarch64` | raw `msr` + `set_saved_process_cr3`, `next_cr3` left armed |
| `arch_impl/aarch64/syscall_entry.rs::check_and_deliver_signals_aarch64` | raw `msr`, neither shadow touched |
| `arch_impl/aarch64/context_switch.rs::check_and_deliver_signals_for_current_thread_arm64` | raw `msr`, neither shadow touched |
| `syscall/wait.rs::ensure_current_address_space` | raw `msr`, neither shadow touched |
| `syscall/futex.rs::ensure_current_address_space` | raw `msr`, neither shadow touched |
| `syscall/graphics.rs::ensure_current_address_space` | raw `msr`, neither shadow touched, **and no TLB invalidation at all** — only `dsb ishst` / `msr` / `isb` |
| `syscall/handlers.rs::poll_ensure_address_space` | raw `msr`, neither shadow touched |
| `memory/process_memory.rs::switch_to_process_page_table` (aarch64 arm) | `Cr3::write`, both shadows left naming another root |

The `graphics.rs` missing-TLBI repair and the `process_memory.rs` repair are
behaviour changes on `main` in their own right, independent of whether the
`next_cr3` trigger is reachable.

`Cr3Flags::bits()` was added in `kernel/src/memory/arch_stub.rs` so
`process_memory.rs` can hand the complete value (root plus ASID) to the
discipline rather than write the register itself.

### Ported commits

| branch commit | what came across |
|---|---|
| `5bd91b81` | the helper and the first eight sites (cherry-picked; applied with no conflicts) |
| `5f0a5481` | `Cr3Flags::bits()`, the ninth site, and the TTBR0 census + caller census |
| `35dc2f15` | the `tests/exec_lock_order_structure.rs` T4 rework and its two new negative tests |

`35dc2f15` was not named in the slice brief but is not optional: `main`'s T4
validator asserts `sys_exec_aarch64` contains exactly one `msr ttbr0_el1` and
one `set_saved_process_cr3(`, both of which the port removes from that
function. Without it the existing suite goes red on a correct change.

### What was deliberately left behind

* Everything else on the parked branch: the #562 ksoftirqd pin (`4f6988cf`),
  the #761 loader continuation and the boot-sequence change built on it
  (`5f21d2f3`, `ead21609`, `ea552f9d`), the idle-identity refusal work
  (`264294f5`, `023e049d` and the rest), the musl catalog commits, the x86
  affinity and staging build commits, and the branch's own RCA document.
* `kernel/src/syscall/time.rs::ensure_current_address_space`, the tenth site
  and the same defect shape. `syscall/time.rs` is on CLAUDE.md's Tier-1
  prohibited list; changing it needs explicit operator approval, which this
  slice does not have. It is disclosed rather than hidden — see section 6.
* The branch's `tests/aarch64_testing_profile_structure.rs` as a file. Its
  #562/#761 tests pin functions that do not exist on `main` (for example
  `kernel/src/task/idle_sleep.rs`, which `main` does not have), so the TTBR0
  census was lifted into a new file,
  `tests/ttbr0_shadow_reconciliation_structure.rs`, and its one non-TTBR0
  neighbour — `an_unreadable_identity_is_not_a_refusal` — was left behind with
  the rest of that work.
* `docker/qemu/run-aarch64-testing-profile-boot-test.sh`, the branch's
  testing-profile gate script, and the two structure tests that score it.

`launch_init_from_elf` came across because it is the same shape and the hunk
applied cleanly; nothing kboot-related or boot-sequence-related rode with it.
`main` still launches init from the pinned boot continuation, and the diff to
that function is one raw `asm!` block replaced by one call.
claim-lint:ok: the whole diff to that function is 1 hunk, reproducible with
`git diff origin/main -- kernel/src/main_aarch64.rs`.

## 3. Why the trigger is latent on `main`, and how that was measured

On `main` the boot CPU is preemption-pinned for the whole boot, so it takes no
idle dispatch before `launch_init_from_elf`, and the word that decides init's
first syscall return is never armed. That is an assertion about the code, so it
was measured rather than argued: the probe below reads `next_cr3=0x0` in 5 of
5 boots.

A scratch kernel was built with two changes, neither committed and both kept in
`serials/slice1/diffs/mutation-and-probe.diff`:

1. **The mutation.** `set_next_cr3(0)` deleted from `adopt_process_ttbr0`,
   which restores `main`'s pre-port disposition of the trigger word at all nine
   routed sites.
2. **The probe.** `launch_init_from_elf` prints both shadow words either side
   of the install, immediately before the ERET that puts init in EL0.

Five strict-gate boots of that kernel
(`docker/qemu/run-aarch64-boot-test-strict.sh 5`, serials in
`serials/slice1/mutation-5/`):

```
Total iterations: 5
Successes: 5
Failures: 0
Success rate: 100%
```

and the probe line, identical in all 5 of 5 serials:

```
[SLICE1_PRE_ERET] pre_next_cr3=0x0 pre_saved=0x0 post_next_cr3=0x0 post_saved=0x100004406c000 ttbr0=0x100004406c000
```

`INSTRUCTION_ABORT` count across those five serials: 0 of 5.

**So the answer is no: the latent defect is not observable on `main` today.**
`next_cr3` reads `0x0` at init's ERET in 5 of 5 boots — the trigger word is not
armed, so removing the clear changes nothing that a boot can see. The 5 of 5
green is not a coincidence to be explained away; it is what the probe predicts.

This slice is therefore **defensive** on the `next_cr3` arm, plus two repairs
that are not conditional on the trigger (the `graphics.rs` missing TLBI and the
`process_memory.rs` shadow reconciliation), plus the ratchets. Its proof of
consequence is the parked branch's own A/B/A mutation battery in section 4 --
13 of 26, then 0 of 24, then 4 of 8 on reversion -- not a red on `main`.

Disclosed narrowing: the mutated build is not byte-for-byte `main`'s behaviour.
`adopt_process_ttbr0` still publishes `saved_process_cr3` (which the syscall
entry stub overwrites from the live register on the next `svc` anyway), the
`graphics.rs` site still gains its TLBI, and `process_memory.rs` still goes
through the discipline. What the mutation isolates is the `next_cr3` arm — the
one the parked branch measured firing.

## 4. The branch A/B/A, cited by committed path

The consequence claim rests on batteries run on the parked branch and committed
there, not on anything re-run here. At branch commit `1245c64b`, under
`docs/planning/green-program/aarch64-testing/serials/r7/aba/`, with
`CLASSIFICATION.tsv` generated by grepping each committed serial:

| battery | boots | `INSTRUCTION_ABORT] FAR` |
|---|---|---|
| `baseline-26/` — branch head, no mutation | 26 | 13 |
| `mutation-a-6/` + `mutation-ab-18/` — the two shadow stores added | 24 | 0 |
| `reverted-8/` — both mutations removed again | 8 | 4 |

13 of 26, then 0 of 24, then 4 of 8. The branch also recorded the mechanism
directly: of the 20 baseline boots carrying the pre-ERET probe, 16 read
`next_cr3=0x40200000` — the kernel root, still armed — while 24 of 24 boots
with the stores in place read `next_cr3=0x0`.

These rows are the parked branch's arithmetic over its own committed serials.
They are cited here by commit and path; they were not reproduced on `main`,
because section 3 shows `main` cannot reproduce them.

## 5. Smoke at this head (R13)

Every boot below was run on its own on this Mac, 18 boots in total across two
batteries, never two at once.
claim-lint:ok: 18 of 18 boots serialised; the serials are the three
`serials/slice1/smoke-*` directories and `serials/slice1/r13-final/`.

The smoke was run twice: once at the code commit `9e9131d0`
(`serials/slice1/smoke-*`), and again at the final head after the ratchets,
serials and this document had landed (`serials/slice1/r13-final/`). The kernel
sources are byte-identical between them — every commit after `9e9131d0` touches
only `tests/`, `docs/` and this file — so the second battery is a re-run at the
returned head rather than a different kernel.
claim-lint:ok: 0 of the 4 commits after `9e9131d0` touch `kernel/`,
reproducible with `git diff --stat 9e9131d0..HEAD -- kernel`.
Serials: `serials/slice1/r13-final/strict-boot1-serial.txt` and its 8 siblings.

| profile | command | boots per battery | result, both batteries |
|---|---|---|---|
| strict | `docker/qemu/run-aarch64-boot-test-strict.sh 1`, three times | 3 | 3 PASS, 0 FAIL each time |
| production | `docker/qemu/run-aarch64-prod-profile-boot-test.sh`, three times | 3 | 3 PASS, 0 FAIL each time |
| testing | direct QEMU boot, same invocation the strict gate uses | 3 | 3 FAIL each time — see below |

The `testing` profile **does build** on `main` (`--features testing`, soft-float
target, `scripts/check-kernel-no-neon.sh` PASS). It does not boot. Two failure
signatures appear across the two batteries, in different proportions: a panic at
`kernel/src/task/softirq_tests.rs:228:5` — "ksoftirqd should have processed
deferred softirqs (tid=Some(2))" — and a boot that makes no progress past
`[smp] 4 CPUs online`, reaching neither a shell prompt nor a panic before the
20 s timeout. The first battery scored 2 panics and 1 no-progress; the second
scored 3 panics and 0 no-progress.

That is #562, and it is not this slice's doing. The same kernel built at
`origin/main` `d6b7a186` with the slice stashed out, booted 3 times with the
same command, produced the same two signatures: 2 panics at
`softirq_tests.rs:228:5` and 1 no-progress boot. Control serials:
`serials/slice1/main-control-testing/`. 3 of 3 red in each battery at this
branch's head, 3 of 3 red on the base commit, the same two signatures — the
profile's red is inherited, not introduced.

## 6. The Tier-1 site, disclosed

`kernel/src/syscall/time.rs::ensure_current_address_space` installs a process
root with a raw `msr` and reconciles neither shadow. It is the same defect
shape as the nine sites this slice repaired, and it is **not repaired here**,
because `syscall/time.rs` is Tier-1 prohibited.

It is printed, not hidden. `every_ttbr0_install_settles_the_per_cpu_shadows`
emits this on each run, 1 of the 7 censused functions:

```
TTBR0 installs still unreconciled behind the Tier-1 rule: ["kernel/src/syscall/time.rs::ensure_current_address_space"]
```

Reproduce with `cargo test --test ttbr0_shadow_reconciliation_structure
every_ttbr0_install -- --nocapture`.

If the list ever empties because someone repaired the site, the test still
passes. If it empties because the census stopped reaching the site, the
coverage floor in the same test is what notices.

## 7. Ratchets

`tests/ttbr0_shadow_reconciliation_structure.rs` walks every Rust function
under `kernel/src` whose body writes `TTBR0_EL1` and sorts the result by shape
rather than by a list of known sites. A censused function must be the
discipline itself, or reconcile inline (it names `set_saved_process_cr3`), or
be a mechanism primitive — one whose installed value came in through its own
signature and which fetched nothing and named no shadow. Anything left over
must live in a file CLAUDE.md lists as Tier-1 prohibited, and
`the_tier_one_exemption_matches_the_project_rule` reads that list back out of
CLAUDE.md rather than trusting the test's own copy.
claim-lint:ok: 7 of 7 censused functions are listed immediately below and
reproduced by the `--nocapture` run cited in section 6.

The census reaches 7 functions at this head, the same 7 the parked branch
reported:

```
kernel/src/arch_impl/aarch64/context_switch.rs::switch_ttbr0_if_needed        (reconciles inline)
kernel/src/arch_impl/aarch64/paging.rs::write_root                            (parameter-borne)
kernel/src/arch_impl/aarch64/syscall_entry.rs::restore_ttbr0_after_failed_exec (both)
kernel/src/arch_impl/aarch64/ttbr0.rs::switch_ttbr0_to_kernel                 (the discipline)
kernel/src/arch_impl/aarch64/ttbr0.rs::adopt_process_ttbr0                    (the discipline)
kernel/src/memory/arch_stub.rs::write                                         (parameter-borne)
kernel/src/syscall/time.rs::ensure_current_address_space                      (Tier-1, unreconciled)
```

The nine routed sites are absent from that list precisely because they no
longer write the register themselves.

`every_aarch64_caller_of_a_mechanism_primitive_settles_the_shadows` closes the
exemption's other end: an aarch64-scoped caller of an exempt primitive must
name `adopt_process_ttbr0`, `quiesce_ttbr0_for_exit` or
`set_saved_process_cr3`, or be the MMU bring-up that installs the kernel root
before per-CPU state exists. That census is what found the
`process_memory.rs` site.

Anti-vacuity, 5 legs in that same file: a synthetic site that reads a root out of
the process manager and installs it raw is classified unreconciled; a synthetic
site whose operand is a masked parameter is still admitted as a primitive; a
synthetic site that fetches its root is rejected as a primitive; a synthetic
site that touches one shadow is rejected as a primitive; and a synthetic
aarch64 wrapper handing a process root straight to a primitive is caught by the
caller census and cleared once routed through the discipline.

`tests/exec_lock_order_structure.rs` gains two negative tests of its own:
deleting `set_next_cr3(0)` from the helper reddens the exec-path T4 validator,
and putting a raw `msr` back into `sys_exec_aarch64` reddens it too.

## 8. Builds and suites at this head

| what | result |
|---|---|
| aarch64 `--features boot_tests`, `aarch64-breenix-kernel.json`, soft-float | builds; `scripts/check-kernel-no-neon.sh` PASS, 0 FP/SIMD load/store in `.text` |
| aarch64 no features (the production profile, built by the prod gate itself) | builds; `check-kernel-no-neon.sh` PASS |
| aarch64 `--features testing` | builds; `check-kernel-no-neon.sh` PASS (it is the boot that fails, section 5) |
| x86_64 `--features testing,external_test_bins --bin qemu-uefi`, on beast | builds, exit 0, 0 lines matching `^(warning\|error)` |
| the 27 `tests/*_structure.rs` suites | 27 of 27 suites green, 0 failures |
| the 9-boot R13 smoke, re-run at the final head | `serials/slice1/r13-final/` |

0 of the 3 aarch64 builds emit a warning attributable to this tree. The one
warning `cargo` prints on each `-Z build-std` invocation here — "the following
packages contain code that will be rejected by a future version of Rust: core
v0.0.0" — is about the pinned toolchain's own `core`, is present identically at
`origin/main`, and is not this branch's.
claim-lint:ok: 3 of 3 aarch64 `-Z build-std` profiles built here emit it, as
does the same command at `origin/main`.

## 9. The x86_64 build

x86 builds run on beast, not on this Mac. A fresh clone of this branch at
`9d847b95` was made at `/root/breenix-ttbr0` inside the `breenix-x86` Incus
container and built with the standard command:

```
cargo build --release --features testing,external_test_bins --bin qemu-uefi
```

Result, from `serials/slice1/builds/x86-testing-external_test_bins-build.txt`:

```
    Finished `release` profile [optimized] target(s) in 3m 10s
```

`BUILD_EXIT=0`, and `grep -nE '^(warning|error)'` over that log returns 0 of
123 lines. No boot was run on x86; this slice touches no x86 code path, and the
x86 gate reds tracked at #630, #636, #554, #608 and #540 are unrelated to it.

## 10. Build and boot logs, by path

| file | what it is |
|---|---|
| `serials/slice1/builds/aarch64-boot_tests-build.txt` | the `--features boot_tests` build at this head, exit 0 |
| `serials/slice1/builds/aarch64-boot_tests-no-neon.txt` | its `check-kernel-no-neon.sh` PASS |
| `serials/slice1/builds/aarch64-no-features-build.txt` | the production-profile build at this head, exit 0 |
| `serials/slice1/builds/aarch64-no-features-no-neon.txt` | its `check-kernel-no-neon.sh` PASS |
| `serials/slice1/builds/aarch64-testing-build.txt` | the `--features testing` build at this head, exit 0 |
| `serials/slice1/builds/aarch64-testing-no-neon.txt` | its `check-kernel-no-neon.sh` PASS |
| `serials/slice1/builds/x86-testing-external_test_bins-build.txt` | the beast x86 build, exit 0 |

claim-lint: scripts/claim-lint.py --files docs/planning/green-program/aarch64-testing/TTBR0-SHADOW-SLICE-2026-09-04.md -> exit 0
