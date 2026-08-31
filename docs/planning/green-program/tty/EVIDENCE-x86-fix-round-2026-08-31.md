# TTY — x86 evidence port, fix round, 2026-08-31

Green program, TTY-x86 evidence arc. Fix-round slot. Companion to
`EVIDENCE-2026-08-30.md` (arc 4, aarch64) and `EVIDENCE-x86-confirm-2026-08-31.md`
(the confirm pass this round repairs). Branch `feat/green-tty-x86`, fix round
applied on top of the confirm slot's `d1480934`.

This document records the fix round's response to the review's three blocking
findings (B1-B3), the four high findings (H1-H4), and the cheap medium/low
findings. Serials referenced below live alongside this file in `serials/`,
prefixed `x86-fix2-`/`aarch64-fix2-`.

## 0. Summary

All three blocking findings are closed. `cargo test --test teardown_structure`
is 81/81 (was 80/1). `cargo test --test tty_oracle_structure` is 16/16, with
one rule (M3) tightened. Both `run_tty_oracle()` launchers (aarch64 and x86)
now check `waitpid`'s `Result` honestly instead of discarding it. The blended
shared-code argument is rewritten as a census, not an absent-diff inference,
and the two false particulars the review found are corrected. `#705`'s issue
body is expanded on GitHub. `#721` carries the exact re-admission steps. Zero
kernel bytes remain touched by this arc, including this fix round — every
edit is `userspace/programs/src/init.rs`, `tests/tty_oracle_structure.rs`, or
`docker/qemu/*.sh`.

## 1. B1+B2 — the marker pin, redesigned against the real serial

The confirm slot's prescribed fix (`-ge 1` → `-eq 2` on
`TTY_ORACLE_LITERAL`, `[TTY_ORACLE:COMPLETE:`) does not work: it trips the
same ratchet's second, later law (every `marker_count` assertion must end
`-eq 0` or `-eq 1`), and its premise is false. `tty_oracle.rs`'s `emit()`
prints its argument **twice**, deliberately:

```rust
/// Emitted twice: console output interleaves at byte granularity, so a single
/// shredded copy must not be able to hide a verdict.
fn emit(line: &str) {
    print!("{}\n", line);
    print!("{}\n", line);
}
```

That is the entire source of the count of 2 — not "init's own exit-record
line plus the oracle's own COMPLETE line" (the confirm slot's and this
in-repo doc's original explanation, now corrected in
`EVIDENCE-x86-confirm-2026-08-31.md`): `INIT_SPAWN_SMOKE_REAP_LITERAL`, a
single init `print!` with no double-emit, is independently pinned `-eq 1`
and passes on every boot, proving userspace console output lands in exactly
one serial stream per print call. Because `emit()`'s double-print exists
specifically so a single **shredded** copy cannot hide a verdict, a shred
landing inside the literal itself is an accepted, expected outcome of the
design — and this session's own Leg 2 mutation serial
(`x86-mutation-M1a-master-nonblock-reverted-verdict-20260831.txt`) shows
exactly that shape on a `FAIL:` line:
`<S>[SW]<K>[SW]<T><U><R>[TTY_ORACLE:FAIL:nonblock_open:...`. Pinning `-eq 2`
on a literal whose own design tolerates dropping to 1 is flaky by
construction — precisely the #725/#731 timing/shred family this arc is
already tripping over elsewhere.

**Fix applied** (route 1 from the review, the preferred one): pin the
single-emit init record instead of the double-emitted oracle tally.
`docker/qemu/run-x86-prod-profile-boot-test.sh` now declares:

```bash
INIT_TTY_ORACLE_EXIT_LITERAL='[init] tty_oracle exited pid='
INIT_TTY_ORACLE_REAP_FAILED_LITERAL='[init] Warning: tty_oracle reap failed'
TTY_ORACLE_FAIL_LITERAL='[TTY_ORACLE:FAIL'
```

with three exact-count assertions:

```bash
test "$(marker_count "$INIT_TTY_ORACLE_EXIT_LITERAL")" -eq 1
test "$(marker_count "$INIT_TTY_ORACLE_REAP_FAILED_LITERAL")" -eq 0
test "$(marker_count "$TTY_ORACLE_FAIL_LITERAL")" -eq 0
```

