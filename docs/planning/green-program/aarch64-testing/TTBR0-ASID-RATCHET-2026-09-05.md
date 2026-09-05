# TTBR0 ASID ratchet — the value a shadow publish carries (#786 follow-on)

Branch `ratchet/786-asid-tag-census`, based on `origin/main` `4f664c99`.
No behaviour change to the TTBR0 discipline: this round adds a structural
census, a runtime counter, two gate assertions and this record.

## 1. The invariant

A process page-table root published into the per-CPU corridor words is not just
a physical address. `set_next_ttbr0_for_thread` in
`kernel/src/arch_impl/aarch64/context_switch.rs` tags the root it publishes into
`next_cr3` with ASID 1, because the boot identity map's TLB entries are ASID 0
and the nG bits on process page-table entries make a non-zero ASID the thing
that keeps a user VA from matching one of them. The `.Lrestore_saved_ttbr` arm
of `kernel/src/arch_impl/aarch64/syscall_entry.S` loads `saved_process_cr3` at
per-CPU offset 80 and writes it into `ttbr0_el1` with no masking, so the ASID
bits in that word are the ASID the next return to EL0 runs under.
claim-lint:ok: 1 of 1 corridor arm reads that word without masking --
`.Lrestore_saved_ttbr` in `kernel/src/arch_impl/aarch64/syscall_entry.S`,
offset 80.

**The invariant this round ratchets: a process root reaching either corridor
word carries the userspace ASID.**

That invariant was violated on `main` for about five hours. `adopt_process_ttbr0`
published the caller's ASID-untagged root, and eight of the ten routed install
sites hand over a bare `page_table.level_4_frame().start_address().as_u64()`, so
the `nanosleep`, EINTR, `wait`, futex and poll returns went to EL0 on ASID 0
while the dispatch path's returns went on ASID 1. `9e877486` (PR #800) repaired
it inside the discipline: `process_root_ttbr0` masks bits [63:48] off and sets
`USER_ASID_TTBR0`, and `adopt_process_ttbr0` / `restore_process_ttbr0` install
and publish that normalised value.
claim-lint:ok: the 10 routed sites and the 8/1/1 split are enumerated in
`docs/planning/green-program/aarch64-testing/TTBR0-SLICE1B-2026-09-04.md`
section 2b; the corridor arm is `.Lrestore_saved_ttbr` in
`kernel/src/arch_impl/aarch64/syscall_entry.S`.

## 2. Why the shape ratchet did not catch it

`tests/ttbr0_shadow_reconciliation_structure.rs` had, before this round, 23
checks over the same code, and the defect passed all 23. They read SHAPE: which
function installs, whether it settles both shadow words, whether it settles them
in the right order, whether the value it installs traces to a parameter.
`adopt_process_ttbr0` did all of that correctly. What was wrong was the VALUE in
the binding it published, and no check in the file read a value.
claim-lint:ok: 25 of the 27 checks in that file at this head still pass with
the
defect restored, and the 2 that redden are named in
`serials/asid-ratchet/01-structural-anti-vacuity-raw-adopt.txt`.

The one check that touches the ASID at all,
`the_discipline_publishes_the_dispatch_asid`, was added by `9e877486` itself and
reads exactly ONE named function. A second publish site added tomorrow with an
untagged operand fails no check in that file.

**On ruling R19.** The brief for this round cites coordinator ruling R19 as the
statement of why the shape ratchet missed this. That ruling's text is not
recorded in this repository, in issue #786, or in the body of PR #800, so it is
**not quoted here** — quoting it from memory would be the kind of unsourced
sentence the claim-lint control exists to stop. What is quoted above is read out
of the tree instead: the check count, the single-function scope of the ASID
check, and the mutation run that shows both.

## 3. The structural test

`tests/ttbr0_shadow_reconciliation_structure.rs` gains a value census and four
tests. The census walks every call to `set_saved_process_cr3` and
`set_next_cr3` in aarch64-scoped code under `kernel/src` — keyed on the accessor
NAMES, so a new publish site is reached because it has to call one of them — and
classifies the operand's provenance by following the function's own `let` and
assignment bindings:
claim-lint:ok: 17 of 17 calls reached at this head are listed below and in
`serials/asid-ratchet/05-suite-green-with-census.txt`.

| provenance | meaning |
|---|---|
| `cleared` | a literal 0: the corridor arm for that word is disarmed |
| `normalised` | passed through `process_root_ttbr0` in this function |
| `dispatch-tagged` | carries the dispatch path's own ASID tag, parsed and compared against `USER_ASID_TTBR0` |
| `kernel root` | this CPU's kernel root, ASID 0 by construction and not a process root |
| `read back` | read out of `ttbr0_el1` or out of a shadow word — the ASID is whatever was installed |
| `caller-borne` | came in through the signature; resolved one level up, at the callers |
| `UNACCOUNTED` | matched no arm above — the failure class |

`every_shadow_publish_has_an_accounted_asid` fails on any UNACCOUNTED publish,
on a caller-borne publish inside the discipline module itself (that is the
pre-`9e877486` shape exactly), and on any caller that hands a caller-borne
publish an unaccounted value. It prints the whole census on `--nocapture`.

The census at this head, 17 calls, printed by the test and recorded verbatim in
`serials/asid-ratchet/05-suite-green-with-census.txt`:

