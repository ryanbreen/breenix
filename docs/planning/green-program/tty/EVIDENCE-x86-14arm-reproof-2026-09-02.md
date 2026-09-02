# TTY — x86 14-arm reproof at #745's shipping bytes, 2026-09-02

Green program, atlas reproof pass (R58 bar). Branch
`green/atlas-reproof-2026-09-02`, `main @ 0efa94a9` (merge of PR #753, which
closed #745 — x86 fork() now works in the zero-feature production profile).
Serials referenced below live alongside this file in `serials/`.

## Why this leg ran

The atlas carried TTY x86-64 and Blended as green at 14 arms (full parity
with aarch64, `cloexec_exec` included) on the strength of two artifacts:

- `docs/planning/745-x86-fork/serials/tty-oracle-14of14-pass-2026-09-02.txt`
  and `.../anti-vacuity-pre-fix-refused-gate-2026-09-02.txt` — single-boot
  evidence that arm 14 executes and passes once fork() is de-gated.
- `docs/planning/745-x86-fork/serials/tty-oracle-25boot-soak-2026-09-02.txt`
  — a 25-boot run of `docker/qemu/run-x86-tty-oracle-gate.sh`, 25/25 green,
  14/14 arms every boot.

The soak file is real and its numbers are accurate for the commit it was
taken at (25 of 25 boots, 14/14 arms, per its own bytes). That commit is
**`3bf42613`**
(`git log --diff-filter=A -- docs/planning/745-x86-fork/serials/tty-oracle-25boot-soak-2026-09-02.txt`
names it as the sole author of that file), which sits two commits **before**
the round-2-review fix `ed6a7c57` ("hoist TLS out of the PM window, make the
CoW receipt order-independent", #745/#756). `git diff --stat 3bf42613
0efa94a9 -- kernel/` shows that fix touched `kernel/src/process/manager.rs`
(+65/-… ) and `kernel/src/syscall/handlers.rs` (+91/-…) — the exact fork
window arm 14 drives (`sys_fork_with_parent_context`,
`register_thread_tls` relocation, `complete_fork`'s CoW receipt ordering).
The 25-boot soak's own bytes never saw that change. Criterion (2) — the
measured arm must be the shipping arm — was unmet at 25-boot scale for the
bytes actually on `main`. This leg closes that gap by re-running the real
14-arm gate at `main`'s current HEAD, `0efa94a9`.

## Leg run

```
ssh beast 'sudo -n incus exec breenix-x86 -- bash -lc \
  "cd /root/breenix && bash docker/qemu/run-x86-tty-oracle-gate.sh --boots 25 --rebuild-userspace"'
```

- Repo state confirmed on beast before the run: `git fetch origin && git
  checkout main && git reset --hard origin/main` → `HEAD is now at
  0efa94a9`, matching this session's local `git rev-parse HEAD`.
- Build: zero-feature production profile (`cargo build --release --bin
  qemu-uefi`, no `--features`), userspace rebuilt fresh
  (`--rebuild-userspace`), so `tty_oracle.elf` and `init` are the bytes this
  run's kernel actually loads — not a stale binary from an earlier build.

## Result

**25 of 25 boots observed, 14/14 arms PASS every boot, 0 fail.**

```
$ grep -c "14/14 arms PASS" docs/planning/green-program/tty/serials/x86-tty-oracle-25boot-HEAD-0efa94a9-20260902.txt
25
$ tail -3 docs/planning/green-program/tty/serials/x86-tty-oracle-25boot-HEAD-0efa94a9-20260902.txt
Booting the x86_64 production profile with the TTY oracle (boot 25/25)...
  boot 25: 14/14 arms PASS, kernel live (bsshd reached)
PASS: x86 TTY oracle gate - 25/25 boots, 14 arms green on the shipped production profile
```

Boot 1 and boot 25's raw `serial_user.txt` (captured directly, not just the
gate script's own summary line) are preserved verbatim in
`x86-tty-oracle-25boot-HEAD-0efa94a9-boot1-and-boot25-serial-user-20260902.txt`.
Both carry the arm-14 verdict line and the oracle's completion literal:

```
[TTY_ORACLE:cloexec_exec:verdict=PASS:cloexec_survived_fork=1:eof_after_parent_close=1]
[TTY_ORACLE:COMPLETE:pass=14:fail=0]
```

(`COMPLETE` and the arm-14 verdict each appear twice per boot in the raw
capture — the oracle's `emit()` deliberately double-prints for
console-shred resistance, the same shape documented in
`EVIDENCE-x86-confirm-2026-08-31.md`'s correction note. Not a defect.)

## Disposition against the R58 bar

| Criterion | Status |
|---|---|
| (1) implementation complete | met — `arm_cloexec_exec()` is called unconditionally, `ARM_COUNT=14` on both arches, `#745` closed |
| (2) gate evidence exercises the shipping arm | **now met** — 25 boots at `main @ 0efa94a9` itself, not a pre-fix commit |
| (3) in-layer issues | unchanged by this leg — the tty-x86-14 scout brief names #705 and #537 as open in-layer, disposition not re-litigated here |

This leg discharges criterion (2) specifically. It does not adjudicate
criterion (3) (open in-layer issues) or the doc defect the scout flagged in
`745-x86-fork/README.md` — those are separate, non-gate-evidence findings
left to the atlas/issue-triage pass.

## Files

- `serials/x86-tty-oracle-25boot-HEAD-0efa94a9-20260902.txt` — full gate
  script stdout+stderr, 25 boots, build + boot log.
- `serials/x86-tty-oracle-25boot-HEAD-0efa94a9-boot1-and-boot25-serial-user-20260902.txt`
  — raw `serial_user.txt` for boot 1 and boot 25 (first and last of the 25),
  confirming the per-boot summary line against the oracle's actual verdict
  output rather than trusting the gate script's own arithmetic alone.