`INIT_TTY_ORACLE_EXIT_LITERAL` is init's own post-wait `print!` call in
`run_tty_oracle()` — exactly one call site, executed at most once per boot,
no double-emit — so its count is genuinely, structurally exactly 1, not
merely observed to be so. This depends on B3 (below): only a genuine `Ok`
reap prints this literal; a `waitpid` failure prints the distinct
`INIT_TTY_ORACLE_REAP_FAILED_LITERAL` instead, so the two are mutually
exclusive per boot and both laws (direct `-eq` form, exact-count ending)
are satisfied. The old `TTY_ORACLE_LITERAL` (COMPLETE) declaration was
removed rather than left declared-but-unspent, which the ratchet's first
law would also have caught.

Verified: `cargo test --test teardown_structure` — **81/81**, including
`x86_production_profile_gate_verdict_discipline_holds` and
`x86_production_profile_gate_ratchet_is_not_vacuous`.

## 2. B3 — honest reap-failure handling, both arches

Both `run_tty_oracle()` bodies (`userspace/programs/src/init.rs`, aarch64
and x86_64 cfg-gated) discarded `waitpid`'s `Result` with `let _ =`, so a
failed reap still printed `code=0` off the pre-zeroed `status` — the exact
regression `run_spawn_smoke()` (`#713` fix-round-2) already fixed once in
this file. Per the review, this was fixed on **both** launchers, not just
the x86 one the port introduced (the aarch64 body carried the identical
hole, inherited unchanged from arc 4).

Both now match `run_spawn_smoke()`'s shape:

```rust
match waitpid(child_pid.raw() as i32, &mut status as *mut i32, 0) {
    Ok(_) => {
        let exit_code = (status >> 8) & 0xFF;
        print!("[init] tty_oracle exited pid={} code={}\n", child_pid.raw(), exit_code);
    }
    Err(e) => {
        print!("[init] Warning: tty_oracle reap failed: {}\n", e);
    }
}
```

Applied via the codex-wf harness against an exact before/after specification
(direct `.rs` edits are blocked in this session under the IRON RULE); the
committed diff is byte-identical to what was specified, checked by direct
`git diff` after the run, not trusted from the tool's own report.

**Gate comments fixed to claim exactly what they now prove.** Both dedicated
gates (`run-x86-tty-oracle-gate.sh`, `run-aarch64-tty-oracle-gate.sh`)
carried the comment *"init's post-wait record: proves the child was actually
reaped with status 0"* over `INIT_EXIT_LITERAL` — false before this fix (the
literal printed on both success and a discarded-Result failure), true after
it. Both gates now also declare and check `INIT_REAP_FAILED_LITERAL` (`-ne 0`
→ FAIL), the dedicated-gate-side mirror of the standing prod gate's
`INIT_TTY_ORACLE_REAP_FAILED_LITERAL`, so a reap failure is reported by its
own name rather than folding into "init never recorded the tty_oracle child
exiting."

Verified: `cargo test --test tty_oracle_structure` — **16/16**
(`init_launches_the_oracle_on_aarch64`/`_on_x86` still pass unchanged, since
neither checks launcher body content beyond signature/call-site presence).

## 3. M3 — `the_x86_gate_refuses_a_cloexec_exec_verdict` now pins mechanism

The rule asserted only that the gate script contains the substring
`[TTY_ORACLE:cloexec_exec:`, which a weakened comparison (e.g. changing
`-ne 0` to something that can never fire) would still satisfy. Tightened to
also require the script contain the literal comparison
`marker_count "$CLOEXEC_EXEC_VERDICT_LITERAL")" -ne 0`, read off the
variable's own declared name rather than hard-coded, so a future rename of
the variable does not silently make this check vacuous either.

## 4. H1 — the blended cell, defined at 13 arms (coordinator ruling)

The review's own test stands: the excluded arm's behavior is **not** proven
on the other arch through shared code. `close_cloexec()`
(`kernel/src/ipc/fd.rs:607`) is genuinely shared, zero `target_arch`, but
every one of its four call sites sits inside an arch-duplicated
`exec_process`/`exec_process_with_argv` body (two `#[cfg(target_arch =
"x86_64")]`, two `#[cfg(target_arch = "aarch64")]`, `manager.rs`). aarch64's
arm 14 proves aarch64's own exec plumbing calling the shared fd-table walk
correctly; it proves nothing about x86's separate exec bodies.