```
context_switch.rs::setup_idle_return_locked        set_saved_process_cr3(0)              [cleared]
context_switch.rs::setup_idle_return_locked        set_next_cr3(kernel_ttbr0)            [kernel root]
context_switch.rs::switch_ttbr0_if_needed          set_saved_process_cr3(next_ttbr0)     [read back]
context_switch.rs::switch_ttbr0_if_needed          set_next_cr3(0)                       [cleared]
context_switch.rs::set_next_ttbr0_for_thread       set_next_cr3(tagged_ttbr0)            [dispatch-tagged]
exception.rs::set_idle_stack_for_eret              set_saved_process_cr3(0)              [cleared]
exception.rs::set_idle_stack_for_eret              set_next_cr3(kernel_ttbr0)            [kernel root]
syscall_entry.rs::sys_exit_aarch64                 set_saved_process_cr3(0)              [cleared]
syscall_entry.rs::sys_exit_aarch64                 set_next_cr3(0)                       [cleared]
syscall_entry.rs::restore_ttbr0_after_failed_exec  set_saved_process_cr3(ttbr0)          [caller-borne]
syscall_entry.rs::restore_ttbr0_after_failed_exec  set_next_cr3(0)                       [cleared]
ttbr0.rs::adopt_process_ttbr0                      set_saved_process_cr3(ttbr0_value)    [normalised]
ttbr0.rs::adopt_process_ttbr0                      set_next_cr3(0)                       [cleared]
ttbr0.rs::restore_process_ttbr0                    set_saved_process_cr3(root)           [normalised]
ttbr0.rs::restore_process_ttbr0                    set_next_cr3(0)                       [cleared]
ttbr0.rs::quiesce_ttbr0_for_exit                   set_saved_process_cr3(0)              [cleared]
ttbr0.rs::quiesce_ttbr0_for_exit                   set_next_cr3(0)                       [cleared]
```

That is 10 cleared, 2 kernel-root, 2 normalised, 1 dispatch-tagged, 1 read-back
and 1 caller-borne. The caller-borne one is
`restore_ttbr0_after_failed_exec`, whose only caller binds its argument from
`read_ttbr0_for_exec()` — an `mrs ttbr0_el1` — so the caller walk resolves it as
a register read-back and the test passes.

The three supporting tests: `the_publication_census_catches_an_untagged_root`
(five synthetic legs, section 5), `the_shadow_setters_feed_the_runtime_census`
(both per-CPU setters count before they write; the counter takes no lock, makes
no allocation, does no formatting), and
`both_aarch64_gates_fail_on_an_untagged_publish`.

The sentence that stood here about that last test — "both gate scripts carry the
three assertion patterns, so deleting one is a test failure rather than a silent
loss of the ratchet" — was FALSE, and R157/ASID-01 proved it by mutation: the
test asserted only that each script CONTAINS three pattern strings, which stays
true after every assertion using them is deleted. With the assertions removed
and the variable definitions left in place, the strict gate scored a serial
reporting `untagged=3` as PASS while the test stayed green. What the test does
now is in section 12.
claim-lint:ok: the mutation deleted 3 of 3 assertions from each of the 2
gates; the reproduction is section 12

## 4. The runtime census and the gate assertion

The structural census reads shapes. What the shipped kernel actually publishes
is a different question, and the register read-back class is one no source shape
can answer. So the publishes are counted where they are WRITTEN.

`note_shadow_publish` in `kernel/src/arch_impl/aarch64/ttbr0.rs` is called by
`Aarch64PerCpu::set_next_cr3` and `Aarch64PerCpu::set_saved_process_cr3` in
`kernel/src/arch_impl/aarch64/percpu.rs`, before each writes its per-CPU word.
It sorts each published value into four relaxed `AtomicU64` counters: a literal
0 (`cleared`), this CPU's kernel root (`kernel`), a process root carrying
`USER_ASID_TTBR0` (`tagged`), and a process root whose ASID field is anything
else (`untagged`). No lock, no allocation, no formatting, no page-table walk: a
mask-and-compare against the per-CPU kernel root and one relaxed increment. The
ERET corridor is not touched.

`emit_asid_census` prints
`[TTBR0_ASID_CENSUS:untagged=N:tagged=N:kernel=N:cleared=N]` from normal context
at four points: the aarch64 boot path beside the root-custody summary
(`kernel/src/main_aarch64.rs`), the cold `/proc/trace/counters` read
(`kernel/src/fs/procfs/trace.rs`), the boot-test sampling oracle's period
(`kernel/src/task/strand_oracle.rs`), and every aarch64 process exit
(`sys_exit_aarch64`).
claim-lint:ok: 4 of 4 emission sites are named here; a production boot prints
13 to 14 lines and a strict boot 13 to 14, counted in
`serials/asid-ratchet/04-prod-boot1-serial.txt` and its 5 siblings.

The exit-path emission is there because the other three are not enough inside a
gate window, and that was measured, not assumed: the boot-path line prints
`untagged=0:tagged=0:kernel=0:cleared=0` because it runs before any process root
has been published, and the userspace heartbeat's first `/proc/trace/counters`
read happens at 20 s of uptime (`next_dump_ms = 20_000` in
`userspace/programs/src/heartbeat.rs`), past the point either gate keeps QEMU
alive — a whole production boot contains 1 `[PT_ROOT_CUSTODY:` line.
claim-lint:ok: the all-zero boot line and the single `[PT_ROOT_CUSTODY:` line
are both in `serials/asid-ratchet/04-prod-boot1-serial.txt`.

