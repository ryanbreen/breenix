# TTBR0 shadow reconciliation — the #786 latent-defect slice, ported to main

`fix/562-761-aarch64-testing-profile` is parked. One of the defects its round-7
RCA found is not specific to that branch's boot sequence: it is a shape present
on `main` at every aarch64 process-root install, where the write to
`TTBR0_EL1` and the two per-CPU words the syscall return corridor reads
disagree about which page-table root a return to EL0 should run on. This slice
ports that repair, and only that repair, onto `main`.
claim-lint:ok: 10 of 10 process-root install decision sites on `main` carried
that shape; 9 of 10 are listed in section 2 and the 10th in section 6.

Two counts run through this document and they are not the same number, so both
are stated here and neither is left to be inferred:

* **10 process-root install DECISION sites** existed on `main` — the places
  that chose a root and wrote it to `TTBR0_EL1`. 9 of 10 are routed through the
  new discipline by this slice; 1 of 10, the Tier-1 site
  `kernel/src/syscall/time.rs::ensure_current_address_space`, is left behind the
  Tier-1 rule. The 9 are listed in section 2 and the 1 in section 6.
* **7 FUNCTIONS still write `TTBR0_EL1` with a raw `msr`** at this head, and
  those 7 are what the ratchet census in section 7 walks: 2 discipline-module
  helpers, 2 that reconcile both shadows inline, 2 mechanism primitives that
  install what a caller decided, and the same 1 Tier-1 site.

The 9 routed sites are absent from the 7 precisely because they no longer write
the register themselves. Wherever this document says "the tenth site" it means
the 10th of the 10 decision sites, which is also 1 of the 7 censused functions.
claim-lint:ok: 9 of 10 decision sites routed and 7 of 7 functions censused at
this head; the decision sites are listed in section 2 and the censused
functions in section 7.

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
* `kernel/src/syscall/time.rs::ensure_current_address_space` — the tenth of the
  10 decision sites the header describes, which is also 1 of the 7 functions
  the section-7 census reaches, and the same defect shape. `syscall/time.rs` is
  on CLAUDE.md's Tier-1 prohibited list; changing it needs explicit operator
  approval, which this slice does not have. It is disclosed rather than hidden
  — see section 6.
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
`main` still launches init from the pinned boot continuation. The diff to
`kernel/src/main_aarch64.rs` is two hunks, not one: the raw `asm!` block
replaced by one call, and the removal of the `use core::arch::asm;` import that
block was the file's last user of. The behaviour-bearing change is the first
hunk; the second is what the first makes dead.
claim-lint:ok: `git diff --unified=0 origin/main..HEAD --
kernel/src/main_aarch64.rs | rg "^@@"` prints 2 hunk headers, `@@ -90 +89,0 @@`
and `@@ -271,12 +270,26 @@`.

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

## 5. Boots at this head (round 3)

The boots that speak for the head this document is committed at are round 3's.
They were run against the kernel and test sources of commit `98f1bf7a`, which
are the same bytes commit `0f2621b0` carries -- that later commit adds only the
round's artifacts.
claim-lint:ok: `git diff --stat 98f1bf7a..0f2621b0 -- kernel tests` reports 0
changed files, and 10 of the 10 files that commit does change are under
`docs/`.

Each boot was run on its own on this Mac, one at a time, with
`pgrep -fl qemu-system-aarch64 | wc -l` recorded immediately before each launch
and the launch made only when that count was 2 or lower. It read 0 before each
of the 6, and each gate transcript carries its own reading on its first line.
claim-lint:ok: 6 of 6 transcripts open with `pgrep-at-launch: 0`; they are the
6 `*-gate.txt` files under `serials/slice1/prove-r3/`.

| profile | command | boots | result |
|---|---|---|---|
| strict | `docker/qemu/run-aarch64-boot-test-strict.sh 1`, three times | 3 | 3 PASS, 0 FAIL |
| production | `docker/qemu/run-aarch64-prod-profile-boot-test.sh`, three times | 3 | 2 PASS, 1 FAIL — below |

### The production red

Boot 1 of the production battery failed. The whole serial is preserved at
`serials/slice1/prove-r3/prod-boot1-FAIL-serial.txt` and the gate transcript
beside it at `prod-boot1-FAIL-gate.txt`; the 5 green boots' transcripts are in
the same directory.

It is a stall, not a fault. `clonevm_exec_test` prints
`CLONEVM_EXEC_TEST: second stage` and never reaches
`post-exec rendezvous complete`; `init` stays blocked in `waitpid` on it, so
`bsshd` is never spawned and the gate ends with "bsshd never reached its
listening state". The guest is alive throughout: heartbeats continue to
119415 ms, `[net-rx-counters]` samples keep advancing to sample 10, and the
custody and tombstone censuses keep printing. The gate counted 0 crash markers.
claim-lint:ok: 1 of 3 production boots, read off the whole serial at
`serials/slice1/prove-r3/prod-boot1-FAIL-serial.txt`.

That is the exact signature of open issue **#690** — "clonevm_exec_test hangs
after 'second stage': post-exec rendezvous never completes" — which that issue
reports at 1 boot in 30 on the `cortex-a72` service-sequence profile. This gate
runs `-cpu max`, so the same signature is being seen on a second profile.
claim-lint:ok: the signature is matched against issue #690, whose own serial is
`docs/planning/green-program/sockets/serials/aarch64-clonevm-second-stage-stall-20260829.txt`.

**It is recorded as UNATTRIBUTED and this round does not call the branch
landable.** #690 is not one of the pre-adjudicated signatures this round was
given (#555 softirq, #536 timer_delay, #576 EL1 NULL-PC, #586 starved
wake-loss, and a host-load window miss, which requires a recorded pre-launch
count above 2 and this had 0). Round 3 ran no control at `origin/main` on this
profile, so no measurement here rules the branch out either — and the branch
does change the exec-path TTBR0 install that `clonevm_exec_test` exercises.
Naming #690 identifies the signature; it does not discharge the red.
claim-lint:ok: 0 of the 5 pre-adjudicated signatures listed above matches this
red; the issue it does match is #690.

