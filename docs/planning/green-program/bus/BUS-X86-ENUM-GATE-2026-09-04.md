# x86-64 PCI enumeration facts, and the two gates that hold the expectations

Green-program "Bus / device infrastructure" row. Round 2, rewritten from
scratch after the round-1 review of this branch (its findings are cited
below by their F-numbers, F1 through F18) and coordinator ruling R127.

<!-- claim-lint:ok: the removals are readable in kernel/src/drivers/pci.rs and kernel/src/drivers/mod.rs; the derivations that replaced them in docker/qemu/run-x86-boot-tests.sh and docker/qemu/run-x86-prod-profile-boot-test.sh. -->
The shape R127 binds: **the kernel prints facts, the scripts hold
expectations.** Round 1 put a hand-transcribed copy of two shell scripts'
QEMU device flags inside the kernel and had the kernel print its own
PASS/FAIL; that duplicate had no sync mechanism (F8), its BAR message
claimed more than its predicate checked (F7), and it logged an `[ERROR]`
line on any x86 boot with a different device topology (F10). None of that
survives. The kernel now emits only transcriptions of what it parsed out of
PCI config space, and each gate script derives what it should see from its
own `-device`/`-netdev` bytes.

<!-- claim-lint:ok: every cited line number was read back out of kernel/src/drivers/pci.rs, kernel/src/drivers/mod.rs, docker/qemu/run-x86-boot-tests.sh and docker/qemu/run-x86-prod-profile-boot-test.sh at the pushed HEAD. -->
Branch `green/bus-x86-enum-gate`. Every line number in this document was
re-derived from the pushed HEAD after the last source edit.

---

## 1. What the kernel prints

`kernel/src/drivers/pci.rs:1408`, `pub fn dump_enumerated_functions()`,
called from `kernel/src/drivers/mod.rs:44` immediately after
`pci::enumerate()`, in the x86-64 arm of `drivers::init()`.

One line per enumerated function, in enumeration order:

```
PCI_FN <bus>:<dev>.<fn> <vendor>:<device> class=<cc>/<sub> bar0=<addr>/<size> irq=<line>
```

then exactly one

```
PCI_FN_TOTAL <n>
```