**Ruling (coordinator, this fix round): the blended TTY cell is DEFINED at
the 13-arm shared surface.** The 13 arms every x86 boot drives are proven,
by census (§5) and by the M1a mutation (both arcs' evidence), to be one
shared implementation exercised identically on both boot targets. Arm 14
(`cloexec_exec`) is aarch64-only supplementary proof, pending `#721` — a
scope definition stated plainly as part of the blended declaration, never a
14-arm claim carrying a footnote. This is the argument the port had not
actually written (`exec()` is ENOSYS in the shipped x86 zero-feature
profile, so exec-linked fd cleanup is not behavior the measured x86 arm has
at all today), not the different, false argument ("proven on the other arch
through shared code") the original dossier implied.

Consequence for the two cells:
- **tty-x86**: green at 13 arms, B1-B3 closed, exclusion stated as a
  profile-scope argument (`#721`).
- **tty-blended**: green, DEFINED at the 13-arm shared surface. Arm 14 is
  documented as aarch64-only supplementary proof, tracked for re-admission
  on `#721` (§6), not folded into the blended cell's own claim.

## 5. H4 — the arch-neutrality census, replacing the absent-diff inference

The dossier's `§0` claimed every primitive the oracle drives is "unmodified,
verbatim shared code with aarch64 — confirmed by the absence of a kernel
diff." An absent diff proves unmodified; it says nothing about shared, and
two named primitives in that same paragraph are not shared. Replaced with an
actual census, re-verified directly against the tree at this fix round's
bytes:

| file | `target_arch` occurrences | note |
|---|---|---|
| `kernel/src/syscall/session.rs` | 0 | `sys_setsid` and friends |
| `kernel/src/syscall/ioctl.rs` | 0 | ioctl dispatch |
| `kernel/src/tty/ioctl.rs` | 0 | TIOCSCTTY/TIOCGWINSZ/etc. |
| `kernel/src/tty/termios.rs` | 0 | tcgetattr/tcsetattr |
| `kernel/src/tty/line_discipline.rs` | 0 | canonical/raw/ONLCR |
| `kernel/src/tty/mod.rs` | 0 | |
| `kernel/src/tty/pty/mod.rs` | 0 | posix_openpt/grantpt/unlockpt/ptsname |
| `kernel/src/tty/pty/pair.rs` | 0 | |
| `kernel/src/syscall/pty.rs` | 0 | PTY syscall entry points (M1a's own fix site) |
| `kernel/src/ipc/fd.rs` | 0 | `close_cloexec()` itself |
| `kernel/src/tty/driver.rs` | **14** | every occurrence is console byte-out (`serial_aarch64::raw_serial_char` vs `serial::write_byte`) or arch-specific diagnostic text — disclosed, not hidden; none of it is TTY/PTY *semantics* |

Two corrections to the dossier's specific claims:
1. **`fork` is not shared code** — `sys_fork_with_frame` is
   `#[cfg(target_arch = "x86_64")]` (`handlers.rs:1801`), with a separate
   aarch64 implementation. But `process::fork()` is called from exactly one
   place in the oracle: `arm_cloexec_exec()` (`tty_oracle.rs:762`) — the
   excluded arm. **Zero of the 13 x86 arms touch arch-split process code at
   all**, which is also why the `ForkResult` import needed its own aarch64
   cfg.
2. **`tty/driver.rs` is not zero-`target_arch`** — 14 occurrences,
   enumerated above, all console-byte-out plumbing rather than TTY/PTY
   protocol logic.

What *is* true and survives as the blended shared-code argument for the 13
arms: every file the 13 x86 arms' syscalls dispatch through (session,
ioctl, termios, line-discipline, PTY alloc/pair, PTY syscall entry,
`close_cloexec()` itself) carries zero arch conditionals, plus the M1a
mutation (§ confirm dossier, unchanged this round) reddening with the exact
same signature on both arches as positive cross-arch identity evidence, not
merely an absence-of-diff inference.

## 6. H2 — `#721` re-admission trigger tracked

Commented on `#721` (https://github.com/ryanbreen/breenix/issues/721) with
the exact re-admission steps: delete the three `#[cfg(target_arch =
"aarch64")]` gates in `tty_oracle.rs` (the call site plus the two arm-local
imports), collapse `ARM_COUNT` back to an unconditional `14`, add
`cloexec_exec` to the x86 gate's `EXPECTED_ARMS`, delete
`CLOEXEC_EXEC_VERDICT_LITERAL` and its now-inverted check, and note that
`the_x86_gate_refuses_a_cloexec_exec_verdict` becomes stale once the literal
it checks is removed and should be deleted in the same change (the
arm-census tests re-sync automatically; that one specific rule does not).

## 7. H3 — `#705`'s body fixed

`gh issue edit 705 --body-file` replaced the un-expanded `@path` literal
with the real content, recovered from the surviving scratchpad file
(`scratchpad/gtty/issue-705-body-new.md`, unchanged from arc 4's original
authoring — verified present and byte-identical to what `verify.md`/
`dossier.md` both summarized). `gh issue view 705 --json body` now returns
the full three-item coverage-gap record, not the bare path string.

## 8. Medium/low dispositions

- **M1** (`#731` attributes a red to `main` without a `main` baseline) —
  commented on `#731`: corrected the "structural, not baselined" framing
  explicitly (the code-unreachability proof stands on its own regardless of
  whether `main` itself flakes at the same rate; whether it does was never
  measured this arc). A dedicated `main @ 09ae3f44` baseline run was not
  taken this round (out of scope: baselining a pre-existing, independently
  filed gate is not this fix round's job).
- **L1** (verify.md's stale claim that `kernel/src/syscall/pty.rs` doesn't
  exist) — not actioned: `verify.md` is a prior slot's session-scoped
  scratchpad artifact, not committed in-repo; nothing in the repository
  carries the error.
- **L2** (`EVIDENCE-x86-confirm-2026-08-31.md`'s "both arches" → "both
  serial streams") — fixed in place (§ above).
- **L3** (`run-x86-tty-oracle-gate.sh`'s POLL bound had no rationale) —
  raised to 240 (matching `run-x86-prod-profile-boot-test.sh`'s own
  measured-vs-bound comment) with a comment naming why: identical profile,
  one extra spawn ahead of bsshd.
- **L4** (the aarch64 mutation battery's `--features` leg re-implements the
  profile-fidelity check inline rather than calling a shared fn) —
  acknowledged, not fixed this round. Inherited from arc 4's aarch64 battery
  (not port-introduced), low severity (fails loud on drift, does not mask a
  real regression), and the correct fix (extract a shared `fn` called by
  both the `#[test]` and both mutation batteries, aarch64 and x86) touches
  test-harness structure beyond this round's blocking/high scope. Deferred,
  disclosed rather than silently dropped.
- **L5** (`the_gate_scores_exactly_the_arms_the_oracle_drives` is not
  arch-aware; symmetric with the x86 side only by accident) — acknowledged,
  not fixed this round. Same reasoning as L4: pre-existing, fails loud, a
  real fix needs the aarch64 rule to also read `run()`'s cfg gates the way
  `x86_reachable_arms()` already does, which is more than a cheap edit.
- **L6** (`#731` mislabels `#725` as closed) — fixed via comment on `#731`
  (§ M1 above, same comment).
- **L7** (nothing has merged; program law is post-merge-only declaration) —
  unchanged disposition: this fix round pushes the branch and expects a PR
  + merge to follow before either cell is declared, per the review's own
  closing instruction ("Then PR, merge, and declare — in that order").

## 9. Self-checks, this round

- `cargo test --test teardown_structure` — 81/81.
- `cargo test --test tty_oracle_structure` — 16/16.
- `cargo test --test exec_lock_order_structure` — 34/34 (M2's predicted-green
  suite, actually run rather than predicted by hand).
- Userspace builds both arches, zero warnings.
- Kernel builds both arches (zero-feature x86 profile; aarch64 soft-float
  target), zero warnings.
- One standing x86 production-profile gate boot, green, on beast (new pins).
- One dedicated x86 TTY oracle gate boot, green, on beast (new pins, 240s
  bound).
- Full serials: `serials/x86-fix2-prod-profile-gate-verdict-20260831.txt`,
  `serials/x86-fix2-dedicated-tty-oracle-gate-verdict-20260831.txt`.