### The `testing` profile

Rounds 1 and 2 measured the `--features testing` profile: it builds (round 3
rebuilt it, `BUILD_EXIT=0`, `check-kernel-no-neon.sh` PASS — section 8) and it
does not boot. Round 1 recorded two signatures across two 3-boot batteries — a
panic at `kernel/src/task/softirq_tests.rs:228:5`, "ksoftirqd should have
processed deferred softirqs (tid=Some(2))", and a boot making no progress past
`[smp] 4 CPUs online` — and a 3-boot control at `origin/main` `d6b7a186` with
the slice stashed out that produced the same two signatures, 2 panics and 1
no-progress. Control serials: `serials/slice1/main-control-testing/`. That is
#562 and it is inherited, not introduced. Round 3 did not boot this profile.

### The earlier batteries, as history

Round 1 ran 18 boots across two batteries (`serials/slice1/smoke-*` and
`serials/slice1/r13-final/`), 3 strict and 3 production PASS in each. Those
batteries were run at the round-1 kernel; round 2 then changed kernel source —
`4770c056` dropped `nomem` from four install blocks — so they are not runs of
the kernel this document is committed at, and round 1's sentence claiming the
kernel sources were byte-identical from `9e9131d0` onward stopped being true at
that commit. Round 3's boots above are the ones that describe this head.
claim-lint:ok: 1 of the commits after `9e9131d0` touches `kernel/` — commit
`4770c056` — reproducible with `git diff --stat 9e9131d0..HEAD -- kernel`.

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
discipline itself, or reconcile inline, or be a mechanism primitive — one whose
installed value came in through its own signature and which fetched nothing and
named no shadow. Anything left over
must live in a file CLAUDE.md lists as Tier-1 prohibited, and
`the_tier_one_exemption_matches_the_project_rule` reads that list back out of
CLAUDE.md rather than trusting the test's own copy.
claim-lint:ok: 7 of 7 censused functions are listed immediately below and
reproduced by the `--nocapture` run cited in section 6.

"Reconciles inline" means the site settles BOTH corridor words: it publishes a
root into `saved_process_cr3` and it retires the pending switch with
`set_next_cr3(0)`. Round 2 tightened this — the earlier predicate cleared a site
that merely named `set_saved_process_cr3`, which is the weaker half, because the
corridor reads `next_cr3` first and installs it whenever it is non-zero. The
predicate is a shape (`settles_both_shadows`), not a list of blessed names, and
it is applied at both ends: to the inline installers themselves and to the
aarch64 callers of a mechanism primitive. Section 11 records the mutation runs
that show each end reddening.
claim-lint:ok: both ends redden under the round-2 mutations recorded at
`serials/slice1/r2/mutation-a-nomem-readded.txt` and
`serials/slice1/r2/mutation-b-next-cr3-deleted.txt`, each exit 101.

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
name `adopt_process_ttbr0` or `quiesce_ttbr0_for_exit`, or settle both shadow
words itself, or be the MMU bring-up that installs the kernel root before
per-CPU state exists. That census is what found the `process_memory.rs` site.

`the_discipline_installs_in_order_and_orders_the_shadow_stores` pins the
discipline module's own asm: each of the 2 install helpers in
`kernel/src/arch_impl/aarch64/ttbr0.rs` must run `dsb ishst` → `msr ttbr0_el1` →
`isb` → `tlbi vmalle1is` → `dsb ish` → `isb` in that order, and must not carry
`nomem`. Its coverage is an equality rather than a floor — the install
occurrences reached by the walk must equal the occurrences the file holds — so a
helper added later cannot slip past it.
`no_ttbr0_installer_claims_it_touches_no_memory` applies the `nomem` half to
the censused installers kernel-wide, with the same Tier-1 disposition as the
shadow census: print, do not pin.
claim-lint:ok: 7 of 7 censused installs are checked and 1 of 7 is the Tier-1
site it prints; the run is `serials/slice1/r3/structure-suites.txt`.
`the_dispatch_ttbr0_switch_settles_both_shadows` pins
`context_switch::switch_ttbr0_if_needed` independently of the census, because it
is the install every userspace thread takes on dispatch: the root it publishes
into `saved_process_cr3` must be the operand it just installed, it must clear
`next_cr3`, and its block must not be `nomem`.

Anti-vacuity, 5 legs in that same file: a synthetic site that reads a root out of
the process manager and installs it raw is classified unreconciled; a synthetic
site whose operand is a masked parameter is still admitted as a primitive; a
synthetic site that fetches its root is rejected as a primitive; a synthetic
site that touches one shadow is rejected as a primitive; and a synthetic
aarch64 wrapper handing a process root straight to a primitive is caught by the
caller census and cleared once routed through the discipline.

Round 2 added 3 more self-contained tests, so the tightened predicates cannot go
vacuous without a test going red:

* `the_nomem_check_reads_the_asm_block_and_not_the_prose` — a synthetic install
  block carrying `options(nomem, nostack)` is caught and clears once the option
  is dropped, prose above the block naming the option does not decide the
  answer, and a bare `msr` with no barriers fails the sequence check. That last
  assertion is a 4th leg living inside this test rather than a test of its own,
  which is what round 1's "four legs" wording obscured.
* `settling_one_shadow_is_not_settling_the_shadows` — the four one-word-settled
  shapes (both / saved only / pending only / pending left armed) are accepted
  and rejected as their names say.
* `the_caller_census_rejects_a_half_settled_caller` — a synthetic aarch64
  caller that hands a process root to a primitive and publishes only
  `saved_process_cr3` is caught by the caller census, clearing only when
  `set_next_cr3(0)` is added beside it.

