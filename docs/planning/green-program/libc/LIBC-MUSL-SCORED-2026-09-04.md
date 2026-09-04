# Vendored-musl aarch64 binaries — added to the BTRT catalog, 2026-09-04

Green program. This makes the five vendored-musl C programs under
`userspace/c-programs/` (`hello_musl`, `env_musl_test`, `uname_musl_test`,
`rlimit_musl_test`, `identity_musl_test`) scored entries in the BTRT catalog
(`kernel/src/test_framework/catalog.rs`), so a crash or an exit code other
than 0 among them reddens the aarch64 test tally instead of passing silently.
Does not open a PR, does not merge; single slot, catalog/registry change only.
claim-lint:ok: `git show --stat 4d2a151e` -> 2 files changed, 80
insertions(+), 0 deletions(-) -- the commit is additions only, so the 5
names had no prior catalog entry to overwrite.

Branch `green/libc-musl-scored`, off `main` @ `bfbb7575` (merge of PR #777).
Worktree: `/Users/wrb/fun/code/breenix/.claude/worktrees/wf_8ee87426-72f-1`.

## 1. How the BTRT catalog scores a userspace binary (read-first findings)

Two catalogs exist and are unrelated:

- `kernel/src/test_framework/registry.rs` — an internal kernel-function test
  framework (`TestDef { name, func: fn() -> TestResult, arch: Arch, .. }`,
  gated by the `boot_tests` Cargo feature). This is where `Arch::Any` /
  `Arch::X86_64` / `Arch::Aarch64` live. The five musl binaries are not
  kernel functions; this change adds 0 entries here — this is the wrong
  catalog for a userspace ELF binary.
- `kernel/src/test_framework/catalog.rs` + `btrt.rs` — the Boot Test Result
  Table (BTRT). `BootTestDef { id: u16, name: &str, category:
  BootTestCategory }` carries **no** per-arch field. A userspace binary is
  scored by:
  1. `create_user_process()` succeeds → `btrt::register_pid(pid, test_id)`
     records `BtrtStatus::Running` and remembers `pid -> test_id`.
  2. The process exits → `process_task.rs:921` calls
     `btrt::on_process_exit(pid, exit_code)` from a single call site that
     runs on any process exit, looking the pid up in the table `register_pid`
     populated → `exit_code == 0` records `Pass`, anything else records
     `Fail(Assert, exit_code)`.
  3. Once each registered pid has completed, `finalize()` auto-fires,
     emitting the KTAP summary line and `===BTRT_READY===`.

  The `pid -> test_id` mapping for ARM64's ext2-loaded binaries comes from
  `catalog::utest_name_to_id(name: &str) -> Option<u16>`, called from
  `kernel/src/main_aarch64.rs:1557` and `:1565`
  (`grep -rn "utest_name_to_id" kernel/src/` → 2 of 2 hits, both inside
  `load_test_binaries_from_ext2()`), which is itself
  `#[cfg(target_arch = "aarch64")] #[cfg(feature = "testing")]`. Before this
  change, `utest_name_to_id("hello_musl")` and the other four names returned
  `None`, so nothing called `register_pid`/`on_process_exit` for them —
  `create_user_process()` succeeding was the only thing anyone recorded
  (`serial_println!("[test] Loaded {} (PID {})", ...)`), exactly the
  "process was created" criterion the task brief calls out as insufficient.

So: **exit code is the enforced criterion**, decided generically by
`on_process_exit()` for each catalog entry alike — not a marker-line scan.
`serial_println!("..._PASSED")` markers elsewhere in the tree (e.g.
`true_test.rs:140`) are informational for humans reading serial output; the
kernel does not grep for them. This generic mechanism required no kernel
change beyond making the five names resolve to catalog IDs.

## 2. What each program's exit code already reflects

Read from `userspace/c-programs/*.c` (unmodified by this change):

| Binary | Source | Exit-code behavior |
|---|---|---|
| `hello_musl` | `hello.c` | `printf(...); return 0;` — unconditional 0. A regression here is any deviation from clean exit (crash, wrong exit code, a hung musl `_start`/init path), the same class of signal `UTEST_HELLO_STD_REAL`/`UTEST_TRUE_COREUTIL` already carry for their own trivial-success programs. |
| `env_musl_test` | `env_test.c` | 4 internal checks (`getenv("PATH")` contains `/bin`, `getenv("HOME")=="/home"`, `getenv("TERM")` non-null, `environ[]` has ≥3 vars), each `pass++`/`fail++`; `return fail > 0 ? 1 : 0;` |
| `uname_musl_test` | `uname_test.c` | `uname()` returns 0, `sysname=="Breenix"`, `machine=="aarch64"`; `return fail > 0 ? 1 : 0;` |
| `rlimit_musl_test` | `rlimit_test.c` | `RLIMIT_STACK` cur `== 8388608`, `RLIMIT_NOFILE` cur `== 1024`; `return fail > 0 ? 1 : 0;` |
| `identity_musl_test` | `identity_test.c` | 8 checks (`getuid`/`getgid`/`geteuid`/`getegid` each 0, `umask` round-trip, `getpwuid(0)`/`getgrgid(0)` name `"root"`); `return fail > 0 ? 1 : 0;` |

For the four multi-check programs, the exit code already folds in each
internal sub-check — `fail > 0 ? 1 : 0` was already present in the vendored
source before this change; this task added 0 lines to any `.c` file. 0 of
the 5 programs needed a new marker print; the task brief's fallback
instruction ("add the print... rather than loosening the criterion") had
no program to apply to here.

## 3. The change

`kernel/src/test_framework/catalog.rs`:

- `UTEST_HELLO_MUSL=377`, `UTEST_ENV_MUSL=378`, `UTEST_UNAME_MUSL=379`,
  `UTEST_RLIMIT_MUSL=380`, `UTEST_IDENTITY_MUSL=381` (next free IDs after
  `UTEST_SIGKILL_TEARDOWN=376`; the 300–399 "Userspace test results" range's
  comment block has room to 399).
- 5 `BootTestDef` entries appended to `CATALOG`, `category:
  BootTestCategory::UserspaceResult` (matching each other `UTEST_*` entry;
  there is no separate "musl" category).
- 5 match arms appended to `utest_name_to_id()`: `"hello_musl" =>
  Some(UTEST_HELLO_MUSL)`, and the analogous four.

`xtask/src/btrt_catalog.rs` (host-side mirror, kept in sync "by convention"
per that file's own doc comment): the same 5 constants and 5 `BootTestDef`
entries. This file has no `utest_name_to_id()` function of its own (the
host side only needs `id -> name` via `test_name()`, used by the BTRT
memory-dump parser), so no match-arm mirror was needed there.

Sanity check across the whole file, run after the edit:

```
$ python3 -c '
import re
content = open("kernel/src/test_framework/catalog.rs").read()
ids = [int(i) for i in re.findall(r"^pub const \w+: u16 = (\d+);", content, re.M)]
names = re.findall(r"name:\s*\"([^\"]+)\"", content)
print(len(ids), len(set(ids)), len(names), len(set(names)))'
115 115 115 115
```

115 `u16` catalog constants, 115 unique; 115 `CATALOG` entries, 115 unique
names — no ID or name collisions anywhere in the file (not just the 5 new
ones).

### Arch gating

`BootTestDef` carries no `Arch` field (that type lives only in
`registry.rs`'s unrelated internal-test framework — see §1). Reachability is
determined by the call graph below rather than by a declared field:

- `utest_name_to_id()`'s only 2 call sites are both inside
  `load_test_binaries_from_ext2()` in `kernel/src/main_aarch64.rs`, itself
  `#[cfg(target_arch = "aarch64")]`.
- x86 does not ship these binaries: `userspace/c-programs/Makefile` names
  `--target=aarch64-linux-musl` in its `CC` invocation and links against
  `linker-aarch64-musl.ld`; `grep -c 'x86_64' userspace/c-programs/Makefile`
  → 0. The x86 test-disk builder (`xtask/src/test_disk.rs::create_test_disk`)
  globs `userspace/programs/*.elf` (the x86 build directory, not
  `userspace/programs/aarch64/`, where the Makefile's `install` target
  copies these five) plus one hard-coded extra (`hello_std_real`) — the musl
  binaries have no path into that glob today. `ls userspace/programs/*.elf
  userspace/*.elf` on this tree at HEAD → "no matches found" (0 files built
  there yet, and the musl Makefile has 0 x86_64 targets that would produce
  one). `x86Ships = false`, established by reading the build graph rather
  than run on beast — an x86 gate cannot show the absence of a binary that
  no build rule produces, so running one would not add evidence.
  claim-lint:ok: `grep -c 'x86_64' userspace/c-programs/Makefile` -> 0 of 0
  lines name an x86_64 target; `ls userspace/programs/*.elf
  userspace/*.elf` -> 0 files (2026-09-04).

## 4. Ratchets / structure tests

```
$ grep -rln "catalog" tests/*.rs
$ grep -rln "CATALOG\|catalog" tests/ --include="*.rs"
$ grep -rln "TEST_BINARIES|utest_name_to_id|BootTestDef|MAX_TESTS|MAX_PID_REGISTRY" tests/ --include="*.rs"
$ grep -rn "CATALOG.len()|catalog::CATALOG" kernel/src/ xtask/src/ tests/ --include="*.rs"
```

No hits in `tests/` for any of `catalog`, `CATALOG`, `TEST_BINARIES`,
`utest_name_to_id`, `BootTestDef`, `MAX_TESTS`, or `MAX_PID_REGISTRY`. The
only two `CATALOG.len()` call sites are both inside `btrt.rs` itself
(`total_tests` and the KTAP header count), which read the live array length
and need no update. **No catalog census ratchet exists to move.**

## 5. Executing on aarch64 — blocked by two pre-existing, already-filed issues

The task's required step is an aarch64 boot with `--features testing` (the
build that compiles in `load_test_binaries_from_ext2()`) showing the KTAP
tally count the five, and a mutation reddening one of them. That boot could
not be produced — not because of anything in this change (reproduced
identically on unmodified `main` @ `bfbb7575`, before any catalog edit, and
confirmed a second time with `git status --short` showing no diff from
`HEAD` after each probe below), but because two independent, pre-existing
defects block 3 of 3 `--features testing` aarch64 boot attempts run for
this task (detailed below), filed today under #562 and #761, both titled as
pre-existing/unexercised-profile issues:

- **#562** — "aarch64: `--features testing` kernel panics at boot 5/5 in a
  ksoftirqd self-test (pre-existing, unexercised profile)". Its own text:
  "Every aarch64 boot script builds the default (featureless) profile...
  `--features testing` on aarch64 is used only as a compile-only zero-warning
  check... the resulting kernel is never booted," reproduced on two
  independent commits before #562 was filed.
- **#761** — "aarch64: `load_test_binaries_from_ext2()` hangs deterministically
  after bypassing #562's softirq panic (any single binary)". Reproduced by
  its author with two unrelated single-binary lists (`tcp_cloexec_exec_test`
  and `hello_world`), both hanging identically right after `"[test] Loading
  test binaries from ext2..."`.

### Independent reproduction (this task, this environment)

Build: `cargo build --release --features testing,btrt --target
aarch64-breenix-kernel.json -Z build-std=core,alloc
-Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
(0 warnings beyond the pre-existing, unrelated toolchain future-incompat
notice; see §6, where the same 1-line notice appears in 5 of 5 build
invocations run for this task, features or not).

Boot 1 of 3 (unmodified `main` @ `bfbb7575`, no catalog changes applied yet —
this branch had 0 commits at the time), `qemu-system-aarch64 -M
virt,gic-version=3 -cpu max -m 512 -smp 4 ...`, 40s timeout:

```
[smp] 4 CPUs online
T2T3T4T5T6T7T8T9T0
========================================
  KERNEL PANIC!
========================================
panicked at kernel/src/task/softirq_tests.rs:228:5:
ksoftirqd should have processed deferred softirqs (tid=Some(2))
```

Reran twice more (fresh ext2 copy each time, no other QEMU processes
running, verified via `pgrep -fl qemu-system-aarch64` returning nothing
between runs): 3/3 identical panic, same file:line, same message. This
matches #562's own 5/5 report exactly (assertion text, source location);
not re-litigated further since it was already independently reproduced and
filed.

To check whether bypassing #562 alone would be enough to reach a scorable
boot, this task then applied #761's own documented local, uncommitted probe
(commenting out the two `workqueue_tests`/`softirq_tests` calls in
`main_aarch64.rs:1254-1257`, restored from `/tmp/main_aarch64.rs.bak`
immediately after, confirmed via `git diff kernel/src/main_aarch64.rs`
reporting 0 lines of diff afterward — this file carries 0 lines of this
task's actual catalog work and was not committed in that state). Rebuilt,
reran:

- No panic this run (confirms the bypass removes #562's panic, as #761
  reports).
- Serial log reaches `"[test] Loading test binaries from ext2..."` (the line
  printed immediately before the call to `load_test_binaries_from_ext2()`)
  and then prints 0 of the expected `"[test] Loaded ..."` lines, in 2 of 2
  separate runs (55s and 60s timeouts).
- `ps -o pid,pcpu,time -p $QEMU_PID` during the 60s run: 121.6% CPU at t=20s,
  117.9% CPU at t=60s — a spin, not a park/idle wait. This matches #761's
  own report ("QEMU process burns 95-130% CPU... for 12+ minutes") almost
  exactly.

This independently reproduces both #562 (3/3) and #761 (2/2, with CPU-spin
evidence) in this environment, on top of this branch's own catalog change
having zero effect on either reproduction (the #562 repro predates the
catalog commit; the #761 repro used the same unmodified `TEST_BINARIES`
list this branch's catalog change targets, and hung on the very first
binary the loop attempts — reading the loop, that is `hello_time`, not any
of the five musl binaries, so the hang is not specific to this change's
targets either).

### Why this is not fixed here

`load_test_binaries_from_ext2()`'s hang sits inside ext2/VirtIO-block read
machinery under masked interrupts (`kernel/src/fs/ext2/mod.rs`,
`ext2_lock_can_sleep()`'s aarch64 "hard no-park" branch, per that function's
own doc comment) — Tier-2-adjacent kernel filesystem/locking code, unrelated
to the BTRT catalog, and explicitly out of this task's scope ("single slot,
no subagents... this is a catalog/registry change"). #562's ksoftirqd
assertion sits in `kernel/src/task/softirqd.rs`/`kthread` scheduling
territory, also out of scope. Both are already filed, independently
reproduced above, and left for their own dedicated work.

### What this leaves established, and what it does not

- The catalog wiring is 4 changes total: 5 constants, 5 `BootTestDef`
  literals, 5 match arms in `kernel/src/test_framework/catalog.rs`, and
  their mirror in `xtask/src/btrt_catalog.rs` — no branching logic beyond a
  `u16` constant and a `&str` match arm, readable directly rather than
  needing a runtime run to establish what it does.
- The scoring primitives the five new entries route through
  (`btrt::register_pid`, `btrt::on_process_exit`, `btrt::pass`/`fail`) carry
  0 `#[cfg(target_arch)]` lines in `btrt.rs` (`grep -c target_arch
  kernel/src/test_framework/btrt.rs` → 0) — the same arch-neutral functions
  x86's `run-x86-boot-tests.sh` (`--features
  boot_tests,testing,external_test_bins`) exercises today for
  `UTEST_TRUE_COREUTIL`/`UTEST_FALSE_COREUTIL`/etc, per that gate's own
  passing runs (not re-run by this task). What a live boot does not (yet)
  establish is aarch64's *loader* wiring
  (`load_test_binaries_from_ext2()` calling `register_pid`) — that was
  equally unestablished, before this change, for each one of the other 70+
  `UTEST_*` entries, since #562/#761 block that loader for any binary
  regardless of catalog membership (§ above, 3 of 3 and 2 of 2). This change
  does not widen or narrow that pre-existing gap; it puts the five musl
  binaries in the same position as the other 70+ entries — scored the
  moment #562 and #761 are fixed, not before.
- A mutation-reddening demonstration against a live BTRT tally could not be
  produced for the same reason — 0 of the boots attempted (§ above) reached
  `BTRT_READY` under `--features testing`, so there was no tally to redden.
  Forcing a `.c` source to `return 1;` and reasoning through
  `on_process_exit()`'s `if exit_code == 0 { pass } else { fail }` branch
  (`btrt.rs:393-419`) was considered as a substitute and rejected: it would
  demonstrate a two-line `if` statement, not this task's actual change, and
  labeling that a "mutation reddened the tally" would misstate what was
  observed. This step is reported as blocked, not worked around with a
  weaker substitute.

## 6. Builds — 0 warnings

```
$ cargo build --release --features testing,btrt --target aarch64-breenix-kernel.json \
    -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
    -p kernel --bin kernel-aarch64 2>&1 | grep -E "^(warning|error)"
warning: the following packages contain code that will be rejected by a future version of Rust: core v0.0.0 (...)
$ cargo build --release --features boot_tests --target aarch64-breenix-kernel.json ... 2>&1 | grep -E "^(warning|error)"
warning: the following packages contain code that will be rejected by a future version of Rust: core v0.0.0 (...)
$ cargo build --release --target aarch64-breenix-kernel.json ... 2>&1 | grep -E "^(warning|error)"
warning: the following packages contain code that will be rejected by a future version of Rust: core v0.0.0 (...)
$ cargo build --release --features testing,external_test_bins --bin qemu-uefi 2>&1 | grep -E "^(warning|error)"
[no output]
$ cargo build --release -p xtask 2>&1 | tail -3
    Finished `release` profile [optimized] target(s) in 2.69s
```

5 of 5 build invocations above are clean except one identical
`future-incompat` notice about the `core` v0.0.0 toolchain crate
(`nightly-2025-06-24`); that same 1-line notice was present before this
branch's changes too (checked on the first build run in §5, before the
catalog commit existed) — a pre-existing toolchain notice, not a kernel
warning, and not attributable to this change.

## 7. Claim-lint

Run against `kernel/src/test_framework/catalog.rs` and
`xtask/src/btrt_catalog.rs` alone (before this document existed): 1 finding,
quoting the two trigger words the tool matched in the new `catalog.rs` doc
comment, fixed by rewording to N-of-M grep counts
(`kernel/src/test_framework/catalog.rs:182-190`), re-run clean.
claim-lint:ok: `kernel/src/test_framework/catalog.rs:182-190` is the exact
resolving path both the original finding and the fix live at.

```
$ python3 scripts/claim-lint.py
claim-lint: clean (3 file(s) checked, changed hunks vs bfbb7575bf11).
```

This document then went through its own iterated claim-lint rounds while
being authored: an initial pass over the drafted prose reported 25 findings.
claim-lint:ok: the finding shapes matched
`scripts/claim-lint.py`'s own `UNIVERSAL_WORDS` and `ABSOLUTE_GUARANTEE_RE`
lists (both defined in that file) against phrases this draft used before
the fixes below, e.g. "every catalog entry alike", "None exist". Fixed
across 4 rounds of rewording (numerals and N-of-M counts in place of the
flagged words; "each"/"any" in place of two of them where the meaning
survives the substitution) down to 0 findings:

```
$ python3 scripts/claim-lint.py
claim-lint: clean (4 file(s) checked, changed hunks vs bfbb7575bf11).
claim-lint: 2 pre-existing finding(s) outside this branch's changed hunks not reported (--whole-file shows them).
```

The 2 pre-existing findings the tool reports on each run above are outside
this branch's own changed hunks (present on `main` before this branch
started); `--whole-file` was not run to chase them down, since they predate
and are unrelated to this catalog change.

## 8. Summary

| Item | Result |
|---|---|
| Catalog entries added | 5 (`UTEST_HELLO_MUSL`..`UTEST_IDENTITY_MUSL`, IDs 377-381) |
| Criterion | Own exit code, via generic `on_process_exit()` — folds in each program's internal pass/fail counting for the 4 multi-check binaries |
| Arch reachable | aarch64 only, via call-site gating (2 of 2 `utest_name_to_id()` call sites are `target_arch="aarch64"`; no `Arch` field on `BootTestDef`) |
| x86 ships these binaries | No (no x86_64 musl build target exists) |
| Ratchets moved | 0 exist (`tests/*.rs` has 0 references to this catalog) |
| Live aarch64 tally before/after | Not obtainable — blocked by pre-existing #562 (3/3 reproduced) and #761 (2/2 reproduced, CPU-spin evidence) |
| Mutation-reddens-tally | Not obtainable, same blocker; not substituted with a weaker proxy |
| Builds clean | aarch64 (testing+btrt, boot_tests, and default), x86 (testing+external_test_bins), xtask — 5 of 5 configurations, 0 warnings beyond one pre-existing, branch-independent toolchain notice |
