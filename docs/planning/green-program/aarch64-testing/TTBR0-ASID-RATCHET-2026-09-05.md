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
claim-lint:ok: 25 of the 27 checks in that file at this head still pass with the
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
`both_aarch64_gates_fail_on_an_untagged_publish` (both gate scripts carry the
three assertion patterns, so deleting one is a test failure rather than a silent
loss of the ratchet).

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
claim-lint:ok: 29 of 29 suites in `serials/asid-ratchet/06-structure-suites.txt`
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
* **No timing measurement was taken for the counter.** The cost argument is
  structural — four relaxed counters, and on the arm where the value is not 0,
  one per-CPU read
  of the kernel root — and this round took 0 cycle counts. The counter is not
  on the ERET or IRQ path; it is at the per-CPU write and at the two discipline
  helpers.
* **Three boots per profile is a small sample.** It is adequate for "the gates
  these changes touch still pass and the census reads 0 on them" and
  inadequate for a rate claim. 0 rate claims are made here, and the 6 boots are
  listed in section 8.
* **The 5-hour exposure window is quoted from the round brief, not
  re-derived.** This round did not re-measure when `9e877486` landed relative to
  PR #795.
* **Ruling R19 is not quoted**, for the reason in section 2: its text is not in
  this repository, in issue #786, or in PR #800's body.