claim-lint:ok: 3 of 3 are named above and run in
`serials/slice1/r3/structure-suites.txt`; the 4th leg is an assertion inside
the 1st of the 3.

Round 3 added 4 more tests, closing three gaps round 2 left open. Section 12
records what each one changed and the mutation that shows it is not vacuous.

* `every_non_primitive_ttbr0_install_performs_the_install_sequence` — the
  6-step sequence, applied to each censused install that is not a mechanism
  primitive, not just to the discipline module. 5 of the 7 censused functions
  are checked, in 3 files, with the census's Tier-1 disposition. The two
  mechanism primitives are exempt and the test's doc comment says what that
  narrows: 2 of 2 of them run no `isb` after the `msr` and no
  `tlbi vmalle1is`, and this ratchet makes no claim that their callers make up
  for it.
* `the_sequence_census_catches_an_install_outside_the_discipline_module` — the
  self-contained leg for the above.
* `every_caller_of_the_kernel_root_install_settles_the_shadows` — the callers
  of `switch_ttbr0_to_kernel`, which settles neither shadow by design and whose
  callers no round-2 check constrained. 3 of 3 callers at this head must either
  settle both words themselves -- clearing them to 0 when no process root is
  live -- or sit in one interrupt-masked window whose 2 exits both reinstall
  through a helper that settles both.
* `the_kernel_root_caller_census_catches_a_caller_that_leaves_the_shadows_armed`
  — its self-contained leg, in 4 parts: bare caller caught, zeroed caller
  cleared, half-zeroed caller caught, masked-window caller cleared and then
  caught again once its failure arm stops reinstalling.

`settles_both_shadows` also changed in round 3: it scores the LAST write to
each shadow word rather than the first, so a body that clears `next_cr3` and
arms it again afterwards no longer passes.

`tests/exec_lock_order_structure.rs` gains two negative tests of its own:
deleting `set_next_cr3(0)` from the helper reddens the exec-path T4 validator,
and putting a raw `msr` back into `sys_exec_aarch64` reddens it too.

## 8. Builds and suites at this head (round 3)

The rows below were run in round 3, at this head, on this Mac. Each aarch64
artifact carries the full `cargo` command on its second line and a
`BUILD_EXIT=` line at the end, so the exit status is recorded rather than
asserted.

| what | result | artifact under `serials/slice1/r3/` |
|---|---|---|
| aarch64 no features (the production profile) | `BUILD_EXIT=0`; 1 `^(warning\|error)` line, the toolchain notice below | `aarch64-no-features-build.txt` |
| its `check-kernel-no-neon.sh` | `NO_NEON_EXIT=0`, PASS, 0 FP/SIMD load/store in `.text` | `aarch64-no-features-no-neon.txt` |
| aarch64 `--features boot_tests`, `aarch64-breenix-kernel.json`, soft-float | `BUILD_EXIT=0`; the same 1 line | `aarch64-boot_tests-build.txt` |
| its `check-kernel-no-neon.sh` | `NO_NEON_EXIT=0`, PASS | `aarch64-boot_tests-no-neon.txt` |
| aarch64 `--features testing` | `BUILD_EXIT=0`; the same 1 line (it is the boot that fails, section 5) | `aarch64-testing-build.txt` |
| its `check-kernel-no-neon.sh` | `NO_NEON_EXIT=0`, PASS | `aarch64-testing-no-neon.txt` |
| x86_64 `--features testing,external_test_bins --bin qemu-uefi`, on beast | `BUILD_EXIT=0`, the `^(warning\|error)` grep printed 0 lines | `x86-testing-external_test_bins-build.txt` |
| the 27 `tests/*_structure.rs` suites | 27 of 27 green, 528 cases, 0 failures | `structure-suites.txt` |
| the userspace ELFs the `boot_tests` build links | copied, not built here; 152 of 152 files identical to the primary working copy | `userspace-elfs.md` |

0 of the 3 aarch64 builds emit a warning attributable to this tree. The one
warning `cargo` prints on each `-Z build-std` invocation here — "the following
packages contain code that will be rejected by a future version of Rust: core
v0.0.0" — is about the pinned toolchain's own `core`, is present identically at
`origin/main`, and is not this branch's.
claim-lint:ok: 3 of 3 aarch64 `-Z build-std` profiles built in round 3 emit it
and nothing else, visible in the 3 build artifacts named above.

Round 1's rows were run at `ceda999d` and round 2's at `887f56d2`; both are
kept in section 11 as that round's record rather than restated here, because
round 2 changed kernel source and round 3 rebuilt everything afterwards.

## 9. The x86_64 build

x86 builds run on beast, not on this Mac. The clone at `/root/breenix-ttbr0`
inside the `breenix-x86` Incus container was reset to this branch and built
with the standard command:

```
cargo build --release --features testing,external_test_bins --bin qemu-uefi 2>&1 | grep -E "^(warning|error)"
```

Round 3 ran it at `0f2621b0`, and the container echoed that commit back from
its own `git rev-parse HEAD` before building. The grep printed nothing and
`BUILD_EXIT=0`. The transcript, including the command and the container's
`rev-parse` line, is `serials/slice1/r3/x86-testing-external_test_bins-build.txt`.

The commits after `0f2621b0` on this branch are documentation only, so the x86
sources the container built are the x86 sources at the head this document is
committed at.

**No x86 boot gate was run, and this round did not intend to run one**: the
x86 `boot_tests` gate is red on `main` for #787, and this slice touches no x86
code path. The x86 reds tracked at #630, #636, #554, #608 and #540 are
unrelated to it as well.

## 10. Build and boot logs, by path

Round 3's artifacts — these are the ones that describe this head:

