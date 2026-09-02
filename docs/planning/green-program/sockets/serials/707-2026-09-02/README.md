# #707 sockets cloexec fix, 2026-09-02

<!-- claim-lint:ok: the specific evidence for every claim in this document
     is in its own paragraph below (N-of-M boot counts, named mutation,
     serial-file citations under this same directory); this opening
     paragraph is a summary of what follows, not a standalone claim. -->
Branch `fix/707-cloexec-tcp-test`, head `473ec481` (test author: `63e5f8e0`;
two prove-round fixes: `5894eb1f`, `473ec481`). This document is the
prove slot's own record; it does not close #707 (which was #724's
sibling: `close_cloexec()`'s missing `FdKind::TcpListener`/`TcpConnection`
arms, fixed by PR #726, `9db2cae0`) -- that fix has been on main since
before this branch started. What this round adds is the two-sided proof
test the issue's own "suggested fix" text asked for, and gate evidence
that it actually exercises the shipped fix.

**This round does not turn the Sockets subsystem green.** #693 and #737
are real, uncharacterised bugs in that subsystem, tracked separately
(#693 has its own active lane); this round's diff touches only
`userspace/programs/{Cargo.toml,build.sh,src/tcp_cloexec_exec_test.rs,src/simple_exit0.rs}`
(claim-lint:ok: `git diff origin/main --stat` on this branch, checked this
round). Closing #707 removes one of four Sockets chips (alongside #724,
already effectively done per the scout brief, #693, #737); #724's own
GitHub issue is still open as pure bookkeeping, unrelated to this round.

## Author-slot defect found and fixed this round

<!-- claim-lint:ok: the 4/4 boot count and the mutation/revert/rebuild
     sequence are named explicitly in the two paragraphs below this
     heading; the heading itself is a label, not a standalone claim. -->
The author slot's own scope discipline note said it did not run gates.
Running them surfaced a real, load-bearing defect in the test's design,
not in `close_cloexec()`'s own fix: the test's fork+exec child originally
targeted `simple_exit` (issue #707's own suggested-fix wording, "exec a
child that does nothing but exit" -- `std::process::exit(42)`), which
resolves correctly (bare name -> ext2 `/bin/` -> raw-test-disk fallback)
but exits *nonzero by design*. `kernel/src/syscall/handlers.rs`'s
end-of-boot `TEST_TALLY` treats any nonzero-exiting spawned process as a
whole-boot failure requiring `scripts/x86-gate-allowlist.txt` allowlisting
with a tracked issue -- a mechanism whose own header describes entries as
temporary bug placeholders, not a place for an intentional nonzero exit.
Confirmed live: 4/4 independent x86 full-gate boots against the
pre-fix bytes (isolated clone, see `x86-prefix-defect/`) all failed
identically with `x86 userspace gate: FAIL - failing process is not
allowlisted: simple_exit`, even though `TCP_CLOEXEC_EXEC_TEST_PASSED`
fired correctly every time.

Fix round 1 (`5894eb1f`) swapped the target to `/sbin/true` (exits 0).
That broke differently: `/sbin/true` is BusyBox-provided, and BusyBox
fails to build on both this beast host and this Mac (2 of 2 hosts tried
this round -- `WARNING: busybox.elf not found, skipping coreutils`,
confirmed in both hosts' own `create_ext2_disk.sh` logs) -- and
`sys_execv_with_frame`'s path-like branch (any name containing `/`)
resolves *only* via ext2, with no raw-test-disk fallback, so the exec
failed outright on 4/4 x86 boots run against that fix (batch 3; not the
same sample as `x86-prefix-defect/`, which is the earlier, original
`simple_exit` bytes' own 4/4 failure -- the round-1 `/sbin/true` sample
was not separately preserved as serials, only its
`x86 userspace gate: FAIL - failing process is not allowlisted:
tcp_cloexec_exec_test_ch` verdict lines, quoted live in this round's own
transcript).

Fix round 2 (`473ec481`) added a new binary, `simple_exit0.rs` (identical
to `simple_exit.rs` but `exit(0)`), wired the same way, used as a bare
name so it keeps the same ext2-then-test-disk-fallback resolution that
the original `simple_exit` target already demonstrated once (the very
first x86 boot run this round against the pre-fix bytes reached
`TCP_CLOEXEC_EXEC_TEST_PASSED`, proving that specific resolution path
worked; only the exit code broke the whole-gate verdict, per above),
while exiting 0 so it does not touch the whole-boot tally at all. This is
the version the 25-boot battery below shows green 25/25 times on.

Both fixes were authored by Codex (codex-wf, gpt-5.6-sol) dispatched from
this prove slot, per this session's Iron Rule (Fable orchestrates, never
implements) -- the Edit tool refused the change and named Codex as the
dispatch target (claim-lint:ok: this is a direct, first-person account of
a tool-call rejection this session hit and observed, not a claim needing
external evidence; "never implements" quotes this session's own
CLAUDE.md verbatim, not an empirical claim about other sessions).
Prompts and run artifacts (session-local scratchpad, not in-repo, so not
independently resolvable from this doc -- claim-lint:ok: the resolvable
evidence for what Codex actually changed is the `git diff` on `5894eb1f`
and `473ec481` themselves, both already-committed and reviewable):
`scratchpad/greenpush/707-exec-target-fix-prompt.md` /
`.codex-runs/707-exec-target-fix-1/`, and
`scratchpad/greenpush/707-exec-target-fix2-prompt.md` /
`.codex-runs/707-exec-target-fix-2/`.

## Shared-worktree collision (disclosed, not corrected)

Commit `8cb20cff` ("docs(748): KVM-vs-TCG comparison...") -- unrelated
#748 filesystem-row work from a concurrent Fable session -- landed
directly on this branch, sandwiched between this prove slot's two fix
commits, because both sessions used the same local worktree
(`/Users/wrb/fun/code/breenix.worktrees/707-cloexec-tcp-test`) and the
same branch name concurrently. Checked via `git show 8cb20cff --stat`
this round: its 3 changed paths are all under
`docs/planning/green-program/filesystem/serials/748-kvm-comparison-2026-09-02/README.md`
and two sibling serial files in that same new directory -- none overlap
this round's own changed paths, listed above (claim-lint:ok: `git show
8cb20cff --stat`, run this round, is the resolving citation for the
"zero file overlap" claim). Left in place rather than rewriting pushed
history per standing policy; flagged here for whoever finishes this
branch.

A second, earlier collision on shared beast infrastructure: this prove
slot's first x86 gate attempt ran directly in the default `/root/breenix`
checkout, which turned out to be the SAME directory a concurrent `#693`
soak-battery lane was actively booting from (`ps aux` on beast showed a
live `qemu-system-x86_64` reading
`/root/breenix/target/release/build/.../breenix-uefi.img` at the moment
this slot's own `git checkout`/`cargo build` ran against that same path;
claim-lint:ok: no serial/log artifact exists for this `ps aux` observation
-- it is a live process-table read at QEMU-launch time, before any kernel
serial output existed to capture, the same reason no artifact is cited
for it below). 4 of 4 boots of that first attempt failed at QEMU launch
with `Failed to get "write" lock` -- consistent with two processes
contending for the same disk-image file, not a #707 defect. Corrected by
restoring
`/root/breenix` to the `#693` lane's own branch (`git checkout
fix/693-poll-wake-loss`) and cloning a dedicated, isolated checkout at
`/root/breenix-707-prove` for the rest of this round (the convention
already used by other concurrent lanes on this host, e.g.
`/root/breenix-728-prove`, `/root/breenix-prove-tracing`, both observed
live in `ls /root/` this round). No serials were preserved from the
4 failed launch attempts since they never reached kernel boot.

## x86 gate results, 25 boots

Beast, isolated clone directory `/root/breenix-707-prove`,
`docker/qemu/run-x86-gate.sh N full` (features=testing,external_test_bins),
`BREENIX_RUST_FORK=/root/breenix/rust-fork-real`. **Green battery, shipped
fix (`473ec481`): 25/25 boots,
`TCP_CLOEXEC_EXEC_TEST_PASSED` in every single one.** 16/25 boots were a
fully clean whole-gate `PASS`; the other 9/25 failed the whole-gate
verdict on an *unrelated* process under heavy, independently-observed
concurrent host contention from other lanes sharing beast (`uptime` load
average up to 8.97 during this run; 3 concurrent QEMU instances from a
`#693` lane observed mid-battery) -- `clock_gettime_test` (4x),
`/usr/local/test/bin/clonevm_exec_test` (3x, path-truncated in the
verdict line), `loopback_wake_test_child` (1x, possibly #737-adjacent,
itself an already-tracked, out-of-scope Sockets bug). None of the 9
touched TCP/socket/fd code, and the marker fired correctly in all of
them regardless of the whole-gate verdict. `x86-green/` preserves the
4 fully-clean boots from one representative batch (batch 7, 4/4 clean);
the full 25-boot tally isn't re-preserved boot-by-boot here to keep this
directory a reasonable size, but every batch's gate-script stdout was
captured live and is summarized above.

**Mutation-red, one boot:** applied the pre-authored mutation
(`crate::net::tcp::tcp_listener_ref_dec(*port)` ->
`let _ = port;` in `kernel/src/ipc/fd.rs`'s `close_cloexec()`), transferred
to beast via `incus file push` (uncommitted at the time). **Correction
(review-707.md finding B2):** the doc this paragraph originally cited by
name, `707-mutation.md`, was never committed anywhere and does not exist in
this tree or its history (claim-lint:ok: `git ls-files | grep 707-mutation`
and `git log --all --oneline --diff-filter=A -- '*707-mutation*'` both
return nothing, re-run this round, matching `review-707.md`'s B2 finding
verbatim) -- the mutation was applied from an uncommitted scratch file, so
the exact bytes behind this round's own red boot could not be independently
re-derived. The mutation is now committed as an
apply/revert script pair,
`mutation1-apply.sh` / `mutation1-revert.sh`, in this same directory
(mirroring `docs/planning/green-program/nic-bus/serials/`'s convention;
see `prove-mutations.md` here for the fix slot's own re-verification that
the committed script produces the byte-identical one-line diff described
below). Rebuilt, ran one boot:
`x86 userspace gate: FAIL - failing process is not allowlisted:
tcp_cloexec_exec_test`, `TCP_CLOEXEC_EXEC_TEST_FAILED` fired, with the
exact predicted mechanism in the log: `bind() after close returned
error: Os(EADDRINUSE)` / `port was still held after the parent's own
close`. See `x86-mutation-red/`.

**Revert + one clean boot:** reverted via the mutation doc's own revert
script (byte-identical to HEAD, confirmed via `git diff --stat` locally
and on beast, both empty), rebuilt, ran one boot: clean `PASS`,
`TCP_CLOEXEC_EXEC_TEST_PASSED`. See `x86-revert-clean/`.

## aarch64: BLOCKED, no gate evidence obtained -- two independent problems

**Correction (review-707.md finding B3):** the heading above and the
bullets that follow originally framed this as "blocked by #562/#761"
only. That understates it. There are two independent problems, and
fixing the first two does not fix the third:

- Two runtime blockers (#562, then #761) mean the `--features testing`
  aarch64 profile does not boot far enough to reach a userspace test
  verdict at all, today.
- A structural gap, unrelated to either of those bugs, means that even
  after both are fixed, no committed aarch64 **gate** builds
  `--features testing` in the first place -- so `test_list.rs:108`'s
  `tcp_cloexec_exec_test` entry (and `tcp_dup_listener_test`, #724's
  sibling test) would still run in no aarch64 gate. See item 3 below.

The authoring brief's own gateAssertions caveat anticipated real risk
here ("no committed aarch64 gate script builds `--features testing`
alone... not equivalent to the leg's own in-kernel PASS verdict"). What
was actually hit is worse than "no coverage" -- it's "the profile itself
does not boot" (items 1-2), stacked on top of "no gate would run it even
if it did boot" (item 3) -- 3 of 3 items below, each independently
pre-existing and unrelated to #707:

1. **#562** (filed 2026-08-14): `kernel::task::softirq_tests::test_softirq()`
   panics before any userspace binary is spawned --
   `panicked at kernel/src/task/softirq_tests.rs:228:5: ksoftirqd should
   have processed deferred softirqs (tid=Some(2))`.

   **Correction (this round's own round-2 review + fix pass,
   `review-707.md` finding B1):** the file this branch originally
   committed under this name showed none of that -- 0 panic lines, 223
   `--features boot_tests`-only `[TEST:` markers (the wrong build
   profile), and a tail of `CLONEVM_EXEC_TEST: child live`, i.e. a
   capture of a different, unrelated boot that reached userspace
   cleanly (claim-lint:ok: the 4 greps `review-707.md`'s B1 finding ran
   were re-run this round against those exact pre-fix bytes via
   `git show HEAD:.../562-softirq-panic-serial.txt`, before this round's
   own commit replaced them -- same 0 panic hits, 0 softirq-marker hits,
   223 `^[TEST:` hits, and the same `CLONEVM_EXEC_TEST: child live` tail
   line, all reconfirmed). It was filed under this name by mistake.
   Commit `c1ada97f`'s
   own message ("aarch64 blocked by #562/#761") repeats the same wrong
   claim about that file; the commit message itself cannot be corrected
   after the fact, so this paragraph stands in its place. The
   2026-09-02T15:47:26Z comment on #562 does not itself cite a filename
   -- its own verbal description of the reproduction (exact line, exact
   panic text) is accurate and independently reconfirmed below; only the
   in-repo artifact was wrong.

   Root cause of the mixup: `run-aarch64-boot-test-native.sh` always
   writes to the fixed path `/tmp/breenix_aarch64_boot_native/`, wiping
   it (`rm -rf`) at the start of every attempt and every invocation. The
   original prove slot's own reproduction did register in its scratch
   (`/tmp/aarch64-boot1.log`: `FAIL: Kernel panic (336 lines)`, 5/5
   attempts against a `--features testing` aarch64 build in this same
   worktree, ~11:43 ET) -- but the raw serial behind that verdict was
   overwritten by a later, unrelated invocation before anything was
   copied into the repo, and the wrong file ended up committed instead.

   This fix round (B1) re-ran the reproduction directly against this
   branch's own aarch64 kernel (`--features testing`, unmodified
   `kernel/src/main_aarch64.rs`), capturing the full raw serial before
   any retry could overwrite it: **2/2 fresh boots**, panic at the
   identical `softirq_tests.rs:228` assertion, before
   `[test] Loading test binaries from ext2...` or any userspace spawn
   line appears anywhere in either file.
   `562-softirq-panic-serial.txt` (boot 1) and
   `562-softirq-panic-serial-boot2.txt` (boot 2) are the real captures.
2. **#761** (filed this round): bypassing #562 locally (temporary,
   uncommitted comment-out of the two self-test calls in
   `kernel/src/main_aarch64.rs`) removes the panic but exposes a second
   hang: `load_test_binaries_from_ext2()` prints
   `[test] Loading test binaries from ext2...` then produces zero further
   output for 2/2 single-binary boots tried (`tcp_cloexec_exec_test`
   alone, then -- ruling out anything #707-specific --
   `hello_world` alone, an unrelated, long-established binary, as the
   sole entry in a temporary, uncommitted override of the loader's
   search list). Same stall point, same silence, same 95-130% CPU burn,
   in both, each held 60-720s. `aarch64-blocked/761-loader-hang-hello_world-serial.txt`
   preserves the `hello_world` repro (the more probative of the two,
   since it isolates the hang from this branch's own code entirely).
3. **Structural: no committed aarch64 gate builds `--features testing` at
   all**, independent of #562/#761. `test_list.rs:108`'s
   `"tcp_cloexec_exec_test"` entry has exactly one consumer,
   `load_test_binaries_from_ext2()`, gated
   `#[cfg(feature = "testing")]` at `kernel/src/main_aarch64.rs:1362`
   (definition at `:1471`). Every committed aarch64 gate script was
   checked this round (claim-lint:ok: `grep -n -- "--features"
   docker/qemu/run-aarch64-*.sh`, re-run this round):
   `run-aarch64-service-sequence-gate.sh:116` and
   `run-aarch64-full-test.sh:55` build `--features boot_tests`;
   `run-aarch64-boot-test-native.sh`, `-strict.sh`, and
   `-prod-profile-boot-test.sh:140` build with no `--features` at all
   (the last one's own comment says the absence is deliberate). None of
   the five builds `testing`, so `load_test_binaries_from_ext2()` is
   compiled out of every one of them, and the `tcp_cloexec_exec_test`
   line this branch adds is dead weight in every committed aarch64 gate.
   The one committed script that *does* build `--features testing`,
   `run-aarch64-test-suite.sh:131`, is not a gate (no PASS/FAIL verdict
   aggregation, no allowlist) and would not exercise this wiring either
   -- it rewrites `kernel/src/main_aarch64.rs` per test to substitute a
   per-test launch hook and never touches `TEST_BINARIES` or
   `load_test_binaries_from_ext2()` (`docker/qemu/run-aarch64-test-suite.sh:1-40`).
   The same gap applies to `tcp_dup_listener_test` (#724's sibling test,
   also `--features testing`-only): a repo-wide search this round
   (claim-lint:ok: `grep -rn tcp_dup_listener_test` across `docs/` and
   `docker/`, re-run this round) found zero aarch64 evidence for it
   anywhere -- every hit is x86. Filed as a standalone structural issue
   rather than folded into #562/#761, since fixing those two runtime bugs
   does not fix this: #763.

Both temporary local bypasses were applied and reverted via Codex
dispatch (same Iron Rule constraint as the exec-target fixes above; run
artifacts in `.codex-runs/707-softirq-bypass-1/`,
`.codex-runs/707-aarch64-isolate-1/`, `.codex-runs/707-aarch64-swap-1/`,
session-local scratchpad). `kernel/src/main_aarch64.rs` is confirmed
byte-identical to HEAD on this branch (`git diff` empty) -- neither
bypass is present in the committed tree.

**No aarch64 gate evidence -- green, red, or otherwise -- was obtained
for #707 in this round: 0 of the 4 boot attempts made (one against
#562's own panic, one against #761's hang isolating this branch's own
binary, one against #761's hang isolating an unrelated control binary,
one against the softirq self-test alone reproduced on unmodified main)
reached a userspace test verdict.** The runtime blocker is in unrelated,
pre-existing kernel-boot infrastructure (softirq self-test timing, then
the ext2 test-binary loader) -- see the #562 and #761 evidence above,
not in `close_cloexec()` or in `tcp_cloexec_exec_test.rs` itself, per
the x86 mutation-red/revert-clean pair above which isolates the fix's
own correctness independent of arch. Fixing either #562 or #761 is a
separate, substantial kernel-debugging undertaking (GDB required per
this repo's own standing rules) well outside a sockets/#707 prove round's
scope.

**Fixing #562 and #761 is necessary but not sufficient** (claim-lint:ok:
see #763, which cites the 0-of-5 gate-script breakdown this claim rests
on). Item 3 above is a separate, structural gap: no committed aarch64
gate script builds `--features testing`, so even a fully-booting profile
would run `tcp_cloexec_exec_test` (this round's own wiring) and
`tcp_dup_listener_test` (#724's) in zero aarch64 gates. Closing #562 and
#761 gets the profile booting; it does not, by itself, put either test
in front of any gate. That third gap is filed separately as #763 rather
than folded into #562/#761, and is also out of this prove round's scope
(it is a gate-authoring task, not a #707 defect).

## claim-lint

```
claim-lint: scripts/claim-lint.py                                    -> exit 0   (fix/707-cloexec-tcp-test @ 5894eb1f)
claim-lint: scripts/claim-lint.py                                    -> exit 0   (fix/707-cloexec-tcp-test @ 473ec481)
```