<!-- claim-lint:ok: the field list is the format string in kernel/src/drivers/pci.rs, and the parse sites it transcribes are probe_device() and decode_bar() in that same file. -->
Every field is a transcription of a value `enumerate()` already parsed out
of live config space: vendor/device from config dword 0x00, class/subclass
from 0x08, `interrupt_line` from 0x3C, and BAR 0's address and size from
`decode_bar()`. `bar0=` prints BAR index 0's `address`/`size` verbatim
(`0x0/0x0` when BAR 0's decoded size is 0); `irq=` prints the raw
`interrupt_line` byte, whose `0xff` is the PCI "unknown / not connected"
sentinel.

<!-- claim-lint:ok: kernel/src/drivers/pci.rs carries no expected-device table and no log::error! in this function; section 5b below is a measured 8-function boot whose only effect on this output was a shorter list. -->
There is no expected-device set in the kernel, no PASS/FAIL verdict, and no
`log::error!` on any boot. A boot with a different device topology -- an
ordinary `./run.sh --x86` without the optional test-binaries or ext2 images,
which was the F10 complaint -- prints a shorter list and nothing else.

**Print path.** `serial_println!` -> `kernel/src/serial.rs::_print` ->
`SERIAL1` = COM1. `kernel/src/serial.rs` carries exactly one cfg, the
crate-level `#![cfg(target_arch = "x86_64")]` at line 1; `_print` has no
feature gate and no log-level filter anywhere on its path. This is the same
kind of unconditional path as the `serial_println!` sibling line
`"[drivers] Found N PCI devices"` that
`docs/planning/green-program/nic-bus/EVIDENCE-2026-08-31.md` section 5
turned to when a `log::info!` leg went invisible in a shipping profile. It
is measured here, not assumed: section 4 below shows the same lines in the
zero-feature production profile and in the `boot_tests` profile.

Both gate scripts capture COM1 and COM2 to `$OUTPUT_DIR/serial_user.txt`
and `$OUTPUT_DIR/serial_kernel.txt` and grep across `serial_*.txt`, so both
read this stream. Measured in both clean runs: the `PCI_FN` lines appear in
`serial_user.txt` and the `grep -c 'PCI_FN'` of `serial_kernel.txt` is 0 in
both profiles.

There is no `assign_bars()` step to sequence after on x86-64:
`pci::assign_bars()` is `#[cfg(target_arch = "aarch64")]`
(`kernel/src/drivers/pci.rs:1225-1226`) and its only call site is the
aarch64 arm of `drivers::init()` (`kernel/src/drivers/mod.rs:135`). On
x86-64, "after enumeration" is also "after the last BAR value the kernel
writes before driver init".

---

## 2. What each script expects, derived from its own bytes

<!-- claim-lint:ok: the derivations are in docker/qemu/run-x86-boot-tests.sh and docker/qemu/run-x86-prod-profile-boot-test.sh at the line numbers cited below. -->
Each script maps a QEMU device model to the identity the kernel's `PCI_FN`
line prints for it, in one table, and derives *how many* of each to expect
by counting its own flag lines -- the same way the pre-existing
`EXPECTED_VIRTIO_BLOCK` leg does
(`docker/qemu/run-x86-boot-tests.sh:524`), and for the same reason
(#549 / #551 / `[[gate-target-fidelity-528]]`: a census, never a
hand-pinned list).

The two identities, identical in both scripts:

| QEMU device model | kernel `PCI_FN` identity | why |
|---|---|---|
| `virtio-blk-pci` with `disable-modern=on` | `1af4:1001 class=01/00` | the legacy VirtIO transport reports the legacy block-device ID 0x1001 (`VIRTIO_BLOCK_DEVICE_ID_LEGACY`), not the modern 0x1042; PCI class 0x01 MassStorage, subclass 0x00 |
| `e1000` | `8086:100e class=02/00` | Intel e1000; PCI class 0x02 Network, subclass 0x00 |

The subclass is part of the match on purpose. The PIIX3 IDE controller both
gates enumerate is also class 0x01, at subclass 0x01 (`class=01/01`,
measured below), so a match on the class byte alone would have counted it
as a fourth virtio-blk function. This is the detail
round-1's topology sentence blurred (F5).

### 2a. `docker/qemu/run-x86-boot-tests.sh`

<!-- claim-lint:ok: produced by grep -nE on docker/qemu/run-x86-boot-tests.sh for the anchored device/drive/net flag pattern, 7/7 matching lines quoted. -->
Every device/drive/net flag line in the file at HEAD, quoted verbatim with
its line number:

```
364:        -drive "if=none,id=hd,format=raw,readonly=on,file=$BREENIX_ROOT/$UEFI_IMG" \
365:        -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
366:        -drive "if=none,id=testdisk,format=raw,readonly=on,file=$BREENIX_ROOT/target/test_binaries.img" \
367:        -device virtio-blk-pci,drive=testdisk,disable-modern=on,disable-legacy=off \
368:        -drive "if=none,id=ext2disk,format=raw,readonly=on,file=$BREENIX_ROOT/target/ext2.img" \
369:        -device virtio-blk-pci,drive=ext2disk,disable-modern=on,disable-legacy=off \
372:        -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
```

Which resolves to: three `-device virtio-blk-pci` flags -> three
`1af4:1001 class=01/00`; the three `-drive` lines are backing stores and
add no PCI function of their own; `isa-debug-exit` is an ISA device, not on
the PCI bus, and is correctly absent from both expectations; and, because
the listing above is the complete set and carries no `-net`/`-netdev`/`-nic`
option, one `8086:100e class=02/00` by the implicit-default-NIC rule below.

Derivations:

- `524` (pre-existing, reused unchanged):
  `EXPECTED_VIRTIO_BLOCK=$(grep -cE -- '^[[:space:]]*-device virtio-blk-pci,drive=' "${BASH_SOURCE[0]}")` -> **3** at HEAD.
- `609-615`: `EXPECTED_E1000_FLAGS` counts `^[[:space:]]*-device e1000,`
  (**0** at HEAD), `NIC_OPTION_FLAGS` counts
  `^[[:space:]]*-(net|netdev|nic)[[:space:]]` (**0** at HEAD), and the
  branch at 611-615 resolves `EXPECTED_E1000` to **1**.

<!-- claim-lint:ok: the 8086:100e reading is the capture quoted in section 4a of this document; the branch is in docker/qemu/run-x86-boot-tests.sh. -->
**Where the implicit e1000 comes from.** QEMU attaches its own default NIC
for `-machine pc` whenever no `-net`/`-netdev`/`-nic` option is given;
`-nic none` is what suppresses it, and this script passes neither. The
model of that default on the beast host's QEMU 8.2 is measured, not
assumed: `8086:100e` at `00:03.0`, in the clean capture quoted in section 4.
The same rule is what the pre-existing `CENSUS_NETWORK -ge 1` leg (line
550) already relies on; the new leg tightens it from ">= 1 network device"
to "exactly one 8086:100e". The branch's other arm is live, not
decoration: an added explicit `-device e1000,netdev=...` raises the count
with the flags, and any added `-net`/`-netdev`/`-nic` option retires the
implicit rule so the count follows the explicit e1000 flags alone.

Identity table at `587-588`; assertions at:

| line | assertion |
|---|---|
| 622 | `test -n "$PCI_FN_LINES"` |
| 625 | `test -n "$PCI_FN_TOTAL_LINE"` |
| 633 | `test "$PCI_FN_LINE_COUNT" -eq "$PCI_FN_TOTAL_VALUE"` |
| 642 | `test "$EXPECTED_VIRTIO_BLOCK" -ge 1` |
| 643 | `test "$EXPECTED_E1000" -ge 1` |
| 644 | `test "$MATCHED_VIRTIO_BLK" -eq "$EXPECTED_VIRTIO_BLOCK"` |
| 645 | `test "$MATCHED_E1000" -eq "$EXPECTED_E1000"` |
| 668 | `test -n "$PCI_FN_FACT_VIOLATIONS"` |
| 669 | `test "$PCI_FN_FACT_VIOLATIONS" -eq 0` |

### 2b. `docker/qemu/run-x86-prod-profile-boot-test.sh`

<!-- claim-lint:ok: produced by grep -nE on docker/qemu/run-x86-prod-profile-boot-test.sh for the anchored device/drive/net flag pattern, 9/9 matching lines quoted. -->
Every device/drive/net flag line in the file at HEAD, quoted verbatim with
its line number:

```
953:    -drive "if=none,id=hd,format=raw,readonly=on,file=$BREENIX_ROOT/$UEFI_IMG" \
954:    -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
955:    -drive "if=none,id=placeholder,format=raw,readonly=on,file=$OUTPUT_DIR/placeholder.img" \
956:    -device virtio-blk-pci,drive=placeholder,disable-modern=on,disable-legacy=off \
957:    -drive "if=none,id=ext2disk,format=raw,readonly=on,file=$BREENIX_ROOT/target/ext2.img" \
958:    -device virtio-blk-pci,drive=ext2disk,disable-modern=on,disable-legacy=off \
959:    -netdev user,id=net0 \
960:    -device e1000,netdev=net0,mac=52:54:00:12:34:56 \
963:    -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
```

Which resolves to: three `-device virtio-blk-pci` flags -> three
`1af4:1001 class=01/00`; one `-device e1000` flag -> one
`8086:100e class=02/00`; the `-netdev` line is a backend, and its presence
is what retires the implicit-NIC rule on this script; `isa-debug-exit` is
ISA, not PCI, and is correctly absent.

Derivations: `264` -> `EXPECTED_VIRTIO_BLK` = **3**; `265` ->
`EXPECTED_E1000_FLAGS` = **1**; `278` -> `NIC_OPTION_FLAGS` = **1**;
branch at `279-283` -> `EXPECTED_E1000` = **1**. Identity table at
`262-263`; observed-values row at `805`.

Assertions:

| line | assertion |
|---|---|
| 1030 | `test -n "$PCI_FN_LINES"` |
| 1031 | `test "$(marker_count 'PCI_FN_TOTAL ')" -eq 1` |
| 1036 | `test "$PCI_FN_LINE_COUNT" -eq "$PCI_FN_TOTAL_VALUE"` |
| 1040 | `test "$EXPECTED_VIRTIO_BLK" -ge 1` |
| 1041 | `test "$EXPECTED_E1000" -ge 1` |
| 1042 | `test "$(marker_count "$PCI_FN_VIRTIO_BLK_ID")" -eq "$EXPECTED_VIRTIO_BLK"` |
| 1043 | `test "$(marker_count "$PCI_FN_E1000_ID")" -eq "$EXPECTED_E1000"` |
| 1065 | `test -n "$PCI_FN_FACT_VIOLATIONS"` |
| 1066 | `test "$PCI_FN_FACT_VIOLATIONS" -eq 0` |

### 2c. The per-function fact predicate

`PCI_FN_FACT_VIOLATIONS` (boot_tests `655-667`, production `1052-1064`) is
an `awk` block over the matched `PCI_FN` lines. For each one it reads
`bar0=<addr>/<size>` and `irq=<line>` out of the text and counts a
violation when the address is `0x0`, the size is `0x0`, or the interrupt
line is `0xff` -- and also when any of the three fields is missing from the
line at all.

<!-- claim-lint:ok: the predicate is the awk block in docker/qemu/run-x86-boot-tests.sh and docker/qemu/run-x86-prod-profile-boot-test.sh at the line ranges named just above. -->
Both BAR halves are checked because round 1's predicate was
`Bar::is_valid()` (`size > 0`) while its message said "no BAR decoded
non-zero" (F7): a BAR with `size > 0` and `address == 0` -- the exact state
an unassigned BAR is in -- passed a check whose own words said it should
not. The assertion now says what it checks and checks what it says.

### 2d. Failure routing

<!-- claim-lint:ok: 5/5 mutations in section 5 produced a verdict line through an ERR trap; the zero count is grep -rn BUS_ENUM_CATALOG over kernel/, docker/qemu/run-x86-boot-tests.sh and docker/qemu/run-x86-prod-profile-boot-test.sh. -->
Every arm above is a `test` under `set -e`, so a failure reaches each
script's `ERR` trap and produces that script's canonical verdict line plus
its serial dump. Round 1's `case ... *) echo ...; exit 1 ;;` arm, which
bypassed the trap and produced neither (F9), is gone; `grep -c
BUS_ENUM_CATALOG` over both scripts and the kernel tree is 0 at HEAD. The
five mutations in section 5 each produced a real verdict line through the
trap, quoted there verbatim.

<!-- claim-lint:ok: neither shape appears in kernel/src/drivers/pci.rs or in either of docker/qemu/run-x86-boot-tests.sh and docker/qemu/run-x86-prod-profile-boot-test.sh at the pushed HEAD. -->
Two shapes round 1 carried are also gone: the per-drive labels that claimed
an identity the matcher never established (F11), and the
`enumerated >= expected` comparison that was `9 < 4` on every boot and
could not fire (F12).

---

## 3. The enumerated topology, per script

Both gates enumerate nine functions. Read from the clean runs of section 4;
the classification of each is the class/subclass byte pair the kernel
printed, not an inference.

`run-x86-boot-tests.sh` (`serial_user.txt:38-47` of the clean run):

| function | id | class/subclass | what it is | claimed by |
|---|---|---|---|---|
| 00:00.0 | 8086:1237 | 06/00 | Bridge / host bridge (440FX) | neither expectation |
| 00:01.0 | 8086:7000 | 06/01 | Bridge / ISA bridge (PIIX3) | neither |
| 00:01.1 | 8086:7010 | 01/01 | MassStorage / **IDE** (PIIX3 IDE) | neither -- subclass 01, not 00 |
| 00:01.3 | 8086:7113 | 06/80 | Bridge / other (PIIX4 ACPI), `irq=0x0a` | neither |
| 00:02.0 | 1234:1111 | 03/00 | Display (QEMU stdvga) | neither |
| 00:03.0 | 8086:100e | 02/00 | Intel e1000 (implicit default NIC) | the e1000 expectation |
| 00:04.0 | 1af4:1001 | 01/00 | virtio-blk, legacy transport | one of the three virtio-blk |
| 00:05.0 | 1af4:1001 | 01/00 | virtio-blk, legacy transport | one of the three |
| 00:06.0 | 1af4:1001 | 01/00 | virtio-blk, legacy transport | one of the three |

`run-x86-prod-profile-boot-test.sh` (`serial_user.txt:31-40` of the clean
run): the same first five functions, byte for byte, then `00:03.0`,
`00:04.0`, `00:05.0` as the three `1af4:1001 class=01/00` virtio-blk
functions and `00:06.0` as the `8086:100e class=02/00` e1000.

<!-- claim-lint:ok: the matchers in docker/qemu/run-x86-boot-tests.sh and docker/qemu/run-x86-prod-profile-boot-test.sh read the vendor:device and class fields only; no drive= token is read anywhere in either. -->
Nothing in either gate ties a particular slot to a particular `drive=`
flag, and this document does not claim one. The count of functions matching
each identity is what is asserted.

Two facts from that table worth stating plainly, because round 1 got them
wrong: 00:01.3 enumerates with `irq=0x0a`, not 255 (F6), and 00:01.1 is a
MassStorage function, not a bridge or the display controller (F5). Neither
affects the assertions -- the `irq != 0xff` predicate only ever runs
against a function already matched on vendor:device *and* class/subclass,
and neither of those two functions matches either identity.

---

## 4. Both profiles executing, at the pushed HEAD

Both runs on beast, Incus container `breenix-x86`, clone
`/root/breenix-busgate`, tree clean at the branch HEAD. One QEMU at a time.

### 4a. `boot_tests` profile

`./docker/qemu/run-x86-boot-tests.sh 1`, exit status **0**. Gate stdout,
lines 407-419 of the run log, verbatim:

```
  Device census: [ INFO] kernel::drivers::pci: PCI: Enumeration complete. Found 9 devices (3 VirtIO block, 1 network)
  PCI function facts (PCI_FN_TOTAL 9):
    PCI_FN 00:00.0 8086:1237 class=06/00 bar0=0x0/0x0 irq=0xff
    PCI_FN 00:01.0 8086:7000 class=06/01 bar0=0x0/0x0 irq=0xff
    PCI_FN 00:01.1 8086:7010 class=01/01 bar0=0x0/0x0 irq=0xff
    PCI_FN 00:01.3 8086:7113 class=06/80 bar0=0x0/0x0 irq=0x0a
    PCI_FN 00:02.0 1234:1111 class=03/00 bar0=0x80000000/0x1000000 irq=0xff
    PCI_FN 00:03.0 8086:100e class=02/00 bar0=0x81080000/0x20000 irq=0x0b
    PCI_FN 00:04.0 1af4:1001 class=01/00 bar0=0xc100/0x80 irq=0x0b
    PCI_FN 00:05.0 1af4:1001 class=01/00 bar0=0xc080/0x80 irq=0x0a
    PCI_FN 00:06.0 1af4:1001 class=01/00 bar0=0xc000/0x80 irq=0x0a
STRAND_CENSUS: threads_saved_blocked=11 stranded=0 lines=18165
x86 userspace gate: PASS - exited=109 expected>=104 nonzero=0 allowlist=0
```

Verdict, log line 438: `x86 frame-custody gate run 1: PASS`.

The same ten lines are in the boot's own capture at
`serial_user.txt:38-47`; `serial_kernel.txt` contains 0 of them. Capture
sizes for this run: `serial_user.txt` 1015 lines, `serial_kernel.txt`
17150 lines.

### 4b. Zero-feature production profile

`./docker/qemu/run-x86-prod-profile-boot-test.sh` (no `--features`), exit
status **0**. Verdict, log line 249:

```
PASS: x86 production profile reached steady state with the teardown census at rest
```

Observed-values row, log line 251:

```
  PCI_FN blk/e1000/total lines: 3/1/9
```

The boot's own capture, `serial_user.txt:31-40`, verbatim:

```
PCI_FN 00:00.0 8086:1237 class=06/00 bar0=0x0/0x0 irq=0xff
PCI_FN 00:01.0 8086:7000 class=06/01 bar0=0x0/0x0 irq=0xff
PCI_FN 00:01.1 8086:7010 class=01/01 bar0=0x0/0x0 irq=0xff
PCI_FN 00:01.3 8086:7113 class=06/80 bar0=0x0/0x0 irq=0x0a
PCI_FN 00:02.0 1234:1111 class=03/00 bar0=0x80000000/0x1000000 irq=0xff
PCI_FN 00:03.0 1af4:1001 class=01/00 bar0=0xc100/0x80 irq=0x0b
PCI_FN 00:04.0 1af4:1001 class=01/00 bar0=0xc080/0x80 irq=0x0b
PCI_FN 00:05.0 1af4:1001 class=01/00 bar0=0xc000/0x80 irq=0x0a
PCI_FN 00:06.0 8086:100e class=02/00 bar0=0x81080000/0x20000 irq=0x0a
PCI_FN_TOTAL 9
```

`serial_kernel.txt` contains 0 `PCI_FN` lines. Capture sizes:
`serial_user.txt` 149 lines, `serial_kernel.txt` 3595 lines.

This is the measurement behind section 1's print-path claim: the
zero-feature build, which compiles no test-framework registry at all, emits
the same ten lines on the same port.

---

## 5. Mutations

Five, each on a scratch, uncommitted copy, each reverted, each with the
verdict line the run actually printed. Two mutate the kernel's reported
fact; three mutate a script's bytes.

### 5a. Kernel: the e1000 device ID the fact line reports

Scratch edit to `kernel/src/drivers/pci.rs`, inside
`dump_enumerated_functions()` only:

```diff
             dev.vendor_id,
-            dev.device_id,
+            if dev.class == DeviceClass::Network { 0x9999u16 } else { dev.device_id },
```

Under that build the NIC's fact line reads
`PCI_FN 00:06.0 8086:9999 class=02/00 bar0=0x81080000/0x20000 irq=0x0a`
and the production gate's observed row reads `PCI_FN blk/e1000/total
lines: 3/0/9`.

**Production gate, mutated, exit 1:**

```
x86 production-profile gate: FAIL (set -e abort at ./docker/qemu/run-x86-prod-profile-boot-test.sh:1043, exit 1)
  failing command: test "$(marker_count "$PCI_FN_E1000_ID")" -eq "$EXPECTED_E1000"
  preserved failing serial: /tmp/breenix_x86_prod_profile_failures/20260904T081434Z_1361719
```

**`boot_tests` gate, mutated, exit 1:**

```
x86 frame-custody gate run 1: FAIL (set -e abort at ./docker/qemu/run-x86-boot-tests.sh:645, exit 1)
  failing command: test "$MATCHED_E1000" -eq "$EXPECTED_E1000"
```

The facts that run printed, verbatim from the gate log:

```
  PCI function facts (PCI_FN_TOTAL 9):
    PCI_FN 00:00.0 8086:1237 class=06/00 bar0=0x0/0x0 irq=0xff
    PCI_FN 00:01.0 8086:7000 class=06/01 bar0=0x0/0x0 irq=0xff
    PCI_FN 00:01.1 8086:7010 class=01/01 bar0=0x0/0x0 irq=0xff
    PCI_FN 00:01.3 8086:7113 class=06/80 bar0=0x0/0x0 irq=0x0a
    PCI_FN 00:02.0 1234:1111 class=03/00 bar0=0x80000000/0x1000000 irq=0xff
    PCI_FN 00:03.0 8086:9999 class=02/00 bar0=0x81080000/0x20000 irq=0x0b
    PCI_FN 00:04.0 1af4:1001 class=01/00 bar0=0xc100/0x80 irq=0x0b
    PCI_FN 00:05.0 1af4:1001 class=01/00 bar0=0xc080/0x80 irq=0x0a
    PCI_FN 00:06.0 1af4:1001 class=01/00 bar0=0xc000/0x80 irq=0x0a
```

The three virtio-blk legs (lines 633, 642 and 644) passed on that run; 645
is the first assertion after them, and it is the one that fired.

What this proves: the identity half of the equality is live on both gates
-- change the vendor:device the kernel reports for the NIC and both gates
redden, through their ERR traps, naming the assertion. What it does not
prove: it does not exercise `probe_device()`'s read of config space. That
the printed values are read from hardware rather than written by the check
is visible in the data instead -- the three byte-identical virtio-blk
functions print three different BAR addresses and two different interrupt
lines, and the two profiles print different slot assignments for the same
device set.

### 5b. `boot_tests` script: one `-device virtio-blk-pci` flag removed

Scratch copy `docker/qemu/r2-mutb1.sh` with line 367
(`-device virtio-blk-pci,drive=testdisk,...`) deleted; its `-drive` left in
place. Derived `EXPECTED_VIRTIO_BLOCK` drops 3 -> 2.

The boot then really has two virtio-blk functions, so the derived
expectation and the observed count fall together -- which is the census
doing its job, not a red:

```
  Device census: [ INFO] kernel::drivers::pci: PCI: Enumeration complete. Found 8 devices (2 VirtIO block, 1 network)
  PCI function facts (PCI_FN_TOTAL 8):
    PCI_FN 00:00.0 8086:1237 class=06/00 bar0=0x0/0x0 irq=0xff
    PCI_FN 00:01.0 8086:7000 class=06/01 bar0=0x0/0x0 irq=0xff
    PCI_FN 00:01.1 8086:7010 class=01/01 bar0=0x0/0x0 irq=0xff
    PCI_FN 00:01.3 8086:7113 class=06/80 bar0=0x0/0x0 irq=0x0a
    PCI_FN 00:02.0 1234:1111 class=03/00 bar0=0x80000000/0x1000000 irq=0xff
    PCI_FN 00:03.0 8086:100e class=02/00 bar0=0x81080000/0x20000 irq=0x0b
    PCI_FN 00:04.0 1af4:1001 class=01/00 bar0=0xc080/0x80 irq=0x0b
    PCI_FN 00:05.0 1af4:1001 class=01/00 bar0=0xc000/0x80 irq=0x0a
```

Both the pre-existing census leg and the new per-identity legs passed on
that run. The gate still reddened, exit 1, because removing that device
moves the ext2 root off virtio index 2 and the boot fails:

```
x86 frame-custody gate run 1: FAIL (set -e abort at ./docker/qemu/r2-mutb1.sh:674, exit 1)
  failing command: test "$passed" = true
```

(`r2-mutb1.sh:674` is `run-x86-boot-tests.sh:675` -- the copy is one line
shorter.) Stated plainly because it is the honest reading: this mutation
demonstrates that the expectation tracks the bytes *downward in step with
reality*, and it does not by itself demonstrate that the new assertion can
fail. 5c and 5d do that.

### 5c. `boot_tests` script: a flag made invisible to the census only

Scratch copy `docker/qemu/r2-mutb2.sh` with line 367's flag value quoted:

```diff
-        -device virtio-blk-pci,drive=testdisk,disable-modern=on,disable-legacy=off \
+        -device "virtio-blk-pci,drive=testdisk,disable-modern=on,disable-legacy=off" \
```

QEMU receives an identical argv element, so the boot still enumerates nine
functions with three virtio-blk; the anchored census stops seeing the flag,
so the derived count drops 3 -> 2. Derived and observed now disagree, and
the gate reddens, exit 1:

```
x86 frame-custody gate run 1: FAIL (set -e abort at ./docker/qemu/r2-mutb2.sh:542, exit 1)
  failing command: test "$CENSUS_VIRTIO_BLOCK" -eq "$EXPECTED_VIRTIO_BLOCK"
```

<!-- claim-lint:ok: 1/1 mutation run, verdict quoted verbatim just above; the assertion it names is line 542 of docker/qemu/run-x86-boot-tests.sh. -->
`r2-mutb2.sh:542` is the same line in the unmutated script: the
**pre-existing** count census, which shares the `EXPECTED_VIRTIO_BLOCK`
derivation the new leg reuses, and which sits earlier in the script. So on
this gate the shared derivation is proven live, and it is the older
assertion that catches it first. That is why 5d exists.

### 5d. Production script: the explicit e1000 flag made invisible to the census

Scratch copy `docker/qemu/r2-mutb3.sh` with line 960's flag value quoted.
QEMU still attaches the NIC (the observed row still reads `3/1/9`), but
`EXPECTED_E1000_FLAGS` drops 1 -> 0 while `NIC_OPTION_FLAGS` stays 1, so
the implicit-NIC arm does not apply and `EXPECTED_E1000` resolves to 0.
Exit 1:

```
x86 production-profile gate: FAIL (set -e abort at ./docker/qemu/r2-mutb3.sh:1041, exit 1)
  failing command: test "$EXPECTED_E1000" -ge 1
  preserved failing serial: /tmp/breenix_x86_prod_profile_failures/20260904T081157Z_1356373
```

<!-- claim-lint:ok: 1/1 mutation run, verdict quoted verbatim just above; docker/qemu/run-x86-prod-profile-boot-test.sh carried no PCI assertion at all before this branch. -->
The production gate carries no other PCI assertion, so this is the new leg
and nothing else. It reddens at the `-ge 1` floor rather than at the
equality, which is the floor doing exactly its job: it says the script's
own bytes stopped declaring a NIC at all. The equality itself is the arm
5a reddens.

### 5e. Structural ratchet: the self-derived arm is not a hole

<!-- claim-lint:ok: both new legs live in x86_production_profile_gate_ratchet_is_not_vacuous in tests/teardown_structure.rs; that test passes at HEAD, which is what makes each named mutation redden. -->
`tests/teardown_structure.rs` required every `marker_count` assertion in
the production gate to end in a literal `-eq 0` or `-eq 1`, so the new
`-eq "$EXPECTED_VIRTIO_BLK"` -- exact, but against a derived count --
reddened `x86_production_profile_gate_verdict_discipline_holds`. Renaming
the helper to dodge the scan would have narrowed the check to avoid
tripping on it, so the check was widened instead: it now also admits
`-eq "$VAR"` when VAR traces, through bare-integer and one-variable
assignments, to a `grep -c ... "${BASH_SOURCE[0]}"` self-census.

Two mutation legs added to `x86_production_profile_gate_ratchet_is_not_vacuous`
hold the new arm shut, alongside the pre-existing `-eq 1` -> `-ge 0` leg
which is unchanged and still reddens:

- pinning a marker assertion to `-eq "$UNDERIVED_EXPECTATION"` reddens;
- replacing the `EXPECTED_VIRTIO_BLK` self-census with a hand-pinned value
  reddens.

---

## 6. Builds, syntax, and the structure family

### 6a. Where the gate runs were made

Every gate run in sections 4 and 5 ran on beast, Incus container
`breenix-x86`, clone `/root/breenix-busgate`, at commit `199e1c7d`. The only
files that differ between `199e1c7d` and the pushed HEAD are
`tests/teardown_structure.rs` (the ratchet change of 5e) and this document;
neither is compiled into the kernel and neither is read by either gate
script, so the runs are runs of the HEAD kernel and HEAD gate-script bytes.
After the last mutation the clone was restored and left clean
(`git status --porcelain` empty, `grep -c 0x9999 kernel/src/drivers/pci.rs`
= 0, no `docker/qemu/r2-mut*.sh` remaining, 0 QEMU processes left running),
and a closing production-gate run at those restored bytes exited **0** with
the same `PASS:` verdict and the same `PCI_FN blk/e1000/total lines: 3/1/9`
row.

### 6b. Builds

On beast, at the restored HEAD bytes, each followed by
`grep -cE '^(warning|error)'` over its own log:

| build | exit | warning/error lines |
|---|---|---|
| `cargo build --release --features boot_tests,testing,external_test_bins --bin qemu-uefi` | 0 | **0** |
| `cargo build --release --bin qemu-uefi` (no `--features` flag) | 0 | **0** |

aarch64, on this Mac (unaffected by the change, built to show it):
`cargo build --release --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
-> exit 0. `grep -cE '^(warning|error)'` = **1**, and that one line is the
pre-existing toolchain future-incompat note about `core v0.0.0`, unrelated
to this branch. The kernel crate really recompiled (`Compiling kernel
v0.1.0` at log line 19) and the artifact was written.

### 6c. Syntax and the structure family

`bash -n docker/qemu/run-x86-boot-tests.sh` -> exit 0;
`bash -n docker/qemu/run-x86-prod-profile-boot-test.sh` -> exit 0 (measured
on beast at the restored bytes and on this Mac at HEAD).

All 25 `tests/*_structure.rs` binaries at HEAD: **25/25 ok, 0 failed, 499
test cases passed**, including the four that parse the two edited gate
scripts (`green_program_envelope_structure`, `loopback_pump_structure`,
`strand_handoff_structure`, `teardown_structure`).
`teardown_structure` alone: 81 passed, 0 failed -- it was 80 passed and 1
failed before the 5e ratchet change. The case that flipped is
`x86_production_profile_gate_verdict_discipline_holds`; 5e's two new
mutation legs live inside the existing
`x86_production_profile_gate_ratchet_is_not_vacuous` case and add no case of
their own, which is why the family total moved by exactly one, 498 -> 499.

### 6d. claim-lint

```
claim-lint: scripts/claim-lint.py --files kernel/src/drivers/pci.rs                          -> exit 1 (19 findings, == the 19 at bfbb7575)
claim-lint: scripts/claim-lint.py --files kernel/src/drivers/mod.rs                          -> exit 1 (3 findings,  == the 3 at bfbb7575)
claim-lint: scripts/claim-lint.py --files docker/qemu/run-x86-boot-tests.sh                  -> exit 1 (31 findings, == the 31 at bfbb7575)
claim-lint: scripts/claim-lint.py --files docker/qemu/run-x86-prod-profile-boot-test.sh      -> exit 1 (60 findings, == the 60 at bfbb7575)
claim-lint: scripts/claim-lint.py --files tests/teardown_structure.rs                        -> exit 1 (133 findings, == the 133 at bfbb7575)
claim-lint: scripts/claim-lint.py --files docs/planning/green-program/bus/BUS-X86-ENUM-GATE-2026-09-04.md -> exit 0
```

The per-file baselines were measured, not assumed: each file's `bfbb7575`
content was extracted into the working tree (so cited paths still resolve
from the repo root, which is what makes the count reproducible) and linted,
giving 19 / 3 / 31 / 60 / 133. The five pre-existing files end this round at
exactly those counts, so this branch's prose introduces zero new findings
and discharges none of the old ones. This round's four commit-message
drafts were linted the same way and are clean.

---

## 7. What this does not claim

- **The virtio-net swap was already caught.** Round 1's motivating example
  -- a virtio-net function landing where a virtio-blk was expected -- is
  caught on `run-x86-boot-tests.sh` by the pre-existing census:
  `Device::is_virtio_block()` (`kernel/src/drivers/pci.rs:212`) keys on vendor *and* device ID, so the swap
  drops `CENSUS_VIRTIO_BLOCK` from 3 to 2 and fails the assertion at line
  542. That claim was overreach (F13) and is not repeated here.
<!-- claim-lint:ok: the assertions are in docker/qemu/run-x86-prod-profile-boot-test.sh, which carried no PCI assertion at bfbb7575. -->
- **What is actually new**: per-function BAR-0 and interrupt-line facts on
  both gates; an exact `8086:100e` identity where the older census only
  required "at least one Network-class function"; a subclass-precise
  virtio-blk identity; and -- the real gap this closes -- any PCI assertion
  at all on the production gate, which had none before this branch.
<!-- claim-lint:ok: the matchers in docker/qemu/run-x86-boot-tests.sh and docker/qemu/run-x86-prod-profile-boot-test.sh read no drive= token. -->
- **No per-drive identity.** Nothing here ties `00:04.0` to `drive=hd`
  rather than to `drive=ext2disk`. The gates count functions per identity;
  they do not establish which QEMU drive backs which slot.
- **Not a proof that config space was read.** See 5a. The check is a
  transcription, and its non-vacuity as a *hardware* reading rests on the
  data varying per function and per profile, not on a mutation.
- **One boot per profile.** These are single runs at HEAD, not a soak. They
  establish that the leg executes and passes in both profiles, not a
  failure rate.
- **x86-64 only.** `dump_enumerated_functions()` is
  `#[cfg(target_arch = "x86_64")]` and is called only from the x86-64 arm
  of `drivers::init()`. aarch64's own `[pci]` dump in
  `kernel/src/drivers/mod.rs` is untouched, and no aarch64 gate reads
  `PCI_FN`.
<!-- claim-lint:ok: no assertion on that path exists in docker/qemu/run-x86-boot-tests.sh or docker/qemu/run-x86-prod-profile-boot-test.sh; the print itself is kernel/src/drivers/pci.rs. -->
- **Nothing about `./run.sh --x86` is asserted.** The kernel prints the
  same facts there; no expectation is attached to that path, which is the
  point of moving the expectations into the gates (F10).