| file | what it is |
|---|---|
| `serials/slice1/r3/aarch64-no-features-build.txt` | the production-profile build, command and `BUILD_EXIT=0` |
| `serials/slice1/r3/aarch64-no-features-no-neon.txt` | its `check-kernel-no-neon.sh`, `NO_NEON_EXIT=0` |
| `serials/slice1/r3/aarch64-boot_tests-build.txt` | the `--features boot_tests` build, command and `BUILD_EXIT=0` |
| `serials/slice1/r3/aarch64-boot_tests-no-neon.txt` | its `check-kernel-no-neon.sh`, `NO_NEON_EXIT=0` |
| `serials/slice1/r3/aarch64-testing-build.txt` | the `--features testing` build, command and `BUILD_EXIT=0` |
| `serials/slice1/r3/aarch64-testing-no-neon.txt` | its `check-kernel-no-neon.sh`, `NO_NEON_EXIT=0` |
| `serials/slice1/r3/x86-testing-external_test_bins-build.txt` | the beast x86 build at `0f2621b0`, command, `rev-parse` echo, empty grep, `BUILD_EXIT=0` |
| `serials/slice1/r3/structure-suites.txt` | the 27 suites, one `cargo test --test` invocation each, 528 cases |
| `serials/slice1/r3/userspace-elfs.md` | where the `boot_tests` profile's userspace ELFs came from, with the hash comparison |
| `serials/slice1/r3/README.md` | the 3 round-3 anti-vacuity mutations, verbatim, with the byte-copy hashes |
| `serials/slice1/r3/mutation-n003-sequence-stripped.txt` | N-003's run, exit 101 |
| `serials/slice1/r3/mutation-n004-next-cr3-rearmed.txt` | N-004's run, exit 101 |
| `serials/slice1/r3/mutation-n005-quiesce-shadows-dropped.txt` | N-005's run, exit 101 |
| `serials/slice1/r3/restored-green.txt` | the suite after the byte-copy restores, exit 0, 20 of 20 |
| `serials/slice1/prove-r3/prod-boot1-FAIL-serial.txt` | the whole serial of round 3's production red |
| `serials/slice1/prove-r3/prod-boot1-FAIL-gate.txt` | that boot's gate transcript, with its pre-launch process count |
| `serials/slice1/prove-r3/strict-boot1-PASS-gate.txt` and its 2 siblings | the 3 strict boots' gate transcripts, each with its pre-launch process count |
| `serials/slice1/prove-r3/prod-boot2-PASS-gate.txt` and its 1 sibling | the 2 green production boots' gate transcripts, likewise |

Earlier rounds, kept for history:

| file | what it is |
|---|---|
| `serials/slice1/builds/` | round 1's builds, run at `ceda999d` |
| `serials/slice1/smoke-*`, `serials/slice1/r13-final/` | round 1's 18 boots |
| `serials/slice1/main-control-testing/` | round 1's `--features testing` control at `origin/main` `d6b7a186` |
| `serials/slice1/mutation-5/`, `serials/slice1/diffs/` | section 3's latency probe and its diff |
| `serials/slice1/r2/` | round 2: the 3 rebuilt aarch64 profiles with their no-neon runs, the 27-suite run, and the 2 anti-vacuity mutation runs |


## 11. Review round 2 (R154)

Five findings were closed on this branch. Sections 1–10 were rewritten where a
round-1 sentence was wrong, so they read as the tree stands; what follows is the
round's own record — what changed, and the run that shows each ratchet is not
vacuous.

### F-001 — `nomem` on the install blocks

`adopt_process_ttbr0` declared its asm `options(nomem, nostack)`. `nomem` tells
the compiler the block reads and writes no memory, which is a licence to move
memory accesses across it — and the memory in question is exactly the two
per-CPU shadow stores that follow the install and the caller's page-table
stores that must precede it. The option is dropped; `nostack` stays.

What the block now orders relative to the surrounding Rust stores: without
`nomem` the compiler must assume the asm may read or write the same memory as
the code around it, so it may not move the `set_saved_process_cr3` /
`set_next_cr3(0)` stores, or a caller's page-table stores, across the block.
That is a constraint on the compiler alone. It adds no instruction, and it makes
no claim about what another CPU observes. The hardware ordering is unchanged:
`dsb ishst` before the `msr`, `dsb ish; isb` after.
claim-lint:ok: the rebuilt kernels are at
`serials/slice1/r2/aarch64-no-features-build.txt`,
`serials/slice1/r2/aarch64-boot_tests-build.txt` and
`serials/slice1/r2/aarch64-testing-build.txt`, 3 of 3 exit 0.

No miscompilation was observed. This closes a permission, not a measured
reorder, and the doc comments on both helpers say so in those terms.

The finding named the discipline helper. 3 more sites in this kernel had the
identical shape — an install block declared `nomem` with shadow stores beside
it — so 4 of the 4 are fixed together:

```
kernel/src/arch_impl/aarch64/ttbr0.rs::adopt_process_ttbr0            (the finding)
kernel/src/arch_impl/aarch64/ttbr0.rs::switch_ttbr0_to_kernel         (same file, quiesce_ttbr0_for_exit stores after it)
kernel/src/arch_impl/aarch64/syscall_entry.rs::restore_ttbr0_after_failed_exec
kernel/src/arch_impl/aarch64/context_switch.rs::switch_ttbr0_if_needed
```

The two mechanism primitives, `paging.rs::write_root` and
`memory/arch_stub.rs::Cr3::write`, already carried `options(nostack)` alone and
were not touched. The tenth site,
`kernel/src/syscall/time.rs::ensure_current_address_space`, is `nomem` and
carries the same hazard; it is Tier-1 and is disclosed here and printed by the
suite on every run rather than changed (section 6).
claim-lint:ok: 4 of the 6 non-Tier-1 censused installs carried `nomem` and were
changed; the run that prints the remaining Tier-1 site is
`serials/slice1/r2/structure-suites.txt`.

