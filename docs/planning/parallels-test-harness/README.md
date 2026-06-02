# Parallels Launcher -> Terminal Test Harness

Reusable host-side automation that drives the Breenix
**launcher -> terminal** flow on a fresh Parallels VM and verifies it with real
serial-log evidence. The harness is host-side tooling only; it does not modify
any kernel or userspace source.

## Flow under test

1. Boot Breenix on a fresh Parallels VM via `./run.sh --parallels`.
2. Wait for the window manager (BWM) to be ready.
3. **Double-tap SUPER** -> the launcher (`/bin/blauncher`) opens with
   `APPS[0] = "Terminal"` (which maps to `/bin/bterm`) pre-selected.
4. **Press Enter** -> the terminal (`/bin/bterm`) launches.
   (Optionally type `term` first to filter the list — "Terminal" stays index 0 —
   then Enter.)

A run **passes only** when the serial log shows the launcher opened **and** the
terminal actually launched and initialized. "Launcher opened" alone is a FAIL.

## Proven recipe (encoded in the scripts)

### Boot

- Boot exclusively via `./run.sh --parallels [--no-build]`. It creates a fresh
  epoch-named VM `breenix-<epoch>`, cleans up old `breenix-*` VMs, and **tails
  serial forever** — so it must be run in the background (the smoke script does
  this with `nohup ... &` and kills it on exit).
- Serial log: `/tmp/breenix-parallels-serial.log`. `run.sh` removes it fresh on
  each boot, so any marker found is from the current boot.

### Readiness + warmup

- Readiness marker (grep serial):
  `[bwm] hotkeys: using built-in defaults for early boot`
- After readiness, allow ~60s VirGL warmup before trusting display capture.

### Trigger — double-tap SUPER

Super is PS/2 set-1 **extended** scancode `0xE0 0x5B`:

| Field            | Value     | Notes                                  |
|------------------|-----------|----------------------------------------|
| Extended prefix  | `224`     | `0xE0`                                 |
| Key code         | `91`      | `0x5B` (left GUI / Super)              |
| Hold per tap     | ~40 ms    | press -> release dwell                 |
| Inter-tap gap    | ~150 ms   | must be `< 400 ms` for a "double" tap  |

A **tap** = (optional `0xE0` prefix press) -> press `91` -> hold -> release `91`
-> (release prefix). A **double-tap** = two taps within 400 ms.

`Enter` = scancode `28`.

### Injection mechanism

`prlctl send-key-event <VM> --scancode <N> --event press|release`, wrapped by
the canonical helper `scripts/parallels/inject.sh`:

```bash
export VM=breenix-<epoch>                            # set once for the sequence
scripts/parallels/inject.sh doubletap 91 150 224     # double-Super
scripts/parallels/inject.sh type term                # filter text
scripts/parallels/inject.sh enter                    # press Enter
```

Commands: `tap <code> [hold_ms]`, `key <code> [hold_ms]`, `doubletap <code>
<gap_ms> [prefix]`, `hold <code> <hold_ms> [prefix]`, `type <string>`, `enter`.
The VM name comes from `$VM` (preferred — `export` it once) or the first
positional argument. If `$VM` is empty/unset and no name is passed, `inject.sh`
errors loudly (exit 2) rather than silently no-op'ing.

### Validation oracles (grep serial, in order)

| Stage              | Serial marker                       |
|--------------------|-------------------------------------|
| Launcher opened    | `[spawn] path='/bin/blauncher'`     |
| Terminal launched  | `[spawn] path='/bin/bterm'`         |
| Terminal init'd    | `[bterm] config:`                   |
| (bonus signal)     | `[bterm] spawned child pid=`        |

**PASS requires both** `[spawn] path='/bin/bterm'` **and** `[bterm] config:`.
Honesty rule: never pass on the launcher marker alone — if only the launcher
opened, the run FAILs with that reason.

## Running a single smoke test

```bash
scripts/parallels/launcher-smoke.sh [--no-build] [--keep-vm] \
                                    [--timeout SECS] [--type-filter]
```

| Flag            | Effect                                                      |
|-----------------|-------------------------------------------------------------|
| `--no-build`    | Pass `--no-build` through to `run.sh` (reuse artifacts).    |
| `--keep-vm`     | Don't stop the VM on exit (default: stop with `--kill`).    |
| `--timeout SECS`| Overall budget (default 900).                               |
| `--type-filter` | Type `term` before Enter (default: just Enter).             |

The script:

- launches `run.sh --parallels` in the background (killed on exit),
- polls serial for the readiness marker,
- resolves the running VM name (`prlctl list -a | grep breenix-`),
- waits VirGL warmup, then injects double-Super and Enter,
- writes an evidence dir at
  `logs/parallels-launcher-test/run-<YYYYmmdd-HHMMSS>/` containing the serial
  excerpt, display screenshots (via `scripts/parallels/capture-display.sh`), and
  `result.txt`,
- prints **exactly one** final line: `RESULT: PASS` (exit 0) or
  `RESULT: FAIL: <reason>` (exit 1).

