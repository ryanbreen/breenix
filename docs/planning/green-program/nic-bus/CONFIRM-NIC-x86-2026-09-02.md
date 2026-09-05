# NIC drivers, x86 — confirming battery, 2026-09-02

Green program. This is the confirming battery for the NIC-x86 cell named as
rank 1 in the bus/NIC/tracing scout brief
(`scout-bus-nic-x86.md`, §6, "Reachable in one gate run — exact sequence for
NIC + Bus", step 2). It runs `docker/qemu/run-x86-prod-profile-boot-test.sh`
— the x86 zero-feature production-profile gate — 25 times on beast, at
`main`, and reports what it measured. #702 stays open after this document,
which does not open a PR either; it is evidence for the ruling slot.

## 0. Coordinator ruling R80 (binding context for this battery)

R80 re-attributes #702 from NIC to Bus/device, on x86 and Blended. As
stated in the ruling and its underlying RCA, not independently re-derived by
this document: the hung boot's last line, `E1000 network device found`, is a
PCI-layer boot-stage marker emitted inside `pci::enumerate()`
(`kernel/src/drivers/pci.rs:1154`, function starts `:1089`) on a vendor/device
ID match, not a call into the driver; `enumerate()`'s own census line
(`pci.rs:1203-1209`) did not appear in the one hung-boot serial the filing
issue preserved, so the hang reads as inside the PCI scan loop, before
`enumerate()` returns. `e1000::init()` is not reached until
`kernel/src/drivers/mod.rs:41`, after `drivers::init()`'s call to
`pci::enumerate()` (`mod.rs:28`) has already returned, so
`kernel/src/drivers/e1000/` reads as not entered on that one hung boot.
Bus-x86 stays blocked on #702; that is not this document's finding, and
closing #702 is not attempted here.
claim-lint:ok: mechanism restated from the ruling's cited RCA,
docs/planning/green-program/nic-bus/serials/702-rca/RCA-2026-08-31.md, one
specimen boot, not re-derived independently in this document.

## 1. Setup

Beast (`breenix-x86` Incus container) synced to `origin/main` before the
build:

```
$ git fetch origin && git checkout main && git reset --hard origin/main && git log --oneline -1
52491c4b Merge pull request #759 from ryanbreen/green/atlas-reproof-2026-09-02
```

`52491c4b` is a docs-only successor of the commit named in this task's brief
(`0efa94a9`): `git diff --stat 0efa94a9..52491c4b` reports 6 files changed,
2155 insertions(+), 3 deletions(-), across the 4 commits in
`git log --oneline 0efa94a9..52491c4b`. The 6 changed paths: 2 markdown docs
(`docs/planning/745-x86-fork/README.md`,
`.../tty/EVIDENCE-x86-14arm-reproof-2026-09-02.md`) and 4 serial-capture
`.txt` files in the same `745-x86-fork/tty/` tree — 0 of the 6 paths are
under `kernel/`. Measured locally in the Mac checkout (this repo, same
commit range) before dispatching to beast; not re-run on beast.
claim-lint:ok: N-of-M — 6 files changed (2 .md, 4 .txt), 0 of 6 under
kernel/, 4 commits, measured via `git diff --stat 0efa94a9..52491c4b` and
`git log --oneline 0efa94a9..52491c4b` in this repo at the time this
document was written.

`userspace/programs/exec_smoke.elf` and `exec_smoke_target.elf` were already
present on beast, both reporting `e_machine=62` (`EM_X86_64`) via
`od -An -tu2 -j 18 -N 2`, checked once before the battery started (not
re-checked per boot) — dated the same day as the TTY-oracle 25-boot
batteries this gate's own comments cite, so no separate userspace rebuild
was run by this battery. The script's own build step
(`cargo build --release --bin qemu-uefi`, no `--features`) and
`create_ext2_disk.sh` do re-run on each of the 25 invocations regardless,
per the script's own unmodified logic (§2). `pgrep -f qemu-system-x86_64`
returned no matches on the container immediately before the battery started.
claim-lint:ok: e_machine=62 checked once via `od`, N=1 check, not per-boot;
build/ext2 re-run behavior is the script's own logic (this document did not
modify the script).

## 2. Battery: 25 sequential boots

`docker/qemu/run-x86-prod-profile-boot-test.sh` does one full build + one
boot per invocation (it takes no boot-count argument), so the battery is a
25-iteration shell loop calling the unmodified script 25 times, copying the
container's `/tmp/breenix_x86_prod_profile/serial_kernel.txt` and
`serial_user.txt` out to a per-boot directory immediately after each
invocation returns and before the next invocation's
`rm -rf "$OUTPUT_DIR"` at the top of the script (confirmed present in the
script body, §0 of this document does not re-quote it). Per-boot stdout and
both serial files for boot 1 of 25 are committed at
`docs/planning/green-program/nic-bus/serials/x86-prod-profile-25boot-2026-09-02/boot_1_stdout.txt`
and `.../boot_1/serial_kernel.txt` — the same pattern repeats for boots 2
through 25 (§4 lists the two-file-per-boot, one-stdout-per-boot layout by
name).
claim-lint:ok: boot_1 paths above resolve in this commit; boots 2-25 follow
the identical `boot_N_stdout.txt` / `boot_N/serial_kernel.txt` /
`boot_N/serial_user.txt` naming, confirmed by the 75-file count cited in §4.