One consequence worth stating plainly: the raw block this slice replaced at
`launch_init_from_elf` carried `options(nostack, preserves_flags)`. Routing that
site through the discipline helper as round 1 shipped it therefore *added*
`nomem` where the code had not had it. This change removes it again.

### F-002 — ratchet for the install shape

`the_discipline_installs_in_order_and_orders_the_shadow_stores` (new) asserts,
for each of the 2 install helpers `kernel/src/arch_impl/aarch64/ttbr0.rs` holds
(found by shape, not by name), that the asm block performs `dsb ishst` → `msr ttbr0_el1` → `isb` →
`tlbi vmalle1is` → `dsb ish` → `isb` in order and does not carry `nomem`. The
option is read off the extracted `asm!` block, not the function body, so prose
naming `nomem` cannot decide the answer. Coverage is an equality, not a floor:
the install occurrences inside the checked bodies must equal the occurrences the
file holds.

`no_ttbr0_installer_claims_it_touches_no_memory` (new) applies the `nomem` half
to the censused installers kernel-wide, with the shadow census's Tier-1
disposition — print, do not pin.
claim-lint:ok: 7 of 7 censused installs are checked, 6 of 7 pinned and 1 of 7
printed, in `serials/slice1/r2/structure-suites.txt`.

Anti-vacuity, recorded verbatim in
`serials/slice1/r2/mutation-a-nomem-readded.txt`: `options(nostack)` was put
back to `options(nomem, nostack)` in `adopt_process_ttbr0` in a scratch copy of
the file, and

```
$ cargo test --test ttbr0_shadow_reconciliation_structure
exit: 101
test result: FAILED. 14 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out

the_discipline_installs_in_order_and_orders_the_shadow_stores panicked:
adopt_process_ttbr0: the install block carries `nomem`, which tells the
compiler it reads and writes no memory -- a licence to move the per-CPU shadow
stores and the caller's page-table stores across the barriers

no_ttbr0_installer_claims_it_touches_no_memory panicked:
these TTBR0 installs are declared `nomem`, so the compiler may move the
surrounding shadow and page-table stores across the barriers:
["kernel/src/arch_impl/aarch64/ttbr0.rs::adopt_process_ttbr0"]
```

The file was then restored from the byte copy taken before the edit, and the
suite is green at the recorded commit (`serials/slice1/r2/structure-suites.txt`,
`ttbr0_shadow_reconciliation_structure ... passed=16 failed=0`).

### F-003 — one shadow was being accepted for two

The raw-install ratchet cleared an inline installer as soon as its body named
`set_saved_process_cr3`; it did not ask for `set_next_cr3(0)`. That is the
weaker half. The corridor reads `next_cr3` FIRST and installs it whenever it
holds a value other than `0`, so a site that publishes a correct
`saved_process_cr3` and leaves a stale `next_cr3` armed has decided which root
the next return to EL0 runs on just as surely as a raw `msr` would. The primitive-caller census had the same
asymmetry in its clearance list.

Both now go through one shape predicate, `settles_both_shadows`: a body
qualifies when it publishes a root other than `0` into `saved_process_cr3` and
clears `next_cr3` with a literal `0`. It is a shape, not a list of blessed
function names — any site that keeps the two corridor words in agreement with
the register satisfies it.
claim-lint:ok: the 4 shapes it separates (both / saved only / pending only /
pending left armed) are the 4 legs of
`settling_one_shadow_is_not_settling_the_shadows`, run in
`serials/slice1/r2/structure-suites.txt`.

`context_switch::switch_ttbr0_if_needed` was reached through the census and had
no pin of its own, though it is the install a userspace thread takes on each
dispatch that changes address space. `the_dispatch_ttbr0_switch_settles_both_shadows` (new) pins
it directly: the root it publishes into `saved_process_cr3` must be the operand
it just installed, it must clear `next_cr3`, and its block must not be `nomem`.

Anti-vacuity, recorded verbatim in
`serials/slice1/r2/mutation-b-next-cr3-deleted.txt`: `set_next_cr3(0)` was
deleted from `switch_ttbr0_if_needed` in a scratch copy of the file, and

```
$ cargo test --test ttbr0_shadow_reconciliation_structure
exit: 101
test result: FAILED. 14 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out

the_dispatch_ttbr0_switch_settles_both_shadows panicked:
the dispatch switch must also retire the pending switch it consumed: a
`next_cr3` left armed is installed FIRST on the next return to EL0

every_ttbr0_install_settles_the_per_cpu_shadows panicked:
these TTBR0 installs leave one or both per-CPU shadows naming another root:
["kernel/src/arch_impl/aarch64/context_switch.rs::switch_ttbr0_if_needed"]
```

The file was then restored from the byte copy taken before the edit, and the
suite is green at the recorded commit — the same
`ttbr0_shadow_reconciliation_structure ... passed=16 failed=0` row.

Four self-contained legs were added beside the two mutations, so the tightened
predicates cannot quietly go vacuous later:
`the_nomem_check_reads_the_asm_block_and_not_the_prose`,
`settling_one_shadow_is_not_settling_the_shadows` (both / saved only / pending
only / pending left armed), `the_caller_census_rejects_a_half_settled_caller`,
and the bare-`msr` leg of the sequence check.

### F-009 — "one hunk" was false

Section 2 said the diff to `launch_init_from_elf` is one hunk. It is two:

```
$ git diff --unified=0 origin/main..HEAD -- kernel/src/main_aarch64.rs | rg "^@@"
@@ -90 +89,0 @@ fn launch_init_from_elf(
@@ -271,12 +270,26 @@ fn launch_init_from_elf(
```

The first removes `use core::arch::asm;`, which the replaced block was the
file's last user of; the second is the install itself. The sentence in section 2
now says two hunks and names both.

