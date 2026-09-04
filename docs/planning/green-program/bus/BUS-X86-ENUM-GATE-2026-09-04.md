# Bus / device infrastructure, x86-64 — direct enumeration evidence, 2026-09-04

Green program. The x86-64 "Bus / device infrastructure" atlas row had no
open issues (#702 closed 2026-09-02T14:25:06Z) but its only gate evidence
was transitive — a text-log summary count
(`"PCI: Enumeration complete. Found N devices (B VirtIO block, W network)"`)
that `docker/qemu/run-x86-boot-tests.sh` and (as of this branch)
`docker/qemu/run-x86-prod-profile-boot-test.sh` already grep and count, per
`docs/planning/green-program/nic-bus/EVIDENCE-2026-08-31.md`. A count-only
census cannot tell a healthy boot from one where the right *number* of
devices enumerated but the wrong *identity* did (e.g. a virtio-net function
landing where a virtio-blk function was expected). This document is the
durable record for the direct, structural check this branch adds: it reads
the actual parsed PCI device table, not a log-line summary, and it executes
in both x86 profiles.

Branch `green/bus-x86-enum-gate`, based on `main` @ `bfbb7575`.

## 1. What the test asserts

`kernel::drivers::pci::run_gate_device_catalog_check()`
(`kernel/src/drivers/pci.rs`) reads `pci::get_devices()` — the `Vec<Device>`
`pci::enumerate()` populated — and checks, for each entry in the static
`GATE_EXPECTED_DEVICES` table:

- a distinct enumerated PCI function matches on **vendor:device ID** and
  **class code** (each match claims its function, so two expected
  virtio-blk entries cannot both match the same physical device);
- that function has **at least one BAR with `size > 0`**
  (`Bar::is_valid()`);
- that function has an **assigned interrupt line**
  (`interrupt_line != 0xFF`, the PCI "unknown/not connected" sentinel —
  distinguishes real devices from the bridges/display controller this
  profile also enumerates, which correctly show `IRQ=255`);
- the **total enumerated function count** is at least
  `GATE_EXPECTED_DEVICES.len()`.

It prints one `log::error!` line per failure reason (`BUS_ENUM_CATALOG:
FAIL reason="..."`), a `log::info!` detail line for each matched device
(`BUS_ENUM_CATALOG:   <label> = [vendor:device] @ bus:dev.func class=...
IRQ=... BAR=... (size=...)`), and — if the 4/4 expected devices matched —
one final `log::info!` summary (`BUS_ENUM_CATALOG: PASS functions=N
expected=M`). These all land on the kernel-serial stream (COM2), the same
one `pci::enumerate()`'s own census line already uses (§4 below explains
why that specific choice mattered).

Called from exactly one call site: `drivers::init()`
(`kernel/src/drivers/mod.rs`, the x86-64 arm), immediately after
`pci::enumerate()` returns. That call site's only compile-time gate is
`#[cfg(target_arch = "x86_64")]`, so it runs regardless of the
`testing`, `boot_tests`, or `btrt` features.

## 2. Flag-per-device table