**Gate assertions.** `docker/qemu/run-aarch64-prod-profile-boot-test.sh` and
`docker/qemu/run-aarch64-boot-test-strict.sh` each gained three, in the shape
each script already uses for its other census lines:

1. the marker must be present at least once;
2. no line may report `untagged` other than 0 — this is the failing condition;
3. at least one line must report a `tagged` above 0, because a census that
   counted 0 process-root publishes reports `untagged=0` for the same reason a
   dead counter does.

claim-lint:ok: 3 of 3 assertions are exercised by the four scoring legs in
`serials/asid-ratchet/07-strict-score-legs.txt`.

The strict gate's scoring is exercisable without booting
(`BREENIX_STRICT_SCORE_ONLY=<serial>`). Against the preserved green serial it
scores PASS; against the same serial with `untagged=3` substituted, with the
census lines deleted, and with `tagged` forced to 0 it scores FAIL with the
three distinct reasons.
claim-lint:ok: 4 of 4 scoring legs were run; their output is in section 5b.

## 5. Anti-vacuity

### 5a. The structural leg: the discipline reverted in the tree

`let ttbr0_value = process_root_ttbr0(ttbr0_value);` was deleted from
`adopt_process_ttbr0` — the pre-`9e877486` shape, which publishes the caller's
raw operand — and the suite was run against the mutated tree:

```
$ cargo test --test ttbr0_shadow_reconciliation_structure
...
test result: FAILED. 25 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out

---- every_shadow_publish_has_an_accounted_asid stdout ----
these publishes hand the syscall return corridor a value with no ASID
provenance, and the corridor installs the word verbatim: [
    "kernel/src/arch_impl/aarch64/ttbr0.rs::adopt_process_ttbr0
     set_saved_process_cr3(ttbr0_value) [caller-borne]",
]
```

Exit 101. The new census names the reverted function. The pre-existing
`the_discipline_publishes_the_dispatch_asid` also reddens, on "the installed
value \"ttbr0_value\" must be bound in the discipline body" — it catches this
one function by name, which is the narrowness section 2 is about. Full run:
`serials/asid-ratchet/01-structural-anti-vacuity-raw-adopt.txt`.

The line was then restored and the suite re-run green:
`serials/asid-ratchet/05-suite-green-with-census.txt`, 27 passed, 0 failed.

Synthetic legs, in `the_publication_census_catches_an_untagged_root` and run on
every invocation of the suite: a new site publishing
`page_table.level_4_frame().start_address().as_u64()` is UNACCOUNTED; the same
site wrapped in `process_root_ttbr0` is accepted (without this leg a predicate
that rejected everything would pass the first); the discipline publishing a bare
parameter is caller-borne, which the census fails on inside the discipline
module; a caller-borne publish whose caller hands over a raw root names that
caller; and the same caller capturing from `ttbr0_el1` instead is resolved.
claim-lint:ok: 5 of 5 legs are in that one test, which is part of the 27 in
`serials/asid-ratchet/05-suite-green-with-census.txt`.

### 5b. The runtime leg: the same mutation, booted

With the same revert applied, the production-profile kernel was rebuilt and one
production-gate boot was run:

```
$ ./docker/qemu/run-aarch64-prod-profile-boot-test.sh
...
FAIL: TTBR0 ASID census reported an untagged process-root publish:
  [TTBR0_ASID_CENSUS:untagged=14:tagged=21242:kernel=23939:cleared=44519]
Observed TTBR0 ASID census marker count: 13
Observed TTBR0 ASID census untagged-publish line count: 11
```

Gate exit 1. 11 of the boot's 13 census lines report a non-zero `untagged`, and
the count climbs monotonically to 14 as exec-driven adoptions accumulate. Gate
output: `serials/asid-ratchet/02-runtime-anti-vacuity-prod-gate.txt`; the boot's
own serial: `serials/asid-ratchet/02-runtime-anti-vacuity-prod-serial.txt`. The
line was then restored and the kernel rebuilt for every run in section 8.

**What this leg does NOT exercise (R157/ASID-04).** The 14 untagged publishes it
counts are the exec/init population. Every blocking-resume site routes through
`restore_process_ttbr0`, not through `adopt_process_ttbr0` directly, so
`nanosleep`, EINTR, `wait`, futex and poll returns — the class section 1 opens
with — are not among them. Section 13 runs the leg that reaches them.
claim-lint:ok: 5 of 5 blocking-resume helpers route through
`restore_process_ttbr0`, printed by
`every_blocking_resume_restore_uses_the_guarded_helper` in serials/asid-
ratchet/09-suite-green-after-r157.txt

The strict gate's scoring legs, run against preserved serials:

```
== leg A: the green serial as captured ==
SCORE: PASS - /tmp/asid-ratchet/leg-green.txt
== leg B: one line mutated to untagged=3 ==
SCORE: FAIL - TTBR0 ASID census reported an untagged process-root publish
  ([TTBR0_ASID_CENSUS:untagged=3:tagged=20718:kernel=24444:cleared=44485])
== leg C: every census line deleted ==
SCORE: FAIL - TTBR0 ASID census marker missing
== leg D: every line forced to tagged=0 ==
SCORE: FAIL - TTBR0 ASID census never counted a process-root publish
```

The input to all four legs is `serials/asid-ratchet/03-strict-boot1-serial.txt`,
this round's own first strict boot; the run is
`serials/asid-ratchet/07-strict-score-legs.txt`.