### F-012 — trailing whitespace in an evidence artifact

`serials/slice1/diffs/mutation-and-probe.diff` line 13 was a single space — the
unified-diff context line for a blank source line — which made
`git diff --check origin/main..HEAD` exit 2. Only that whitespace was removed;
the line is now empty and no other byte of the artifact changed. It is the sole
trailing-whitespace line the file held (`grep -c ' $'` on it returns 1 before,
0 after), and `git diff --check` is clean at the round-2 head.

### F-007 — disposition

Recorded verbatim, as the arbitration rule this branch is held to:

> the reviewer's strict smoke 1/3 miss (exec smoke outside the fixed window,
> heartbeats alive through 18617 ms, no fault marker) matches the
> pre-adjudicated host-load window-miss signature only when the host was loaded
> at launch; the round-2 prove (strict x10 under the <=2-QEMU rule) is the
> arbiter; a same-shape miss on a quiet host is UNATTRIBUTED and gate-failing

No boot was run in this round: it is a source-and-docs round, and the strict x10
run named above is the separate arbiter.
claim-lint:ok: the round's own evidence is the 2 mutation runs and the 27-suite
run under `serials/slice1/r2/`; F-007 is dispositioned, not measured, here.

### Builds, suites and claim-lint at the round-2 head

| what | result | artifact |
|---|---|---|
| aarch64 no features (production profile) | `BUILD_EXIT=0`, 1 of 1 `^(warning\|error)` line is the toolchain's `core v0.0.0` future-incompat notice | `serials/slice1/r2/aarch64-no-features-build.txt` |
| its `check-kernel-no-neon.sh` | exit 0, PASS, 0 FP/SIMD load/store in `.text` | `serials/slice1/r2/aarch64-no-features-no-neon.txt` |
| aarch64 `--features boot_tests` | `BUILD_EXIT=0`, same single toolchain warning | `serials/slice1/r2/aarch64-boot_tests-build.txt` |
| its `check-kernel-no-neon.sh` | exit 0, PASS | `serials/slice1/r2/aarch64-boot_tests-no-neon.txt` |
| aarch64 `--features testing` | `BUILD_EXIT=0`, same single toolchain warning | `serials/slice1/r2/aarch64-testing-build.txt` |
| its `check-kernel-no-neon.sh` | exit 0, PASS | `serials/slice1/r2/aarch64-testing-no-neon.txt` |
| the 27 `tests/*_structure.rs` suites | 27 of 27 green, 524 cases, 0 failures | `serials/slice1/r2/structure-suites.txt` |

The `boot_tests` profile `include_bytes!`s userspace ELFs that a fresh worktree
does not carry; they were produced here by
`./userspace/programs/build.sh --arch aarch64` (148 binaries installed, exit 0)
before that build was run. This branch changes no userspace source.

x86_64 was not rebuilt in round 2: x86 builds run on beast and that round
touched no x86 code path. Section 9 now carries round 3's x86 build, run at
`0f2621b0`, rather than round 1's.

```
claim-lint: scripts/claim-lint.py                                          -> exit 0
claim-lint: scripts/claim-lint.py --files TTBR0-SHADOW-SLICE-2026-09-04.md -> exit 0
claim-lint: scripts/claim-lint.py --files serials/slice1/diffs/mutation-and-probe.diff -> exit 0
claim-lint: scripts/claim-lint.py --files serials/slice1/r2/{structure-suites,mutation-a-nomem-readded,mutation-b-next-cr3-deleted}.txt -> exit 0
```

(The `--files` paths above are shown relative to
`docs/planning/green-program/aarch64-testing/`; the tool was given the full
repo-relative paths.) Whole-file `--files` runs over the 4 kernel and test
sources this round edited return exit 1 on prose those files already carried;
those findings sit outside this round's hunks, which is what the diff-mode run
above scores, and it is clean.
claim-lint:ok: diff mode reports "clean (56 file(s) checked, changed hunks vs
d6b7a186e37b)" with 269 pre-existing findings outside those hunks not
reported.

claim-lint: scripts/claim-lint.py --files docs/planning/green-program/aarch64-testing/TTBR0-SHADOW-SLICE-2026-09-04.md -> exit 0

## 12. Round 3 (R158)

Round 2's review left three ratchet gaps open and five documentation claims
that were no longer true of the tree. Round 3 closes 3 of 3 gaps and 5 of 5
claims in one pass and re-derives the evidence at this head. It changes no
kernel source: the diff against the round-2 head is
`tests/ttbr0_shadow_reconciliation_structure.rs`, this file, and the round-3
artifacts.
claim-lint:ok: 0 of round 3's commits touch `kernel/`, reproducible with
`git diff --stat e03d4cea..HEAD -- kernel`.

### N-003 — the install sequence ran over one file

`the_discipline_installs_in_order_and_orders_the_shadow_stores` checked the
6-step sequence in `kernel/src/arch_impl/aarch64/ttbr0.rs` only. The two
installs outside that module — `context_switch.rs::switch_ttbr0_if_needed` and
`syscall_entry.rs::restore_ttbr0_after_failed_exec` — could lose their barriers
with the suite still green.

`every_non_primitive_ttbr0_install_performs_the_install_sequence` (new) applies
`performs_install_sequence` to each censused install that is not a mechanism
primitive. It reaches 5 sites at this head, in 3 files:

```
kernel/src/arch_impl/aarch64/context_switch.rs::switch_ttbr0_if_needed
kernel/src/arch_impl/aarch64/syscall_entry.rs::restore_ttbr0_after_failed_exec
kernel/src/arch_impl/aarch64/ttbr0.rs::switch_ttbr0_to_kernel
kernel/src/arch_impl/aarch64/ttbr0.rs::adopt_process_ttbr0
kernel/src/syscall/time.rs::ensure_current_address_space
```