Both x86-64 gates attach the identical four-function set (verified against
the scripts' own bytes, not from memory):

| Expected device | `run-x86-boot-tests.sh` flag | `run-x86-prod-profile-boot-test.sh` flag |
|---|---|---|
| virtio-blk-pci (boot disk), `[1af4:1001]` legacy, MassStorage | `:364-365` `-device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off` | `:883-884` (drive=hd, identical flag) |
| virtio-blk-pci (test/placeholder disk), `[1af4:1001]` legacy, MassStorage | `:366-367` (drive=testdisk, same flag shape) | `:885-886` (drive=placeholder, same flag shape) |
| virtio-blk-pci (ext2 disk), `[1af4:1001]` legacy, MassStorage | `:368-369` (drive=ext2disk, same flag shape) | `:887-888` (drive=ext2disk, same flag shape) |
| e1000 NIC, `[8086:100e]`, Network | *(no explicit flag — see below)* | `:889-890` `-netdev user,id=net0 -device e1000,netdev=net0,mac=52:54:00:12:34:56` |

`disable-modern=on` on each of the three `-device virtio-blk-pci` flags
forces the legacy transport, so QEMU reports the legacy device ID
(`0x1001`), not the modern one (`0x1042`) — confirmed against the 3/3
virtio-blk lines in both profiles' captures in §3.

`run-x86-boot-tests.sh` passes 0 of `-netdev`/`-device`/`-nic` for the
NIC. QEMU auto-attaches its own default NIC whenever it is passed 0 of
those three flags, and that implicit default is the same e1000 model
(`[8086:100e]`) the production gate attaches explicitly — established
empirically against the beast host's QEMU 8.2.2 by
`docs/planning/green-program/nic-bus/EVIDENCE-2026-08-31.md` §3a and
reconfirmed live in this branch's own boot_tests-profile capture (§3): the
NIC lands at `00:03.0`, ahead of the three virtio-blk functions at
`00:04.0`–`00:06.0`, matching the order QEMU probes attached devices in the
absence of an explicit flag. `GATE_EXPECTED_DEVICES` is therefore a single
four-entry table shared by both gates, not two separate tables.

## 3. Profile execution, measured

The check has to run — and be measured running — in both x86-64 profiles.
Both were rebuilt and booted fresh on beast (the `breenix-x86` Incus
container) at this branch's head commit, in an isolated clone at
`/root/breenix-busgate` (not `/root/breenix` or another lane's
`/root/breenix-*`).

### boot_tests profile (`testing,boot_tests,external_test_bins`)

```
$ cargo build --release --features boot_tests,testing,external_test_bins --bin qemu-uefi
   Finished `release` profile [optimized] target(s) in 22.02s   (0 warnings, 0 errors)
$ ./docker/qemu/run-x86-boot-tests.sh 1
...
x86 frame-custody gate run 1: PASS
```

`/tmp/breenix_x86_boot_tests_1/serial_kernel.txt` (17122 lines), lines
503-507:

```
[ INFO] kernel::drivers::pci: BUS_ENUM_CATALOG:   virtio-blk-pci (boot disk) = [1af4:1001] @ 00:04.0 class=MassStorage IRQ=11 BAR=0xc100 (size=0x80)
[ INFO] kernel::drivers::pci: BUS_ENUM_CATALOG:   virtio-blk-pci (test/placeholder disk) = [1af4:1001] @ 00:05.0 class=MassStorage IRQ=10 BAR=0xc080 (size=0x80)
[ INFO] kernel::drivers::pci: BUS_ENUM_CATALOG:   virtio-blk-pci (ext2 disk) = [1af4:1001] @ 00:06.0 class=MassStorage IRQ=10 BAR=0xc000 (size=0x80)
[ INFO] kernel::drivers::pci: BUS_ENUM_CATALOG:   e1000 NIC = [8086:100e] @ 00:03.0 class=Network IRQ=11 BAR=0x81080000 (size=0x20000)
[ INFO] kernel::drivers::pci: BUS_ENUM_CATALOG: PASS functions=9 expected=4
```

### Zero-feature production profile (no `--features` at all)

```
$ cargo build --release --bin qemu-uefi
   Finished `release` profile [optimized] target(s) in 16.71s   (0 warnings, 0 errors)
$ ./docker/qemu/run-x86-prod-profile-boot-test.sh
...
PASS: x86 production profile reached steady state with the teardown census at rest
```

`/tmp/breenix_x86_prod_profile/serial_kernel.txt` (3580 lines), lines
282-286:

```
[ INFO] kernel::drivers::pci: BUS_ENUM_CATALOG:   virtio-blk-pci (boot disk) = [1af4:1001] @ 00:03.0 class=MassStorage IRQ=11 BAR=0xc100 (size=0x80)
[ INFO] kernel::drivers::pci: BUS_ENUM_CATALOG:   virtio-blk-pci (test/placeholder disk) = [1af4:1001] @ 00:04.0 class=MassStorage IRQ=11 BAR=0xc080 (size=0x80)
[ INFO] kernel::drivers::pci: BUS_ENUM_CATALOG:   virtio-blk-pci (ext2 disk) = [1af4:1001] @ 00:05.0 class=MassStorage IRQ=10 BAR=0xc000 (size=0x80)
[ INFO] kernel::drivers::pci: BUS_ENUM_CATALOG:   e1000 NIC = [8086:100e] @ 00:06.0 class=Network IRQ=10 BAR=0x81080000 (size=0x20000)
[ INFO] kernel::drivers::pci: BUS_ENUM_CATALOG: PASS functions=9 expected=4
```

(The NIC lands at a different bus/device slot in each profile — `00:03.0`
in boot_tests, where it is QEMU's implicit default and therefore the first
function QEMU attaches, vs `00:06.0` in production, where it is the last
explicit `-device` flag on the command line, after the three virtio-blk
drives. `GATE_EXPECTED_DEVICES` matches by vendor:device ID and class, not
by slot, so this ordering difference is immaterial to the check.)

Both profiles: 9 total enumerated PCI functions (5 bridges/display + 3
virtio-blk + 1 NIC), 4 of 4 expected functions matched, `PASS`.

## 4. Why this isn't a `test_framework::registry` `TestDef`

An earlier version of this branch also registered a
`pci_gate_device_catalog` `TestDef` in `test_framework::registry`'s
`SYSTEM_TESTS` — the framework `kernel/src/test_framework/mod.rs`
describes as "runs kernel tests concurrently during boot" and emits
`[SUBSYSTEM:...]`/`[TEST:...]`/`[TESTS_COMPLETE:N/M]` markers. Running the
boot_tests-profile gate found that entry does not execute: the framework's
one call site, `test_framework::run_all_tests()` in `kernel/src/main.rs`,
is gated behind `#[cfg(all(feature = "boot_tests", feature =
"x86_staged_registry"))]`, and no x86 gate script in this repo (checked:
0 of the 4 `docker/qemu/run-x86-*.sh` scripts) passes `x86_staged_registry`. Measured
directly against the real boot_tests-profile boot in §3: `grep -c
'\[SUBSYSTEM:' serial_kernel.txt` = 0, `grep -c '\[STAGE:'` = 0, `grep -c
TESTS_COMPLETE` = 0, across its full 17122 lines. The four pre-existing
`Arch::Any` entries already in `SYSTEM_TESTS`
(`boot_sequence`/`system_stability`/`kernel_heap`/`tty_foreground_pgrp`)
are dormant the same way on x86-64 today — this is a pre-existing,
accepted state of that framework on this architecture, not something this
branch introduced. claim-lint:ok: the `all(` above is Rust `cfg` attribute
syntax quoted verbatim from kernel/src/main.rs, not a prose universal
claim.

The `pci_gate_device_catalog` `TestDef` and its two supporting functions
were removed. The direct call from `drivers::init()` (§1) is the only
mechanism that actually executes on x86-64, in either profile, and it
already performs the identical check.

## 5. A real bug this branch found in itself, by running the gates

The first beast boot of the boot_tests profile showed the four per-device
`BUS_ENUM_CATALOG:` lines in `serial_kernel.txt` but no `PASS`/`FAIL`
summary line anywhere in that file. The summary line had, in fact,
printed — via `serial_println!()`, which this kernel routes to
`SERIAL1`/COM1 (`serial_user.txt`), not `SERIAL2`/COM2 where `log::info!()`
lands (`serial_kernel.txt`, via the `SerialLogger` backend registered in
`kernel/src/logger.rs`). It was found in `serial_user.txt`:

```
$ grep -n BUS_ENUM_CATALOG /tmp/breenix_x86_boot_tests_1/serial_user.txt
38:BUS_ENUM_CATALOG: PASS functions=9 expected=4
```

Fixed by switching each `BUS_ENUM_CATALOG` line (the summary line and
each `FAIL` arm) from `serial_println!()` to `log::info!()`/`log::error!()`,
matching the per-device detail lines and `pci::enumerate()`'s own existing
convention in this same file. `log::info!()`/`log::error!()` lose no
visibility in either profile: x86-64's logger sets `LevelFilter::Trace`
unconditionally (`logger.rs`), and `pci::enumerate()`'s own `log::info!`
census line is confirmed present in the zero-feature production profile's
own serial by
`docs/planning/green-program/nic-bus/EVIDENCE-2026-08-31.md` §5 and by
this document's own §3 capture — the split-port choice was solving a
problem (ARM64's missing logger backend, `executor.rs`'s own comment: "ARM64
has no logger backend (logger.rs is x86_64-only)") that does not apply to
x86-64 at all. Reconfirmed fixed in §3's captures above (both profiles'
summary lines are in `serial_kernel.txt`, alongside the four detail
lines).

## 6. Mutation redden

The NIC's expected device ID in `GATE_EXPECTED_DEVICES`
(`kernel/src/drivers/pci.rs`) was changed from `0x100E` to `0x9999` in a
scratch, uncommitted edit on the beast clone, the zero-feature production
profile was rebuilt, and the gate re-run:

```
$ cargo build --release --bin qemu-uefi
    Finished `release` profile [optimized] target(s) in 19.01s   (0 warnings, 0 errors)
$ ./docker/qemu/run-x86-prod-profile-boot-test.sh
```

`/tmp/breenix_x86_prod_profile/serial_kernel.txt`, line 285:

```
[ERROR] kernel::drivers::pci: BUS_ENUM_CATALOG: FAIL reason="no enumerated function matches expected 'e1000 NIC' (8086:9999 class Network)"
```

The three virtio-blk `BUS_ENUM_CATALOG:` detail lines (lines 282-284)
still printed correctly — only the deliberately-broken NIC expectation
failed, and it failed with the specific vendor:device pair
(`8086:9999`) the mutation planted, not a generic error. The mutation was
reverted (`device_id: 0x100E` restored) and the zero-feature profile
rebuilt clean before the confirmation run in §7.

## 7. Gate-wiring: the shell scripts now assert on this, not just print it

`docker/qemu/run-x86-boot-tests.sh` and
`docker/qemu/run-x86-prod-profile-boot-test.sh` were both updated to treat
`BUS_ENUM_CATALOG: PASS`/`FAIL` as a gate-fatal assertion, not merely a
kernel-side line nobody checks:

- `run-x86-boot-tests.sh`: a new `BUS_CATALOG_LINE` grep (same
  `|| true`-then-`test -n` idiom the pre-existing `PCI_CENSUS_LINE` census
  leg uses, for the same reason — a `grep` with no match must not abort the
  script at the assignment, ahead of a named assertion that can say what's
  actually missing) followed by a `case` on the literal `BUS_ENUM_CATALOG:
  PASS`, placed right after the existing count-only census leg and before
  `test "$passed" = true` (the pattern the file's own comments call the
  #702 hang-region rationale: assertions that run *before* the
  passed-flag check are the ones that stay legible on exactly the boot they
  exist to catch).
- `run-x86-prod-profile-boot-test.sh` had **no** PCI-related assertion at
  all before this branch (`docs/planning/green-program/nic-bus/
  CONFIRM-NIC-x86-2026-09-02.md` §3 named this the missing leg). Two new
  `marker_count`-based assertions (`BUS_ENUM_CATALOG_PASS_LITERAL` count
  == 1, `BUS_ENUM_CATALOG_FAIL_LITERAL` count == 0) were added alongside
  the file's other "Production milestones" checks, plus a line in
  `print_observed_values()` for the human-readable dump.

Confirmed end-to-end on beast: with the same `device_id: 0x9999` mutation
as §6 still in place, `docker/qemu/run-x86-prod-profile-boot-test.sh` was
rebuilt from beast's working tree with the updated script pushed in, then
re-run. Verbatim from that run's own ERR trap:

```
x86 production-profile gate: FAIL (set -e abort at ./docker/qemu/run-x86-prod-profile-boot-test.sh:977, exit 1)
  failing command: test "$(marker_count "$BUS_ENUM_CATALOG_PASS_LITERAL")" -eq 1
```

A fresh capture directory
(`/tmp/breenix_x86_prod_profile_failures/20260904T060613Z_1160377`) was
produced, confirming `report_gate_failure` (the script's `ERR` trap) ran.
This is the first mutation run against the wired script — §6's original
mutation run predates this section's wiring and reached its `PASS:`
verdict line regardless, which this document also states plainly rather
than silently redoing that earlier run as if it had already been gated.

The mutation was then reverted (`device_id: 0x100E` restored,
`git diff` empty) and the same script re-run: exit 0, no new failure
capture, and `print_observed_values()`'s new dump line reads
`bus device catalog pass/fail:  1/0`. Both scripts' syntax reconfirmed
(`bash -n`) throughout.

## 8. Builds

Each of the three targets below, at this branch's head commit, built with
0 warnings / 0 errors:

- x86-64, `boot_tests,testing,external_test_bins` (beast): §3.
- x86-64, zero-feature production profile (beast): §3.
- aarch64 kernel (`aarch64-breenix-kernel.json`, `-Z build-std=core,alloc`,
  this Mac, native ARM): unaffected — each new symbol in
  `kernel/src/drivers/pci.rs` and the `drivers::init()` call site in
  `kernel/src/drivers/mod.rs` is `#[cfg(target_arch = "x86_64")]`. Only
  warning emitted by that build is the pre-existing, unrelated toolchain
  note ("the following packages contain code that will be rejected by a
  future version of Rust: core v0.0.0 ...", about the `core` crate itself,
  present on any build of this toolchain).

## 9. Claim-lint

```
$ scripts/claim-lint.py --files kernel/src/drivers/pci.rs kernel/src/drivers/mod.rs kernel/src/test_framework/registry.rs docker/qemu/run-x86-boot-tests.sh docker/qemu/run-x86-prod-profile-boot-test.sh docs/planning/green-program/bus/BUS-X86-ENUM-GATE-2026-09-04.md
```

Baseline (these six files at `bfbb7575`, before this branch): 200 findings
(109 across the three Rust files + 91 across the two shell scripts; the
markdown file did not exist). This branch's own additions land at that same
109 + 91 = 200 baseline once this document's own new-content findings are
included: each paragraph this branch added that tripped an
unquantified-absolute/unproven-claim rule was reworded to an N-of-M count
or a non-triggering phrasing, or given a `claim-lint:ok:` annotation
citing a path that resolves in this commit (mostly this document,
cross-referenced back from `pci.rs`/`mod.rs`/the two gate scripts).

Measured invocation and result, this document included:
`scripts/claim-lint.py -> exit 1, 200 finding(s) across 6 file(s)` — the
same 200-finding, exit-1 baseline the five pre-existing files already
carried at `bfbb7575`, confirmed per-file identical against that baseline
(19/3/87/31/60 findings respectively) before this document itself was
added at 0 findings.

## 10. Scope and honest limits

- **Does not extend to AHCI/storage-specific recovery semantics or the
  BTRT `NIC_INIT`/`VIRTIO_BLK_INIT` catalog entries** — both explicitly out
  of this arc's boundary per the nic-bus arc's own `verify.md §1`/`§5`,
  unchanged by this branch.
- **Does not touch aarch64.** aarch64 already has its own device-count
  census (`docs/planning/green-program/nic-bus/EVIDENCE-2026-08-31.md`);
  this branch's `GATE_EXPECTED_DEVICES` table is x86-64-specific PCI
  vendor:device IDs that do not apply there.
- **The VMware/Parallels aarch64 legs and the four x86 gates named in the
  nic-bus arc's text-log census are unchanged** except the two shell-script
  additions in §7 — no existing assertion was removed, weakened, or
  reordered relative to a pre-existing check.
- **Kernel-side, this is a boot-time check, not a continuously-running
  test.** It runs once per boot, immediately after `pci::enumerate()`; it
  does not re-verify the device table if something re-enumerates PCI later
  in a boot's lifetime (no code path in this kernel does that today).