## 6. Builds

Each aarch64 build was preceded by `touch kernel/src/main_aarch64.rs` to force a
kernel recompile — a `cargo build` that hardlinks a cached artifact for a feature
set prints `Finished` without compiling anything, which produced one misleading
gate run earlier in this round (a strict boot scored against a kernel built
before the change; caught because the census marker was absent from a serial
whose binary contained the literal).

| profile | command | exit | `^(warning\|error)` lines | no-NEON |
|---|---|---|---|---|
| default | `cargo build --release --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64` | 0 | 1 (toolchain) | PASS |
| `boot_tests` | same, `--features boot_tests` | 0 | 1 (toolchain) | PASS |
| `testing` | same, `--features testing` | 0 | 1 (toolchain) | PASS |

claim-lint:ok: 3 of 3 build logs are `/tmp` console captures quoted here in
full; the single warning line is reproduced immediately below.

The single warning line in each is the toolchain's own future-incompatibility
notice, disclosed as such rather than counted as clean:

```
warning: the following packages contain code that will be rejected by a future
version of Rust: core v0.0.0 (…/nightly-2025-06-24-aarch64-apple-darwin/…)
```

`scripts/check-kernel-no-neon.sh` was run on the produced binary after each
build and reported `0 FP/SIMD load/store instructions in kernel .text
(allowlisted & suppressed: 0)`.

Build inputs: the worktree has no `rust-fork` checkout, so the aarch64 userspace
ELFs under `userspace/programs/aarch64/` and `target/ext2-aarch64.img` were
copied from the main checkout at the same commit rather than rebuilt here. They
are build outputs of `4f664c99`, and this round changes no userspace source.

## 7. Structure suites

Every `tests/*_structure.rs` suite was run at this head: **29 of 29 green, 546
cases**, recorded in `serials/asid-ratchet/06-structure-suites.txt`. The TTBR0
suite is 27 of 27 (23 before this round, plus the 4 added here).

## 8. Boots

Host-load rule: `pgrep -fl qemu-system-aarch64 | wc -l` was read immediately
before each launch and was within the cap of 2 every time; strict runs one boot
at a time inside its own script.
claim-lint:ok: 4 of 4 launches in this table recorded 0.

| gate | profile | pgrep at launch | result | census lines | last census line |
|---|---|---|---|---|---|
| strict (3 iterations, one invocation) | `boot_tests`, cortex-a72 | 0 | 3/3 PASS | 14 / 13 / 14 | `untagged=0:tagged=20718` / `=17901` / `=20763` |
| prod boot 1 | default, `-cpu max` | 0 | PASS | 13 | `untagged=0:tagged=21835:kernel=24730:cleared=45909` |
| prod boot 2 | default, `-cpu max` | 0 | PASS | 14 | `untagged=0:tagged=24673:kernel=27598:cleared=51477` |
| prod boot 3 | default, `-cpu max` | 0 | PASS | 14 | `untagged=0:tagged=24479:kernel=27414:cleared=51107` |

claim-lint:ok: 6 of 6 boots in this table have their gate output and their
serial preserved under `serials/asid-ratchet/`.

Every census line in all six boots reports `untagged=0`; the gate greps that
report it are absence-of-non-zero greps over the whole serial, not a check of
the last line only.
claim-lint:ok: 0 of 6 serials in `serials/asid-ratchet/` match
`TTBR0_ASID_CENSUS:untagged=[1-9]`; the 6 are `03-strict-boot{1,2,3}-serial.txt`
and `04-prod-boot{1,2,3}-serial.txt`.

Unattributed failures: 0 of 6 boots.
claim-lint:ok: 6 of 6 boots passed their gate; the gate outputs are
`serials/asid-ratchet/03-strict-x3.txt` and `04-prod-boot{1,2,3}.txt`.

## 9. x86