Its coverage floor is census-shaped rather than a name list: at least 4 sites,
in at least 2 files, so it cannot quietly collapse back onto the discipline
module. The Tier-1 site is checked and printed rather than pinned, the same
disposition the other kernel-wide censuses use.

The mechanism-primitive exemption is a real narrowing and the test's own doc
comment says so: 2 of 2 primitives at this head — `paging.rs::write_root` and
`memory/arch_stub.rs::Cr3::write` — run `dsb ishst` / `msr` / `dsb ish` / `isb`,
with no `isb` after the `msr` and no `tlbi vmalle1is`, so neither would pass
this check if it were applied to them. The ratchet makes no claim that their
callers make up for it.

Mutation, recorded verbatim in
`serials/slice1/r3/mutation-n003-sequence-stripped.txt`: the `tlbi vmalle1is`,
`dsb ish` and trailing `isb` lines were deleted from the install block in
`switch_ttbr0_if_needed`, and

```
$ cargo test --test ttbr0_shadow_reconciliation_structure
exit: 101
test result: FAILED. 19 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out

every_non_primitive_ttbr0_install_performs_the_install_sequence panicked:
these TTBR0 installs do not run ["dsb ishst", "msr ttbr0_el1", "isb", "tlbi vmalle1is", "dsb ish", "isb"] in order, so a stale translation can survive the install or the root can be taken before it is visible: ["kernel/src/arch_impl/aarch64/context_switch.rs::switch_ttbr0_if_needed"]
```

The file was restored from the byte copy taken before the edit and the SHA-256
matched.

### N-004 — the predicate read the first write, not the last

`settles_both_shadows` read the FIRST occurrence of each accessor. A body that
cleared `next_cr3` and then armed it again with a root passed, because the
reader stopped at the first call — while the corridor reads whatever the last
store left there.

`call_argument` becomes `call_arguments` plus `last_call_argument`, and both
shadow predicates now score the LAST write to each word. A companion predicate,
`zeroes_both_shadows`, scores the kernel-root disposition (both words literal
`0`) the same way; the two are deliberately not interchangeable, because
`settles_both_shadows` rejects a `saved_process_cr3` of 0.

Mutation, recorded verbatim in
`serials/slice1/r3/mutation-n004-next-cr3-rearmed.txt`:
`Aarch64PerCpu::set_next_cr3(next_ttbr0);` was appended immediately after the
existing `set_next_cr3(0)` in `switch_ttbr0_if_needed`, and

```
$ cargo test --test ttbr0_shadow_reconciliation_structure
exit: 101
test result: FAILED. 18 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out

the_dispatch_ttbr0_switch_settles_both_shadows panicked:
the dispatch switch must also retire the pending switch it consumed: a `next_cr3` left armed is installed FIRST on the next return to EL0

every_ttbr0_install_settles_the_per_cpu_shadows panicked:
these TTBR0 installs leave one or both per-CPU shadows naming another root: ["kernel/src/arch_impl/aarch64/context_switch.rs::switch_ttbr0_if_needed"]
```

Under round 2's first-occurrence reader that same mutation passed.

### N-005 — the kernel-root install's callers were unconstrained

`switch_ttbr0_to_kernel` settles neither shadow, by design: it is the mechanism,
and the kernel root is not a value either corridor arm may install on a return
to EL0. The obligation is therefore the caller's, and no round-2 check scored
those callers — the primitive-caller census skips the discipline module, and
this helper is not a mechanism primitive by that census's definition.

`every_caller_of_the_kernel_root_install_settles_the_shadows` (new) censuses
them. 3 callers at this head:

```
kernel/src/arch_impl/aarch64/syscall_entry.rs::sys_exit_aarch64
kernel/src/arch_impl/aarch64/syscall_entry.rs::sys_exec_aarch64
kernel/src/arch_impl/aarch64/ttbr0.rs::quiesce_ttbr0_for_exit
```

Each has to discharge the obligation one of two ways. 2 of the 3 —
`sys_exit_aarch64` and `quiesce_ttbr0_for_exit` — settle both words themselves,
clearing them to 0 because no process root is live on this CPU any more. The
remaining 1, `sys_exec_aarch64`, is the exec shape: the kernel-root install and
the reinstall that ends it are one `without_interrupts` window, and both ways
out of that window go through a helper that settles both words —
`adopt_process_ttbr0` on the success arm and `restore_ttbr0_after_failed_exec`
on the failure arm.

That window is pinned by two suites this one names rather than duplicating:
`validate_aarch64_failed_exec_ttbr0_rollback` in
`tests/context_restore_structure.rs` requires the TTBR0 capture, the
kernel-root transition and the `exec_process_with_argv` call to appear exactly
once in that order and the `Err` arm to roll back before any return; and
`validate_sys_exec_releases_process_manager` in
`tests/exec_lock_order_structure.rs` requires exactly one
`adopt_process_ttbr0(` after `commit.apply()` and no raw `msr ttbr0_el1`
anywhere in the function.

Mutation, recorded verbatim in
`serials/slice1/r3/mutation-n005-quiesce-shadows-dropped.txt`: the
`set_saved_process_cr3(0)` / `set_next_cr3(0)` pair was deleted from
`quiesce_ttbr0_for_exit`, and

```
$ cargo test --test ttbr0_shadow_reconciliation_structure
exit: 101
test result: FAILED. 19 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out

every_caller_of_the_kernel_root_install_settles_the_shadows panicked:
these aarch64 callers install the kernel root and leave the per-CPU TTBR0 shadows naming another one, so the next return to EL0 may reinstall a root this CPU has just left: ["kernel/src/arch_impl/aarch64/ttbr0.rs::quiesce_ttbr0_for_exit"]
```