```
export BREENIX_RUST_FORK=/root/breenix/rust-fork-real   # present, unused by
                                                          # this script but
                                                          # harmless to export
for i in $(seq 1 25); do
    ./docker/qemu/run-x86-prod-profile-boot-test.sh
    # copy $OUTPUT_DIR/serial_{kernel,user}.txt out here
done
```

Run window: 2026-09-02 13:19:05Z (boot 1 start) — 14:03:34Z (boot 25 done),
~2696s for 25 boots, ~108s/boot average.

**Result: 25 of 25 boots PASS, 0 of 25 failures.**
Verified two independent ways per boot: the loop wrapper's own captured exit
code (`boot N exit_code=0`, all 25, in `00-loop-summary.txt`) and the gate
script's own verdict line (`grep -c '^PASS:' boot_N_stdout.txt` == 1 and
`grep -c '^x86 production-profile gate: FAIL' boot_N_stdout.txt` == 0, all
25). No `report_gate_failure` capture directory was produced by this battery
— the newest directory under `/tmp/breenix_x86_prod_profile_failures/` at
battery start was already 7 hours stale (`20260902T062458Z`, from a prior
session's work), and it stayed the newest directory throughout the battery,
confirming `report_gate_failure` (the script's `ERR` trap) never fired.

The gate script itself embeds roughly 50 `test` assertions between the boot
and its verdict line (§ script body, `run-x86-prod-profile-boot-test.sh`);
since all 25 invocations exited 0 with a `^PASS:` line and none produced a
`report_gate_failure` capture (§2), all ~50 assertions passed on all 25
boots — `set -e` plus the script's own `ERR` trap means a single failing
`test` on any boot would have aborted that invocation non-zero and captured
a failure directory, neither of which this battery observed. This includes
the full #721 exec-smoke and #745 fork-smoke chains, and the 21 test-only
markers plus 3 fault markers this document counted at zero for boot 1 (§4)
— the same 24 literals are asserted `-eq 0` on all 25 boots by the script's
own unmodified logic, not spot-checked per boot by this document beyond
boot 1. See any `boot_N_stdout.txt` for the complete `print_observed_values`
dump per boot.
claim-lint:ok: inferred from 25/25 exit-0 + 25/25 `^PASS:` + 0/25
`report_gate_failure` captures (§2), combined with the script's own
`set -e`/`ERR`-trap semantics (unmodified by this document) rather than a
per-boot re-grep of all ~50 assertions; boot 1's dump (§4) is the one boot
this document actually re-grepped in full.
`pgrep -f qemu-system-x86_64` on the container returned no matches after the
battery finished (1 check, not per-boot).

## 3. NIC-exercising markers — what bar (2) is actually being judged on

The gate's own boot-NIC device lines, self-counted from the script's own
bytes (matches the scout brief's citation exactly):

```
$ grep -cE -- '^[[:space:]]*-device virtio-blk-pci,drive=' docker/qemu/run-x86-prod-profile-boot-test.sh
3
$ grep -cE -- '^[[:space:]]*-device e1000,' docker/qemu/run-x86-prod-profile-boot-test.sh
1
$ grep -cE -- '^[[:space:]]*-netdev ' docker/qemu/run-x86-prod-profile-boot-test.sh
1
```

One `-netdev` and one `-device e1000,netdev=net0,...` — an explicit netdev is
wired, so QEMU adds no implicit default NIC; the e1000 in this profile is
unambiguously the one and only boot NIC.

This gate carries no explicit device-count assertion of its own (the "missing
leg" the scout brief names in §7 — a script-only gap, not touched by this
battery). What it does have, and what this battery measured by grepping the
preserved `serial_kernel.txt` for each of the 25 boots (table below, all
counts N=25), is functional evidence the NIC path executed — not just that
QEMU attached the device, but that the kernel's e1000 driver initialized it,
brought the link up, and drove TX+RX traffic through it:
claim-lint:ok: N-of-M table immediately below this paragraph, 25/25 per row,
computed by grep against the 25 committed `serial_kernel.txt` files.

| marker | literal | boots hit / 25 |
|---|---|---|
| PCI enumeration reached the NIC and completed | `PCI: Enumeration complete` | 25/25 |
| e1000 detected during PCI scan | `E1000 network device found` | 25/25 |
| e1000 driver initialized (`kernel/src/drivers/e1000/`) | `E1000 driver initialized` | 25/25 |
| link brought up | `E1000: Link up` / `[net] e1000 link up` | 25/25 (2 lines/boot — driver-level + net-layer) |
| network stack init completed | `NET: Network initialization complete` | 25/25 |
| ARP request transmitted (TX through `e1000::transmit()`) | `ARP request sent successfully` | 25/25 |
| ARP reply received (RX interrupt path, off SLIRP) | `ARP: Reply from 10.0.2.2 ->` | 25/25 (2 lines/boot) |

Every boot's `E1000 network device found` line (line 278 of every one of the
25 committed `serial_kernel.txt` files, identical line number across all 25 —
a deterministic boot sequence at this profile) is followed immediately by
`PCI: Enumeration complete` on the very next line and by ~3400-3500 more
kernel-log lines before the boot reaches steady state — i.e. every boot in
this battery is the observed opposite of the #702 signature (hang with
`E1000 network device found` as the last line, `PCI: Enumeration complete`
never printed). 0 of 25 boots in this battery reproduced anything resembling
the #702 signature. This is consistent with, and adds 25 more sequential
boots to, the existing 0/176 #702 null-reproduction record on `main`
(`serials/702-rca/RCA-2026-08-31.md`) — this battery does not attempt to
move that number and reports it only as context.

Total: **25/25 boots reached every NIC-exercising marker listed above; 0/25
showed the #702 hang signature or any other failure.**

## 4. Representative full observed-values dump (boot 1 of 25)

```
PASS: x86 production profile reached steady state with the teardown census at rest
  ext2 root mounted:            1
  kernel init complete:         1
  async executor started:       1
  steady state reached:         1
  console prompt:               2
  tombstone census lines:       1
  tombstone census at rest:     1
  root custody lines:           1
  root custody at rest:         1
  crash markers:                0
  (all 21 test-only markers:    0)
  (all 3 fault markers:         0)
  fixed PRECONDITION 5 pass (#673): 1     fail: 0
  fixed PRECONDITION 7 pass (#672): 1     fail: 0
  preempt census at rest:        1
  prod bracket release at rest:  1
  init designation (#673):      1
  ring3 syscall confirmed (#673): 1
  init first line (#673 M6):    1
  bsshd started / listening (#713): 1 / 1
  spawn-smoke reaped exit 0 (#713): 1     reap failed: 0
  tty oracle exit record:       1         reap failed / oracle failed: 0 / 0
  exec smoke launch/target-enter/target-ok/launcher-exit (#721): 1/1/1/1
  exec smoke spawn/exec/argv failures (#721):  0/0/0
  exec lock order first commit (#721 K7): 1   violations (3 kinds): 0/0/0
  fork smoke launch/child/CoW-OK/parent-reaped/launcher-exit (#745): 1/1/1/1/1
  fork smoke child exit=37 / first CoW fault (#745): 1 / 1
  fork smoke spawn/fork/unexpected-return/reap failures (#745): 0/0/0/0
  console prompt count over 60s: 1 -> 2
```

Two representative files, both resolving in this commit:
`docs/planning/green-program/nic-bus/serials/x86-prod-profile-25boot-2026-09-02/boot_1_stdout.txt`
and
`docs/planning/green-program/nic-bus/serials/x86-prod-profile-25boot-2026-09-02/boot_25/serial_kernel.txt`.
The same three-file-per-boot pattern (`boot_N_stdout.txt`,
`boot_N/serial_kernel.txt`, `boot_N/serial_user.txt`) is committed for boots
1 through 25 (75 files), plus one `00-loop-summary.txt` — 76 files total,
matching `find
docs/planning/green-program/nic-bus/serials/x86-prod-profile-25boot-2026-09-02
-type f | wc -l` = 76 at commit time.
claim-lint:ok: two named paths above resolve in this commit; the 76-file
total is a `find | wc -l` count taken at commit time, recorded in the
branch's commit message.

## 5. What this battery does and does not settle

**Settles:** NIC-x86 bar (2) evidence at `main` (`52491c4b`, docs-only
successor of `0efa94a9`) — functional NIC traffic (TX+RX) on the zero-feature
shipping profile, 25/25 boots, with no gap between "device attached" and
"driver actually ran." Under R80's re-attribution, NIC-x86 has zero open
in-layer issues, so this battery is the confirming evidence for bar (3) as
well, conditional on the ruling holding.

**Does not settle:** #702 itself (Bus-x86, correctly still open and blocking
that row — this battery adds 25 sequential boots of null evidence to its
existing 0/176 record and does not change its status); the missing
device-count assertion leg on this gate (§3, unchanged, script-only,
not touched here); anything about the R80 ruling's own correctness (this
document assumes it, per the task brief, and does not re-litigate it).

## 6. Claim-lint

```
$ scripts/claim-lint.py --files docs/planning/green-program/nic-bus/CONFIRM-NIC-x86-2026-09-02.md
```
Result recorded in the branch's commit; see the commit message on
`green/nic-x86-2026-09-02`.
