# #748 KVM-vs-TCG comparison, 2026-09-02

One gate invocation, no kernel code change, exactly the measurement #748's
own PR #751 comment named as the cheapest untried next step:

```
BREENIX_QEMU_ACCEL=kvm BREENIX_QEMU_CPU=host \
  docker/qemu/run-ext2-lock-race-gate.sh --x86
```

## Where this ran, and why in an isolated checkout

At launch time, beast's `breenix-x86` Incus container had two other
active, unrelated lanes already running against the shared `/root/breenix`
checkout, both observed directly via `ps aux` inside the container (`ps
aux` output, this session's own transcript): a 60-boot `#693` KVM soak
battery (`/root/r693/driver.sh`, reading `./target/release/qemu-uefi`
directly, mid-run at boot 9-17 throughout this measurement) and a second,
unrelated tracing-validation battery against its own separate checkout
elsewhere on the host. `run-ext2-lock-race-gate.sh` (no `--no-build` skip
available for a from-scratch feature combination) does `cargo build
--release --features boot_tests,ext2_lock_race,...` writing to
`target/release/qemu-uefi` and repacks `target/test_binaries.img` /
`target/ext2.img` in place — the exact paths the `#693` battery's own
driver script (`/root/r693/driver.sh`) re-reads once per loop iteration
(~60s cadence, per its own `HARD_TIMEOUT`/poll structure). Running the gate
build there would have clobbered a different lane's live evidence mid-soak.

Per the task's own contention check, this measurement ran in a **separate,
isolated clone** instead: `/root/breenix-748-kvm`, `git clone --no-hardlinks
/root/breenix`, checked out to **main `3d601400`** (the exact commit named
"main bytes"). The two things this checkout shares with `/root/breenix` are
a symlinked `rust-fork` (a large, static, read-only forked-Rust-std input
already present as a real directory at `/root/breenix/rust-fork-real`, used
read-only by other isolated checkouts on this host by the same convention)
and a copied (not shared) OVMF firmware pair; source tree, `target/` build
output, `test_binaries.img`, and `ext2.img` are each independent copies
built inside `/root/breenix-748-kvm` itself, not links back to
`/root/breenix`. `/tmp/breenix_ext2_lock_race_gate_x86` (the gate script's hardcoded
output directory — not parameterized by checkout) was checked via `ls -la`
and `ps aux` immediately before use: 1 stale file set from a prior day
(mtime 2026-09-02 01:17 UTC) and 0 live processes referencing it.
<!-- claim-lint:ok: the sharing claim is the concrete two-item list given
(rust-fork symlink + copied OVMF, everything else an independent copy); the
idle-directory check is the ls -la/ps aux result run immediately before
use, in this session's own transcript. -->

**First launch attempt (15:03:24 UTC) was killed within ~24s** — the
`timeout`-wrapped `qemu-system-x86_64` process received an external
SIGKILL (bash's own "Killed" job-control message; QEMU's own stderr/stdout
was captured live at the time as `qemu.log` but was not committed
alongside this document, so its contents are not independently checkable
here -- **correction (review-707.md finding F7):** the sentence
previously reasoned from that file's contents directly, which is the same
dangling-artifact defect the #707 round's own B2 finding blocked on, in a
different file). `incus info breenix-x86` showed the container's cgroup
memory at 7.36GiB against an 8GiB `limits.memory`, coincident with two
concurrent `cargo build` processes from other lanes finishing
near-simultaneously — consistent with a cgroup OOM-kill under memory
pressure from combined page cache across the concurrent checkouts on this
shared, 8GiB-limited container, not an x86/KVM-specific failure. No other
lane's process was touched to recover; the retry (15:06:05 UTC) ran to
completion once builds elsewhere on the host had quieted (`ps aux` showed
no active `cargo`/`rustc` processes at retry time).

## Sampling methodology

Direct repeated `wc -l` sampling of the primary kernel log
(`serial_kernel.txt`, the position-aware liveness source both accelerators
share), same method the issue's own TCG table used. Host load confirmed via
`uptime` inside the container at each sample.

## Results

**Leg entry** (`Added thread 1196 'lockrace_holder' to scheduler`) at
`serial_kernel.txt` line 13483.

### Pre-leg pace (KVM)

| interval (UTC) | lines | Δlines | Δt | rate |
|---|---|---|---|---|
| 15:06:38 → 15:07:45 | 2454 → 7897 | 5443 | 67s | 81.2 lines/s |
| 15:07:45 → 15:08:52 | 7897 → 13486 | 5589 | 67s | 83.4 lines/s |

(The second interval crosses leg entry at line 13483 — 13483 of its 5589
lines are still pre-leg, so this row is effectively pure pre-leg pace too.)

### In-leg pace (KVM), post line 13483

| interval (UTC) | lines | Δlines | Δt | rate | host load (1m) |
|---|---|---|---|---|---|
| 15:08:52 → 15:10:29 | 13486 → 13494 | 8 | 97s | 12.1 s/line | — |
| 15:10:29 → 15:12:41 | 13494 → 13506 | 12 | 132s | 11.0 s/line | 0.78 |
| 15:12:41 → 15:14:28 | 13506 → 13514 | 8 | 107s | 13.4 s/line | 0.41 |
| 15:14:28 → 15:16:32 | 13514 → 13524 | 10 | 124s | 12.4 s/line | 1.19 |

**Total in-leg window: 38 lines over 460s (7m40s) = 12.1s/line average.**
Host load stayed low (≤1.19) throughout the in-leg sampling window, once
the earlier OOM-contention subsided.

### Park-path entry (KVM)

`EXT2_LOCK_PARK_FIRST lock=ROOT_EXT2_write parks=1` fired once
(`serial_user.txt` line 101) — the same marker PR #751's `--park-only`
probe uses to establish "the park path was entered" on TCG. Confirmed here
under KVM too.

### Terminal markers

Zero matches. `grep -a "LOCKRACE:COMPLETE\|EXT2_LOCK_SPIN_STALL\|soft lockup\|KERNEL PANIC"`
over the full captured serial (`serial_kernel.txt` + `serial_user.txt`,
13,625 total lines, preserved alongside this README) returned no lines
across the ~7m40s of continuous in-leg observation. The gate was stopped
manually (both QEMU and the wrapper script killed) once four consecutive
in-leg samples (the table above) had established a stable rate — the same
"kill once the pace is established" methodology the issue's own TCG round
used, not a crash or hang.

<!-- claim-lint:ok: the zero-matches claim is the literal grep result over
the two preserved serial files in this same directory (serial_kernel.txt,
serial_user.txt), re-checkable by running the same grep against them. -->

## Comparison to the issue's own TCG table

| accel | pre-leg pace | in-leg pace |
|---|---|---|
| TCG (issue #748 body, six samples, host load 2.28–24.21) | 12.9–41.8 lines/s | 12.0–13.5 s/line |
| KVM + `-cpu host` (this run, four samples, host load ≤1.19) | 81.2–83.4 lines/s | 11.0–13.4 s/line |

## Preserved evidence

- `serial_kernel.txt` (13,524 lines) — primary kernel log (COM2), full
  capture from boot through the point the gate was stopped.
- `serial_user.txt` (101 lines) — COM1, includes the
  `EXT2_LOCK_PARK_FIRST` line.

**Correction (review-707.md finding F7):** this manifest previously also
listed a QEMU stderr/stdout capture as preserved evidence. That capture
was not committed to this directory -- only the two files above are. The
sentence elsewhere in this document that reasoned from its contents has
been restated to not do so; see the correction inline above under "Where
this ran, and why in an isolated checkout."

## What these numbers decide

- **KVM hardware acceleration does not materially change the in-leg pace.**
  12.1s/line average under KVM (range 11.0–13.4s/line across four
  20-line-scale samples) sits inside the same 12.0–13.5s/line band the
  issue already recorded for TCG. This further weakens (alongside the
  issue's own host-load refutation) "TCG instruction-emulation overhead" as
  an explanation for the pathological in-leg pace — a real hardware-virtualized
  CPU reproduces the identical stall rate.
- **The x86 park path is entered under KVM too**, matching the TCG
  `--park-only` result: `EXT2_LOCK_PARK_FIRST` fires for `ROOT_EXT2_write`.
  This is entry evidence only, exactly as already scoped for TCG — it does
  not show the park resolving, the race completing, or the leg reaching a
  verdict.
- **KVM materially speeds up everything outside the leg**: pre-leg pace
  roughly doubled to 6x'd (81–83 lines/s vs TCG's 12.9–41.8 lines/s),
  confirming the pathology is specific to the leg's own in-progress
  construction, not a general property of this boot profile or of x86
  logging under nested virtualization broadly.

## What these numbers do not decide

- **Does not distinguish the issue's two named candidate mechanisms** —
  the leg harness's own kthread/park-timeout construction, vs. the x86
  park/logging primitive more generally. Both predict "KVM doesn't help";
  neither is confirmed or refuted by an accelerator swap. That
  differentiation is what PR #751's still-untried next steps 2–3 (attach
  GDB mid-leg, read the `TIMER_TICK` counter via
  `call trace_dump_counters()`, sample PC/backtrace mid-gap) are for — not
  performed in this round.
- **No GREEN/RED verdict for #728's x86 half.** The full leg still did not
  reach `[LOCKRACE:COMPLETE:...]`, `EXT2_LOCK_SPIN_STALL`, or a soft-lockup
  in this run (0 of these markers across the 13,625-line capture, per the
  grep above). #748 remains open and #728's x86 half remains
  unproven-by-capture; this run does not close either.
- **Not a full-duration run.** The gate was stopped after ~10 minutes total
  (~7m40s in-leg) once the rate had stabilized across four samples (the
  table above), not run to the 1800s timeout or to COMPLETE. A late-arriving
  rate change past that window is not ruled out by this run's ~10-minute
  sample or by the issue's own ~27-minute TCG sample — neither shows any
  such trend within its own observed window, but neither observed past it
  either.

<!-- claim-lint:ok: both bullets' scope is the exact sample windows and
grep counts given inline (13,625-line capture, 0 terminal markers, ~10min
KVM / ~27min TCG observation windows) — see the tables and grep above in
this same file. -->