**Disclosed narrowing.** The aarch64 scope filter these caller censuses use
covers code under `kernel/src/arch_impl/aarch64/` and functions carrying
`#[cfg(target_arch = "aarch64")]`. Code with no `cfg` at all is outside it.
`kernel/src/memory/kernel_page_table.rs::build_master_kernel_pml4` is exactly
that shape: cfg-free, in a cfg-free file, and it calls the `Cr3::write`
primitive. No aarch64 code path executes it at this head — its only caller is
the cfg-free `kernel/src/memory/mod.rs::init`, whose only caller is
`kernel_main` in `kernel/src/main.rs` behind `#[cfg(target_arch = "x86_64")]` —
but that is a fact about the current call graph, not something the filter
checks. A cfg-free caller added on the aarch64 side would sit outside each
census in the file. This is written on `aarch64_scoped_functions` in the test
as well as here.

### The documentation claims that were wrong

* **N-001.** Section 5 said round 1's boots were a re-run at the returned head
  because "the kernel sources are byte-identical" from `9e9131d0` onward. Round
  2's `4770c056` dropped `nomem` from four install blocks, so that stopped
  being true. Section 5 is now round 3's boots at this head, with round 1's
  batteries kept as clearly-labelled history and the false byte-identical
  sentence removed. Section 8 and section 10 likewise now carry round 3's
  builds and artifact paths rather than round 1's.
* **N-006.** The header counted "10 installs / 9 of 10 routed / 1 of 10 Tier-1"
  while section 7's census counted 7 functions, and nothing said these were
  different things. The header now states both accountings and section 2 says
  which one "the tenth site" refers to. The test module's own doc carried the
  same conflation twice over; it now states both counts once, and the duplicated
  `claim-lint:ok` sentence on `ttbr0_install_census` is collapsed to one.
* **N-007.** Round 2's build table used a code-font `BUILD_EXIT=0` token that
  its artifacts did not actually record. Round 3's artifacts do: each carries
  the full `cargo` command on its second line and a `BUILD_EXIT=` line at the
  end, and `serials/slice1/r3/userspace-elfs.md` records where the userspace
  ELFs came from — copied from the primary working copy rather than built here,
  with the hash comparison showing 152 of 152 files match.
* **N-008.** "The tenth site" now cross-references the two accountings
  explicitly wherever it appears.
* **N-009.** Section 7 said round 2 added "4 more self-contained legs" and then
  listed 4 items, one of which is an assertion inside another. It now says 3
  tests, names them, and says which leg lives inside the first of them.

### What round 3 measured

Builds, suites and boots are in sections 5 and 8, with artifact paths in
section 10. In summary: 3 of 3 aarch64 profiles `BUILD_EXIT=0` with
`check-kernel-no-neon.sh` PASS, the beast x86 build `BUILD_EXIT=0` with an
empty `^(warning|error)` grep, 27 of 27 structure suites green over 528 cases,
3 of 3 strict boots PASS, and 2 of 3 production boots PASS with 1 red carrying
open issue #690's exact signature and recorded as UNATTRIBUTED.

**This round does not call the branch landable.** The production red is
unattributed by the pre-adjudicated set it was given, and no control at
`origin/main` was run on that profile, so no measurement here rules the branch
out either.
claim-lint:ok: 1 of 3 production boots is the red, recorded at
`serials/slice1/prove-r3/prod-boot1-FAIL-gate.txt`.

```
claim-lint: scripts/claim-lint.py                                                                                          -> exit 0
claim-lint: scripts/claim-lint.py --files docs/planning/green-program/aarch64-testing/TTBR0-SHADOW-SLICE-2026-09-04.md     -> exit 0
claim-lint: scripts/claim-lint.py --files docs/planning/green-program/aarch64-testing/serials/slice1/r3/README.md          -> exit 0
claim-lint: scripts/claim-lint.py --files docs/planning/green-program/aarch64-testing/serials/slice1/r3/userspace-elfs.md  -> exit 0
claim-lint: scripts/claim-lint.py --files docs/planning/green-program/aarch64-testing/serials/slice1/r3/structure-suites.txt docs/planning/green-program/aarch64-testing/serials/slice1/r3/mutation-n003-sequence-stripped.txt docs/planning/green-program/aarch64-testing/serials/slice1/r3/mutation-n004-next-cr3-rearmed.txt docs/planning/green-program/aarch64-testing/serials/slice1/r3/mutation-n005-quiesce-shadows-dropped.txt docs/planning/green-program/aarch64-testing/serials/slice1/r3/restored-green.txt -> exit 0
claim-lint: scripts/claim-lint.py --files docs/planning/green-program/aarch64-testing/serials/slice1/r3/x86-testing-external_test_bins-build.txt docs/planning/green-program/aarch64-testing/serials/slice1/prove-r3/prod-boot1-FAIL-gate.txt -> exit 0
claim-lint: scripts/claim-lint.py --files docs/planning/green-program/aarch64-testing/serials/slice1/prove-r3/strict-boot1-PASS-gate.txt docs/planning/green-program/aarch64-testing/serials/slice1/prove-r3/strict-boot2-PASS-gate.txt docs/planning/green-program/aarch64-testing/serials/slice1/prove-r3/strict-boot3-PASS-gate.txt docs/planning/green-program/aarch64-testing/serials/slice1/prove-r3/prod-boot2-PASS-gate.txt docs/planning/green-program/aarch64-testing/serials/slice1/prove-r3/prod-boot3-PASS-gate.txt -> exit 0
```

The whole-file run over `tests/ttbr0_shadow_reconciliation_structure.rs`
returns exit 1 on prose the file already carried before this round; those
findings sit outside round 3's hunks, which is what the diff-mode run above
scores, and that run is clean.
claim-lint:ok: diff mode reports "clean (77 file(s) checked, changed hunks vs
d6b7a186e37b)" with 269 pre-existing findings outside those hunks not
reported.