The injection method is a clearly-marked config block at the top of the script
(`SUPER_PREFIX=224`, `SUPER_CODE=91`, `INTER_TAP_MS=150`, `ENTER_CODE=28`). If
the proven trigger changes, edit those values — nothing else needs to change.

> The smoke script contains **no sandbox logic**. Callers must run it
> un-sandboxed (a wrapper passes `dangerouslyDisableSandbox`).

## Running the streak workflow

`.claude/workflows/parallels-launcher-test.js` runs the smoke test
**sequentially** (single VM — never in parallel) and measures stability:

```js
Workflow({ name: 'parallels-launcher-test' })
```

- Up to **15 attempts**, one `agent()` per attempt; each agent runs
  `launcher-smoke.sh` via the Bash tool with `dangerouslyDisableSandbox: true`
  and `run_in_background: true` (a run takes ~8-15 min), polling until it sees a
  `RESULT:` line.
- Tracks the consecutive-PASS streak. **Stops early on a 10-in-a-row streak.**
  On any FAIL it records the streak + evidence and **continues** (to measure
  flakiness) until 15 attempts or the 10-streak is achieved.
- Returns `{ consecutiveGreenAchieved, greenStreakMax, attempts, firstFailure,
  evidenceDir }`.

## Host prerequisites & known limitations

These were root-caused during the build-out (2026-06-01). Read them before
running, especially for unattended runs.

### The macOS screen MUST be unlocked

`prlctl send-key-event` reaches the guest only when the Mac console is
**unlocked**. With the console locked, Parallels detaches the VM window and
**silently drops** every injected keystroke: `send-key-event` returns `rc=0`
but the key never lands in the guest (proven functionally — injecting `=` into
the Bounce demo changed nothing; no hotkey `[spawn]` appeared).

This is **not** a TCC / Accessibility / Input-Monitoring permissions issue and
there is **no permissions grant that fixes it**. Injection goes through the
virtual xHCI HID via `prl_disp_service`, not through macOS CGEvent/`CGPostEvent`
— so TCC is never consulted. A locked console simply has no presented VM
console for the HID stream to attach to.

There is **no non-interactive unlock bypass**. The smoke script preflights this
and refuses to run on a locked Mac:

```bash
# One-line lock check (exit 0 = locked, 1 = unlocked):
python3 -c "import Quartz,sys; d=Quartz.CGSessionCopyCurrentDictionary(); sys.exit(0 if (d and d.get('CGSSessionScreenIsLocked')) else 1)"
```

On a locked screen the script prints
`RESULT: FAIL: macOS screen is locked — ...` and exits 1 rather than producing
a misleading boot/injection failure.

### Unattended / overnight runs (testing at scale)

For runs without a human present:

1. **Disable auto-lock.** System Settings -> Lock Screen ->
   "Require password after screen saver begins/display is turned off" = **Never
   / Off**. Otherwise the screen re-locks mid-run and injection silently dies.
2. **Keep the display awake** with `caffeinate -d` for the run's duration. The
   smoke script starts `caffeinate -d &` automatically (and kills it on exit),
   but disabling auto-lock is still required because `caffeinate` prevents sleep,
   not the lock that fires on display-off.

These two together are the requirement for driving the launcher flow at scale
unattended.

### QEMU is NOT a viable substitute for this flow

QEMU was evaluated as a lock-independent alternative (it injects keys via its
own monitor, not macOS events). It does **not** work for this specific flow, for
two independent reasons:

- **BWM never starts on QEMU.** BWM's ARM64 path requires the **VirGL 3D
  compositor**, which is Parallels-specific and absent on the QEMU build here.
  With no compositor, BWM does not come up, so there is nothing to drive.
- **SUPER is never observed on QEMU.** The double-tap-Super hotkey reads
  `SUPER_PRESSED` exclusively from the **USB-HID / xHCI** driver, which never
  enumerates on QEMU. QEMU's `virtio-keyboard` MMIO driver never tracks the
  Super modifier, so the gesture cannot be recognized even if keys arrive.

Making QEMU viable would require **kernel changes** (a software-compositor
fallback for BWM, plus a `virtio-keyboard`->SUPER bridge) — explicitly out of
scope for this host-side harness.

For reference, the working QEMU ARM64 boot recipe is `-M virt,gic-version=3
-cpu max` (run.sh's `cortex-a72` hangs). `run.sh` exposes a QEMU monitor on
`tcp:127.0.0.1:4444` and a QMP socket at `/tmp/breenix-qmp.sock`, which is how
keys would be injected if the two kernel gaps above were closed.

### If the injection method changes

A separate effort may change the injection primitive. If it does (different key,
non-extended encoding, or a new mechanism entirely), update the config block at
the top of `scripts/parallels/launcher-smoke.sh` (`SUPER_PREFIX`, `SUPER_CODE`,
`INTER_TAP_MS`, `ENTER_CODE`) and, if the primitive itself changes, the
`press`/`release`/`tap` logic in `scripts/parallels/inject.sh`.

## Exit criterion

The harness is considered green when the workflow reports
**10 consecutive `RESULT: PASS` runs** (`consecutiveGreenAchieved: true`,
`greenStreakMax >= 10`).
```