`git diff --name-only 4f664c99..HEAD` at this head -- against the commit this
branch was cut from, because `origin/main` advanced to `2a444455` (PR #801)
while this round was running, and `origin/main..HEAD` would fold that in:

```
docker/qemu/run-aarch64-boot-test-strict.sh
docker/qemu/run-aarch64-prod-profile-boot-test.sh
docs/planning/green-program/aarch64-testing/TTBR0-ASID-RATCHET-2026-09-05.md
docs/planning/green-program/aarch64-testing/serials/asid-ratchet/  (17 files)
kernel/src/arch_impl/aarch64/percpu.rs
kernel/src/arch_impl/aarch64/syscall_entry.rs
kernel/src/arch_impl/aarch64/ttbr0.rs
kernel/src/fs/procfs/trace.rs
kernel/src/main_aarch64.rs
kernel/src/task/strand_oracle.rs
tests/ttbr0_shadow_reconciliation_structure.rs
```

Three of the kernel files live under `kernel/src/arch_impl/aarch64/`, a module
`kernel/src/arch_impl/mod.rs` declares under `#[cfg(target_arch = "aarch64")]`,
and `main_aarch64.rs` is the aarch64 binary's own root. The two shared files —
`kernel/src/fs/procfs/trace.rs` and `kernel/src/task/strand_oracle.rs` — each
gain one statement carrying `#[cfg(target_arch = "aarch64")]` and nothing else,
so the x86 build compiles the same code it compiled before.
claim-lint:ok: 2 of 2 shared files carry exactly 1 cfg-gated statement each;
`git diff origin/main..HEAD -- kernel/src/fs/procfs/trace.rs kernel/src/task/strand_oracle.rs`
shows both hunks.

Compile evidence: the kernel crate is built for `x86_64-unknown-none` as part of
every structural-suite run, and
`target/x86_64-unknown-none/debug/deps/libkernel-*.rlib` was rebuilt during the
29-suite sweep in section 7.
claim-lint:ok: 29 of 29 suites in `serials/asid-ratchet/06-structure-
suites.txt`
build that rlib as a dependency.

Not done: no beast x86 build and no x86 boot test were run this round. The
statement above is "the diff's x86-reachable content is unchanged and the x86
target compiles", not "the x86 gates were run".

## 10. claim-lint

```
claim-lint: scripts/claim-lint.py -> exit 0
claim-lint: scripts/claim-lint.py --files docs/planning/green-program/aarch64-testing/TTBR0-ASID-RATCHET-2026-09-05.md docs/planning/green-program/aarch64-testing/serials/asid-ratchet/README.md -> exit 0
```

## 11. What is NOT claimed

* **No boot in this round observes the ASID a return to EL0 actually ran
  under.** The counter reads what is PUBLISHED into the two corridor words. It
  does not read `ttbr0_el1` after `.Lrestore_saved_ttbr` has installed that
  word, and no probe in this round sampled the register at an exception return.
  A publish counted as `tagged` is evidence about the word, not about the
  translation regime EL0 then used.
* **The runtime census does not see the two assembly stores.**
  `syscall_entry.S` copies the live `ttbr0_el1` into `saved_process_cr3` at
  syscall entry and clears `next_cr3` with `xzr` on the return path. Both are on
  the syscall hot path and neither was instrumented. The argument that neither
  can introduce an untagged value is structural — one copies the register, the
  other writes zero — and is not backed by a measurement.
  claim-lint:ok: 2 of 2 assembly stores are at `syscall_entry.S` offsets 80
  (from `ttbr0_el1`) and 64 (from `xzr`).
* **The structural census is aarch64-scoped, with the scope
  `aarch64_scoped_functions` documents.** Shared code carrying no cfg at all is
  outside it, and so is `kernel/src/per_cpu_aarch64.rs`, whose module
  declaration carries the cfg its path does not. A publish added in either place
  reaches the RUNTIME counter, which sits at the per-CPU write itself, but not
  this census.
* **The kernel-root exemption is a root comparison, not a proof.** A published
  value whose root bits equal this CPU's kernel root is counted as `kernel`
  whatever its ASID field. Nothing here proves a process root can never equal
  the kernel root; it is exempted because the kernel root is the boot identity
  map and legitimately runs under ASID 0.
  claim-lint:ok: 0 experiments in this round tried to make a process root
  collide with the kernel root; the comparison is `TTBR0_ROOT_MASK` in
  `kernel/src/arch_impl/aarch64/ttbr0.rs`.
* **The counter IS reached from the exception-return corridor, and this
  document said the opposite.** The sentence that stood here — "The counter is
  not on the ERET or IRQ path; it is at the per-CPU write and at the two
  discipline helpers" — was false, and it is the sentence that licensed adding
  an atomic read-modify-write at the per-CPU write with no measurement. The
  commit message of `6bd6edf3` ("nothing added to the ERET corridor") and the
  round's fix report carry the same defect; pushed history is not rewritten, so
  the correction lives here and in the PR body. What is true is narrower: the
  `.S` files are untouched. The Rust the corridor branches to is not.
  The chain, read out of the tree: `syscall_entry.S` and `boot.S` each
  `bl check_need_resched_and_switch_arm64`; that reaches `dispatch_thread_locked`
  (which calls `set_next_ttbr0_for_thread` and `switch_ttbr0_if_needed`) and
  `dispatch_idle_locked` (which calls `setup_idle_return_locked`);
  `handle_sync_exception` reaches `set_idle_stack_for_eret`. All four publish a
  shadow word, so all four run the counter — and all four were already listed as
  publish sites in this document's own section-3 table, which is what made the
  sentence self-contradicting.
  claim-lint:ok: 4 of 4 corridor-reached publish sites named in this bullet
  appear in this document's own section-3 census table; the quoted commit
  message is `6bd6edf3`.
* **The cost is now measured, as a static instruction count, and only that.**
  R157 built the shipped production profile twice, differing only in whether the
  two `note_shadow_publish` calls are present, disassembled both and counted
  instructions per symbol: `switch_ttbr0_if_needed` 20 → 53 (+33, for its two
  publishes), `set_next_ttbr0_for_thread` 1925 → 1943 (+18),
  `setup_idle_return_locked` 255 → 294 (+39). Per publish that is a load-acquire
  of the per-CPU-initialised flag plus a per-CPU read of this CPU's kernel root,
  a four-way counter select, and an `ldxr`/`add`/`stxr`/`cbnz` retry loop — there
  is no LSE atomic on this target. This is an instruction count, NOT a cycle
  measurement: it says nothing about LL/SC retry under contention or about cache
  behaviour, and 0 cycle counts were taken in this round either.
  claim-lint:ok: 2 builds, 3 symbols, both disassemblies and the arithmetic
  are
  in `serials/asid-ratchet/08-corridor-instruction-delta.txt`
* **Three boots per profile is a small sample.** It is adequate for "the gates
  these changes touch still pass and the census reads 0 on them" and
  inadequate for a rate claim. 0 rate claims are made here, and the 6 boots are
  listed in section 8.
* **The 5-hour exposure window is quoted from the round brief, not
  re-derived.** This round did not re-measure when `9e877486` landed relative to
  PR #795.
* **Ruling R19 is not quoted**, for the reason in section 2: its text is not in
  this repository, in issue #786, or in PR #800's body.

---

# R157 review round: what the five findings changed

The sections above are the round as first written. Five review findings against
it — two blocking, three major — are closed below. Where a section above stated
something the findings showed to be false, that section has been corrected in
place rather than annotated; this part records what was done and what it cost.

## 12. ASID-01: the gate ratchet is behavioural now

The finding, reproduced at the previous head: deleting the three census
assertions from `score_serial` in the strict gate and the three `[ ... ] || {}`
assertions from the production gate — 598 and 483 characters, both scripts still
`bash -n` clean — left `both_aarch64_gates_fail_on_an_untagged_publish` green
AND made the strict gate score a serial reporting `untagged=3` as `SCORE: PASS`.
The test read three `script.contains(...)` strings and nothing anchored them to
a `return 1` or an `exit 1`.
claim-lint:ok: the mutation was reproduced at the previous head and again at
this one, on 2 of 2 gates, singly

What replaced it RUNS both gates. Each gate now has a scoring-only entry point
that skips the boot and executes its own verdict block against a named serial:
`BREENIX_STRICT_SCORE_ONLY`, which the strict gate already had (its preflight —
kernel present, no-NEON, boot_tests profile, ext2 disk — is now guarded on the
variable so the entry point needs no build), and `BREENIX_PROD_SCORE_ONLY`,
added to the production gate in the same shape. Neither guards the verdict
itself; that is the point.

Four legs run against each gate, on a serial that gate was recorded green on
(`03-strict-boot1-serial.txt`, `04-prod-boot1-serial.txt`):

| leg | serial | required verdict |
|-----|--------|------------------|
| A | as captured | PASS — anti-vacuity: a gate that rejected everything would satisfy B–D |
| B | one census line rewritten to `untagged=3` | FAIL, naming `untagged` |
| C | every census line deleted (claim-lint:ok: 13 of 13 in each baseline serial) | FAIL |
| D | every census line forced to `tagged=0` (claim-lint:ok: 13 of 13 in each baseline serial) | FAIL |

The `script.contains(variable)` check that remains is a guard, not the
assertion: without the entry point, invoking a gate from a unit test would boot
QEMU, so the test fails first instead.

Mutation-proven, at this head, one mutation at a time:

* assertions deleted from the STRICT gate only →
  `both_aarch64_gates_fail_on_an_untagged_publish` FAILED with
  "run-aarch64-boot-test-strict.sh passed a serial reporting an untagged
  process-root publish: SCORE: PASS - …-untagged.txt".
* strict restored, assertions deleted from the PRODUCTION gate only → the same
  test FAILED naming `run-aarch64-prod-profile-boot-test.sh`, whose output was
  "PASS: production profile reached bsshd …".

Both scripts were then restored.

## 13. ASID-04: the leg that actually reaches the blocking-resume class

The finding: section 5b's runtime mutation deletes normalisation from
`adopt_process_ttbr0`, and every blocking-resume site routes through
`restore_process_ttbr0`, so the untagged publishes it counts are exec/init ones.
The finding proposed deleting `let root = process_root_ttbr0(root);` from
`restore_process_ttbr0` instead.
claim-lint:ok: 5 of 5 blocking-resume helpers route through
`restore_process_ttbr0`, printed by
`every_blocking_resume_restore_uses_the_guarded_helper` in serials/asid-
ratchet/09-suite-green-after-r157.txt

**That leg was run, and it does NOT redden the gate.** Production gate exit 0,
15 census lines, final line
`[TTBR0_ASID_CENSUS:untagged=0:tagged=24767:kernel=28311:cleared=52360]`.
Evidence: `serials/asid-ratchet/10-leg-restore-normalisation-deleted.txt` and
`10-leg-restore-normalisation-deleted-serial.txt`.

The reason is structural and worth recording, because it says something about
the shape of the defence. With the normalisation gone, `root` is the caller's
untagged root; `local_ttbr0() == root` is then false against a register holding
the tagged root, so the fast arm is never taken and control falls through to
`adopt_process_ttbr0(root)` — which normalises. The blocking-resume path is
defended by the adopt path's normalisation even with its own removed.
claim-lint:ok: the mutation was booted and its gate output is serials/asid-
ratchet/10-leg-restore-normalisation-deleted.txt, 15 of 15 census lines at
untagged=0

The publish that is NOT defended that way is the fast arm's, which writes both
corridor words directly and returns. So the leg that reaches the class is:
publish the caller's RAW operand on that arm, leaving the comparison and the
adopt call on the normalised value.

```
 pub fn restore_process_ttbr0(raw: u64) {
     let root = process_root_ttbr0(raw);
     if local_ttbr0() == root {
         unsafe {
-            super::percpu::Aarch64PerCpu::set_saved_process_cr3(root);
+            super::percpu::Aarch64PerCpu::set_saved_process_cr3(raw);
```

Production gate, that build, exit 1:

```
FAIL: TTBR0 ASID census reported an untagged process-root publish:
  [TTBR0_ASID_CENSUS:untagged=104:tagged=22235:kernel=25920:cleared=47607]
Observed TTBR0 ASID census marker count: 14
Observed TTBR0 ASID census untagged-publish line count: 13
```

13 of 14 lines report a non-zero `untagged`, and the population is the right one:
the count is 9 while init is still starting and jumps 9 → 52 → 92 across the
poll/TTY/bsshd phase, which is where the blocking-resume returns are. Gate
output: `serials/asid-ratchet/11-leg-resume-fast-arm-raw-gate.txt`; serial:
`11-leg-resume-fast-arm-raw-serial.txt`. The line was restored and the kernel
rebuilt for section 16.

## 14. ASID-05: one place constructs the ASID tag

The finding: `set_next_ttbr0_for_thread` still spelled the tag as
`ttbr0 | (1u64 << 48)` — the or-only form `process_root_ttbr0`'s own doc comment
refuses, because it preserves a foreign ASID the operand already carried — and
the new publish census scored that operand `[dispatch-tagged]` and ACCEPTED it,
so the value ratchet could not tell "replaced" from "or-ed".

Three sites were changed, not one. The census that found the other two is a
scan for the tag's two spellings across `kernel/src`:

| site | was | is |
|------|-----|-----|
| `context_switch.rs::set_next_ttbr0_for_thread` | `ttbr0 \| (1u64 << 48)` | `super::ttbr0::process_root_ttbr0(ttbr0)` |
| `main_aarch64.rs` init launch | `ttbr0_phys \| (1u64 << 48)` | `ttbr0_phys` (adopt normalises) |
| `process_memory.rs::switch_to_process_page_table` (aarch64) | `root \| (flags.bits() & 0xFFFF_0000_0000_0000)` | `root` (adopt normalises) |

The first is the one that mattered: its operand is `process.inherited_cr3` on the
arm where the row has no page table, a word that came from a register rather than
from a frame address.

Two ratchets, both anti-vacuity-proven:
claim-lint:ok: 2 of 2 ratchets are proven by the mutation below, which reddens
3 of 32 tests

* `Provenance::DispatchTagged` is gone. Reaching the discipline's tag WITHOUT
  going through `process_root_ttbr0` is now `HandTagged`, which is not accounted,
  so the publish census fails on it.
  `the_publish_census_tells_a_replaced_tag_from_an_or_ed_one` runs both spellings
  of one synthetic publish and requires the census to separate them.
* `the_asid_tag_is_constructed_in_one_place` censuses every line under
  `kernel/src` that ors the tag in — naming `USER_ASID_TTBR0`, or spelling a
  shift that parses to the discipline's own tag — and requires them all to be in
  `arch_impl/aarch64/ttbr0.rs`, with an anti-vacuity assertion that the
  discipline's file does construct it.
  `the_tag_census_reads_the_or_and_not_the_comparison` pins the predicate's six
  cases, including that a comparison against the tag and a commented-out or are
  not findings.
claim-lint:ok: 3 of 3 or-only sites in the tree were found by this census and
changed; the mutation that reddens it is below

Mutation-proven: restoring `let tagged_ttbr0 = ttbr0 | (1u64 << 48);` at the
dispatch site reddens THREE tests —
`every_shadow_publish_has_an_accounted_asid`, naming
`set_next_ttbr0_for_thread set_next_cr3(tagged_ttbr0) [HAND-TAGGED]`;
`the_asid_tag_is_constructed_in_one_place`; and
`the_discipline_publishes_the_dispatch_asid`, which no longer parses a literal
out of the dispatch body but requires the published value to be bound through
`process_root_ttbr0(`.

Disclosed narrowing, and it is why the third site was CHANGED rather than left
for the predicate: the tag census reads the constant and the shift. It does not
reach a hand-managed ASID field spelled with the `0xFFFF_0000_0000_0000` mask,
because that literal is also this kernel's HHDM base and appears on a dozen
unrelated lines under `kernel/src`. `switch_to_process_page_table` was the one
site with that spelling; removing it is not the same as being able to catch the
next one.

## 15. ASID-03: the completeness premise is enforced

The finding: `SHADOW_WRITERS` is keyed on two function names, the comment above
it claims "a new publish site is reached because it has to call one of them",
and nothing pinned that. A third function calling `percpu_write_u64` with either
offset would evade the structural census (keyed on the names) AND the runtime
counter (living inside them). That `percpu_write_u64` is module-private is a fact
about today's tree, not a ratchet — the module it is private to is the one that
would host the third writer.
claim-lint:ok: 0 of 32 tests at the previous head counted writers of either
offset; the mutation that reddens the new one is below

`each_corridor_shadow_word_has_exactly_one_writer` censuses, for each of
`PERCPU_SAVED_PROCESS_CR3_OFFSET` and `PERCPU_NEXT_CR3_OFFSET`, every function
under `kernel/src` whose comment-stripped body names it, and requires: exactly
one writes it through `percpu_write_u64`; that one is in `SHADOW_WRITERS`; and
every other function naming it reads it through `percpu_read_u64`, so a function
that names the offset while doing neither is a failure rather than a silence.
claim-lint:ok: 2 of 2 offsets are censused, each reaching 2 of 2 functions at
this head, named in the failure message of the mutation below

Anti-vacuity: `the_writer_census_catches_a_second_writer` counts 1 writer in a
synthetic source and 2 when a second is appended. And on the real tree, adding

```rust
pub unsafe fn arm_the_corridor(val: u64) {
    percpu_write_u64(PERCPU_NEXT_CR3_OFFSET, val);
}
```

to `percpu.rs` failed the test, naming both writers and all three functions.
claim-lint:ok: the mutation named 2 writers and 3 of 3 functions; the failure
message is quoted from that run

Disclosed narrowings: the census counts functions that NAME the constants, so a
write that computed the offset arithmetically or went through a raw pointer is
outside it, as are the two assembly stores accounted separately. Files under
`arch_impl/x86_64/` are skipped: that module defines constants with the same two
names, a different block's offsets reached only from x86_64 code, and counting
them would make this census fail on a namesake. The walk is otherwise
kernel-wide rather than confined to `arch_impl/aarch64/`, because the aarch64
constants are `pub`.

## 16. Builds, gates and suites for this round

Every aarch64 build is the mandated invocation:
claim-lint:ok: 2 of 2 aarch64 builds in this round used this invocation, one
per profile

```
cargo build --release --target aarch64-breenix-kernel.json \
  -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64            # production profile
  … --features boot_tests …                 # strict-gate profile
```

followed by `scripts/check-kernel-no-neon.sh`, which passed on both profiles
(0 FP/SIMD load/store instructions in `.text`).

Host-load rule: `pgrep -fl qemu-system-aarch64 | wc -l` was read before each gate
invocation and returned 0 every time; one boot ran at a time. Six gate
invocations booted QEMU in this round: two mutation legs and two gates run
twice, once for the code change and once at the head that carries the
documentation corrections. The one invocation without its own fresh check was
the immediate `--rebuild-userspace` retry of a command whose first attempt had
just been checked and had exited before booting, on a missing ext2 disk.
claim-lint:ok: 6 of 6 booting gate invocations in this round are listed in the
table below or in sections 13 and 12

| run | result | evidence |
|-----|--------|----------|
| structure suite, 32 tests | 32 passed | `serials/asid-ratchet/09-suite-green-after-r157.txt` |
| production gate ×1 | PASS, `untagged=0:tagged=24924:kernel=28996:cleared=53224`, 15 census lines | `12-prod-boot-r157.txt`, `12-prod-boot-r157-serial.txt` |
| strict gate ×1 | PASS 1/1, 14 census lines, last `untagged=0:tagged=17349` | `13-strict-boot-r157.txt`, `13-strict-boot-r157-serial.txt` |

The publish census at this head is 17 calls, unchanged in count; the one that
moved is `set_next_ttbr0_for_thread set_next_cr3(tagged_ttbr0)`, now
`[normalised]` where it read `[dispatch-tagged]`. The four accounted
provenances — cleared, normalised, kernel root, read back — are each still
present in the tree, which is what `every_shadow_publish_has_an_accounted_asid`
requires so that "no unaccounted publishes" is a statement about a classifier
that fires.

## 17. What is still NOT claimed, after this round

* **No boot samples `ttbr0_el1` at an exception return.** Unchanged from
  section 11: the counter reads what is PUBLISHED, not the regime EL0 ran under.
* **The cost measurement in section 11 is an instruction count.** No cycle count
  was taken, on any path, in this round either. The <1000-cycle budget comparison
  is arithmetic over instructions, not a timing result.
* **One boot per profile at the shipping head.** Two gates, one boot each, plus
  two booted mutation legs and one earlier pass of both gates before the
  documentation corrections. Adequate for "these changes leave both gates
  passing and the census reading 0"; 0 rate claims are made.
* **The tag census cannot see a mask-spelled ASID field.** Stated in section 14
  with the reason. The one site in the tree with that spelling was removed, which
  is not the same as the predicate reaching the next one.
* **`each_corridor_shadow_word_has_exactly_one_writer` counts named offsets.**
  A raw-pointer write, or an arithmetically computed offset, is outside it.
* **The ASID-04 leg proves the counter SEES a blocking-resume untagged publish.**
  It does not prove the shipped kernel's blocking-resume publishes are tagged for
  the reason the discipline gives; what it proves is that if they were not, this
  gate would be red.
  claim-lint:ok: the leg is a booted mutation whose gate output is
  serials/asid-ratchet/11-leg-resume-fast-arm-raw-gate.txt, 13 of 14 census
  lines above 0.
* **Section 13's first leg is a null result, not a repair.** Deleting
  normalisation from `restore_process_ttbr0` leaves the gate green because
  `adopt_process_ttbr0` re-normalises on the fall-through. That is an observation
  about this tree, and no claim is made that the fall-through is a designed
  defence.

## 18. claim-lint, R157 review round

Run at the head this branch now carries, after the annotations this round added:

```
claim-lint: scripts/claim-lint.py -> exit 0
claim-lint: scripts/claim-lint.py --files docs/planning/green-program/aarch64-testing/TTBR0-ASID-RATCHET-2026-09-05.md docs/planning/green-program/aarch64-testing/serials/asid-ratchet/README.md -> exit 0
```

The first run reported 28 findings across this round's changed hunks; each was
discharged with a same-paragraph `claim-lint:ok:` citation naming an N-of-M
count or a captured artifact, and 0 were discharged by weakening a claim the
round measures. The 228 pre-existing findings outside this branch's changed
hunks are unchanged and unreported, as they were before this round.
claim-lint:ok: 28 of 28 findings in the changed hunks are discharged by
citation; the tool's own scope statement is in
docs/planning/green-program/claim-linting.md
