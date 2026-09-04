# x86-64 PCI enumeration facts, and the two gates that hold the expectations

Green-program "Bus / device infrastructure" row. Round 3, corrected against
the round-2 review (its findings are cited below by their F/N-numbers).
Round 2's own findings are cited as F1-F18; round 2's *review* findings
(caught re-checking round 2's own doc) are cited as N1-N4. F4 and N3 were
the same defect family (volatile per-run figures published as unscoped flat
facts); this round eliminates that family by scoping each such figure to a
named, committed capture file under `docs/planning/green-program/bus/serials/`
instead of a flat number, or by removing the figure. N1 and N2 were code
defects (a mis-derived expectation formula, a lossy printed field); N4 was a
mislabeled line reference. Each of the four is fixed below, with reproduced
evidence.

<!-- claim-lint:ok: the removals/additions are readable in kernel/src/drivers/pci.rs, docker/qemu/run-x86-boot-tests.sh, docker/qemu/run-x86-prod-profile-boot-test.sh and tests/teardown_structure.rs; the captures are the committed files under serials/ cited throughout. -->
The shape R127 bound stands unchanged from round 2: **the kernel prints
facts, the scripts hold expectations.** Round 3 does not touch that shape --
it fixes one lossy field in what the kernel prints (N2), one mis-derived
formula in what a script expects (N1), and the evidentiary discipline of
this document itself (F4/N3, N4).

<!-- claim-lint:ok: every cited line number was read back out of kernel/src/drivers/pci.rs, docker/qemu/run-x86-boot-tests.sh, docker/qemu/run-x86-prod-profile-boot-test.sh and tests/teardown_structure.rs at the pushed HEAD after the round-3 commits, which shifted nearly every line number below round 2's own citations (comments and code both grew). -->
Branch `green/bus-x86-enum-gate`. Every line number in this document was
re-derived from the pushed HEAD after the round-3 source edits. Round 3
commits: `d4d9d5a1` (N2, the raw-class-byte fix), `60fad834` (N1, the
additive-derivation fix), `608dcd97` (the structural ratchet widened to
admit the additive shape N1's fix introduced -- a regression this round
found in its own work and fixed in the same round, not carried forward).

---

## 0. Round-3 fixes, one at a time

### 0a. N2 -- the printed class byte is now byte-faithful

`kernel/src/drivers/pci.rs`'s `dump_enumerated_functions()` printed
`dev.class as u8`, where `dev.class` is `DeviceClass::from_u8(raw_byte)` --
a lossy round-trip. `DeviceClass` has 18 explicit arms (`0x00`-`0x11`) and a
fallback arm `_ => DeviceClass::Unknown`, whose discriminant is `0xFF`
(`kernel/src/drivers/pci.rs:107`, unchanged). Any class code outside those
18 arms -- `0x12` Processing Accelerator, `0x40` Co-processor, and so on --
would have printed `class=ff/<sub>`, indistinguishable from a genuine
`0xFF` byte. The function's own doc comment and this document's section 1
both described *each* emitted field as "a transcription of a value
`enumerate()` already parsed out of live PCI config space" -- true for
each other field, false in the general case for `class` as printed.

**Fix.** `Device` gained a new field, `raw_class: u8`
(`kernel/src/drivers/pci.rs:189-198`), populated at `probe_device()`
(`kernel/src/drivers/pci.rs:1068`) from the same `class_code` byte the
existing `class: DeviceClass::from_u8(class_code)` field already reads out
of config dword `0x08` -- a plain assignment, not a second parse, so there
is exactly one read of the byte and two ways of holding it.
`dump_enumerated_functions()` (`kernel/src/drivers/pci.rs:1425-1446`, moved
from round 2's `:1408` by the added field and its doc comment) now prints
`dev.raw_class` instead of `dev.class as u8`
(`kernel/src/drivers/pci.rs:1437`). The function's doc comment
(`kernel/src/drivers/pci.rs:1372-1387`) now names `raw_class` explicitly and
states the `class`/`raw_class` distinction, so the comment's
"transcription" framing now holds for each field it lists.

Not reachable on either gate's current PCI topology today (both gates'
nine enumerated functions are classes `01`/`02`/`03`/`06`, each covered by
`from_u8`'s explicit arms -- see section 3), but the fixed claim was about
the code's general behavior, not this run's data, which is exactly what N2
objected to.

### 0b. N1 -- EXPECTED_E1000 is derived additively, not either/or

Both gates' implicit-default-NIC branch read:

```bash
if [ "$EXPECTED_E1000_FLAGS" -eq 0 ] && [ "$NIC_OPTION_FLAGS" -eq 0 ]; then
    EXPECTED_E1000=1
else
    EXPECTED_E1000="$EXPECTED_E1000_FLAGS"
fi
```

<!-- claim-lint:ok: `-nic none` names an actual QEMU flag value, confirmed absent from both gate scripts by grep -nE -- '(^|[[:space:]])-(net|netdev|nic)([[:space:]]|=)' docker/qemu/run-x86-boot-tests.sh docker/qemu/run-x86-prod-profile-boot-test.sh. -->
QEMU attaches its own default NIC for `-machine pc` whenever no
`-net`/`-netdev`/`-nic` option is present on the command line --
`-nic none` is what suppresses it -- **regardless of how many explicit
`-device e1000,...` flags are also present**: a bare `-device` flag is
not on its own a `-net`/`-netdev`/`-nic` option, so the implicit NIC and an
explicit `e1000` device coexist as two separate PCI functions when both are
in play. The old branch's either/or shape mis-derives that case: with
`EXPECTED_E1000_FLAGS=1, NIC_OPTION_FLAGS=0` it would take the `else` arm
and derive `EXPECTED_E1000=1` against a real `2`.

**Fix.** Both scripts now compute
(`docker/qemu/run-x86-boot-tests.sh:618-622`,
`docker/qemu/run-x86-prod-profile-boot-test.sh:283-287`):

```bash
if [ "$NIC_OPTION_FLAGS" -eq 0 ]; then
    EXPECTED_E1000=$((EXPECTED_E1000_FLAGS + 1))
else
    EXPECTED_E1000="$EXPECTED_E1000_FLAGS"
fi
```

i.e. `EXPECTED_E1000 = EXPECTED_E1000_FLAGS + (1 if no -net/-netdev/-nic
option is present, else 0)`. Neither script's *current* flag set changes
behavior: `boot-tests` still derives `0 + 1 = 1`
(`EXPECTED_E1000_FLAGS=0, NIC_OPTION_FLAGS=0`, confirmed in section 2a's
clean run); `prod` still derives `1` via the explicit-only arm
(`EXPECTED_E1000_FLAGS=1, NIC_OPTION_FLAGS=1`, confirmed in section 2b).
Both branch comments were rewritten to state the additive rule and to stop
implying the explicit arm presupposes a paired `-netdev` (round 2's own
sentence about this branch was narrowly true as written, per N1's own
analysis, but only because it happened to describe the one case the old
formula got right).

**Proof, the case N1 named.** On beast, a scratch copy of
`run-x86-boot-tests.sh` with one line added -- `-device
e1000,mac=DE:AD:BE:EF:00:01 \` after the last `-device virtio-blk-pci` line,
with **no** paired `-net`/`-netdev`/`-nic` option -- and one diagnostic
`echo` of the derived values to stderr (the only difference from the
committed script besides the added flag; both removed after the run).
Captured verbatim at
`serials/n1-proof-added-e1000-no-net-run1-gate.log`, lines 407-419:

```
  Device census: [ INFO] kernel::drivers::pci: PCI: Enumeration complete. Found 10 devices (3 VirtIO block, 2 network)
R3-N1-PROOF: EXPECTED_E1000_FLAGS=1 NIC_OPTION_FLAGS=0 EXPECTED_E1000=2
  PCI function facts (PCI_FN_TOTAL 10):
    PCI_FN 00:00.0 8086:1237 class=06/00 bar0=0x0/0x0 irq=0xff
    PCI_FN 00:01.0 8086:7000 class=06/01 bar0=0x0/0x0 irq=0xff
    PCI_FN 00:01.1 8086:7010 class=01/01 bar0=0x0/0x0 irq=0xff
    PCI_FN 00:01.3 8086:7113 class=06/80 bar0=0x0/0x0 irq=0x0a
    PCI_FN 00:02.0 1234:1111 class=03/00 bar0=0x80000000/0x1000000 irq=0xff
    PCI_FN 00:03.0 8086:100e class=02/00 bar0=0x810a0000/0x20000 irq=0x0b
    PCI_FN 00:04.0 1af4:1001 class=01/00 bar0=0xc100/0x80 irq=0x0b
    PCI_FN 00:05.0 1af4:1001 class=01/00 bar0=0xc080/0x80 irq=0x0a
    PCI_FN 00:06.0 1af4:1001 class=01/00 bar0=0xc000/0x80 irq=0x0a
    PCI_FN 00:07.0 8086:100e class=02/00 bar0=0x81080000/0x20000 irq=0x0b
```

Derived reads **2** (`EXPECTED_E1000_FLAGS=1` from the one added flag, `+1`
for the still-absent `-net`/`-netdev`/`-nic` option), and the boot really
enumerates **2** `8086:100e class=02/00` functions -- `00:03.0` (the
implicit default) and `00:07.0` (the explicit flag). The additive formula
is what makes derived and observed agree here; the old either/or formula
would have derived `1`. Gate verdict at
`serials/n1-proof-added-e1000-no-net-run1-gate.log`, line 440: `x86 frame-custody
gate run 1: PASS`. Scratch script and its capture directory removed after
the run; beast clone confirmed clean (`git status --porcelain` empty) and 0
QEMU processes left running before the next step.

### 0c. N1's fix regressed a structural test, fixed in the same round

Widening `EXPECTED_E1000`'s derivation from a literal `EXPECTED_E1000=1` to
arithmetic `EXPECTED_E1000=$((EXPECTED_E1000_FLAGS + 1))` broke
`tests/teardown_structure.rs`'s `x86_prod_expectation_is_self_derived_depth`,
which round 2's own 5e section had taught to recognize exactly two
self-derived shapes for a marker assertion's right-hand side: a bare
integer literal, and a `$OTHER_VAR` reference. The new arithmetic RHS
matched neither, so `x86_production_profile_gate_verdict_discipline_holds`
started failing: `x86 production-profile gate relaxed a marker assertion:
test "$(marker_count "$PCI_FN_E1000_ID")" -eq "$EXPECTED_E1000"` -- a false
alarm (the assertion is still exact and still self-derived), but the
ratchet's own vocabulary didn't yet have the words for it. Per this
campaign's standing rule -- not narrowing a check to dodge what it catches;
widen it honestly and hold the wider arm shut with a mutation leg -- this
round added a third self-derived shape to
`x86_prod_expectation_is_self_derived_depth`
(`tests/teardown_structure.rs:14459-14507`): `$((VAR OP N))`, arithmetic
adding or subtracting a bare integer literal `N` from a variable `VAR` that
is itself self-derived (checked recursively, same depth cap of 4 as the
existing `$OTHER_VAR` recursion). A hand-pinned constant dressed up in
arithmetic syntax -- no left-hand variable name to recurse into -- still
falls through to the pre-existing `all_ok = false` path unchanged.

A new mutation leg, `"arithmetic derivation hand-pinned"`
(`tests/teardown_structure.rs:14627-14637`), holds the widened arm shut:
replacing `EXPECTED_E1000=$((EXPECTED_E1000_FLAGS + 1))` with
`EXPECTED_E1000=$((SOME_OTHER_VALUE + 1))` -- an undefined variable inside
the arithmetic -- still reddens the ratchet, because `SOME_OTHER_VALUE` has
no assignment anywhere in the script and so is not itself self-derived.
Verified on beast (section 6c): `x86_production_profile_gate_verdict_discipline_holds`
and `x86_production_profile_gate_ratchet_is_not_vacuous` both pass;
`teardown_structure` stays at 81 passed / 0 failed (the new leg lives
inside the existing ratchet-vacuity test case, adding no case of its own);
the full `tests/*_structure.rs` family is unchanged at 25/25 binaries, 499
cases, 0 failed.

### 0d. F4 + N3 -- each per-run-volatile figure is now scoped to a committed capture

Round 2's doc published flat numbers for quantities that vary between
otherwise-identical runs -- `serial_kernel.txt` line counts (17150 vs. a
re-checker's 17032, then a third value 17245 measured fresh this round),
and a build log's `Compiling kernel v0.1.0` position ("log line 19" vs. a
re-checker's line 1, then this round's own from-scratch build putting it at
line 1 again on a re-touch of just `pci.rs`, or *absent altogether* on a
warm-cache rebuild -- the position depends on what cargo actually had to
recompile, which is not a property of this branch's bytes at all). F4
(round 1) and N3 (round 2's re-check) are the same defect: a genuinely
volatile, honestly-measured, per-run quantity published as an unscoped flat
fact that the next re-checker measures differently and reads as a lie.

**Fix.** Each quantity in this category is now one of two things: scoped
to a named file under `docs/planning/green-program/bus/serials/` that is
committed in this same round (so "the number in this doc" and "the number
in the repo" are the same artifact, not two independent transcriptions),
or removed. The sweep, one figure at a time:

| figure | round-2 text | round-3 handling |
|---|---|---|
| `boot_tests` gate-log block, lines 407-419 | quoted inline, no capture backing it | scoped: `serials/boot-tests-run1-gate.log`, lines 407-419 (committed; quoted block below is that file's content, byte for byte) |
| `boot_tests` verdict line, "log line 438" | quoted inline | scoped: `serials/boot-tests-run1-gate.log`, line 438 |
| `boot_tests` `serial_user.txt:38-47` | line range cited, no capture backing it | scoped: `serials/boot-tests-run1-serial_user.txt`, lines 38-47 (committed) |
| `boot_tests` `serial_user.txt`/`serial_kernel.txt` line counts (1015 / 17150) | flat facts, unscoped | scoped + re-measured: `serials/boot-tests-run1-serial_user.txt` is committed at exactly 1015 lines; `serials/boot-tests-run1-serial_kernel.txt` is committed at exactly 17245 lines (not 17150 -- see below) -- both counts are now `wc -l` of a file in this repo, not an assertion a re-checker must reproduce by booting |
| `STRAND_CENSUS ... lines=18165` | quoted inline as if invariant | scoped: the figure is whatever `serials/boot-tests-run1-gate.log` line 418 says (`lines=18260` this round -- `1015 + 17245`), read from that file, not asserted as a constant |
| `prod` verdict line, "log line 249" | quoted inline | scoped: `serials/prod-profile-run1-gate.log`, line 249 |
| `prod` observed row, "log line 251" | quoted inline | scoped: `serials/prod-profile-run1-gate.log`, line 251 |
| `prod` `serial_user.txt:31-40` | line range cited, no capture backing it | scoped: `serials/prod-profile-run1-serial_user.txt`, lines 31-40 (committed) |
| `prod` `serial_user.txt`/`serial_kernel.txt` line counts (149 / 3595) | flat facts, unscoped | scoped + re-measured: `serials/prod-profile-run1-serial_user.txt` is committed at exactly 147 lines this round (not 149 -- this specific count moves run to run, confirmed independently by N3's own re-check landing on 149 both times it tried, and this round landing on 147; the file committed here is the ground truth for *this* round, not a claim about every future run); `serials/prod-profile-run1-serial_kernel.txt` is committed at exactly 3236 lines |
| aarch64 build, "`Compiling kernel v0.1.0` at log line 19" | flat line-number fact | **deleted**. Confirmed this round that the position is not a property of the branch: touching only `pci.rs` and rebuilding puts the line at position 1 (only the kernel crate recompiles, its deps already built); a from-scratch build would put it dozens of lines in, after each dependency; a pre-warmed cache omits the line altogether. Section 6b now reports only what is invariant -- exit code, warning/error count, and whether the kernel crate's artifact mtime moved -- and drops the line-position claim rather than re-measuring a number that has no fixed answer to scope |
| Section 5 mutation verdict lines and preserved-serial paths | quoted inline, tied to round-2's own script-line numbers | re-measured this round against the actual pushed HEAD (which round-3's own N1/N2 fixes shifted); each is additionally scoped to a committed `serials/mut5*-*.log` file (see section 5) |

Eleven rows above. Six are scoped to a committed capture with no change to
the underlying number (the two gate-log blocks, the two verdict lines, and
the two `serial_user.txt` ranges); two are re-measured *and* scoped (the
`serial_user.txt`/`serial_kernel.txt` line-count pairs, both of which moved
between round 2, its own re-check, and this round); one is deleted outright
(the aarch64 build-log line position, which section 0d shows has no fixed
answer to scope); one is the `STRAND_CENSUS` line, which was already a
capture-block member and is re-stated here only because round 2 quoted it
as if it were a constant rather than a sum of the two counts next to it;
and one row is the whole of section 5's mutation evidence, handled as a
group and detailed there rather than duplicated in this table.

### 0e. N4 -- the leg names at section 5's mutation writeup

Round 2's section 5 wrote: "The three virtio-blk legs (lines 633, 642 and
644) passed on that run; 645 is the first assertion after them, and it is
the one that fired." Line 633 (round 2's numbering) was not a virtio-blk
leg -- it was `test "$PCI_FN_LINE_COUNT" -eq "$PCI_FN_TOTAL_VALUE"`, the
dump-completeness check, identity-agnostic. The two actual virtio-blk legs
were 642 (`-ge 1`) and 644 (`-eq ...`); the enumeration also silently
skipped 643, the e1000 `-ge 1` floor, which sits between the two it named
and also passed on that run.

**Fix.** Section 5a below names each leg correctly by its round-3 line
number (the completeness check, the two `-ge 1` floors, the virtio-blk
equality, and the e1000 equality that fires), instead of grouping three
unlike things under one label.

---

## 1. What the kernel prints

`kernel/src/drivers/pci.rs:1425`, `pub fn dump_enumerated_functions()`,
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

<!-- claim-lint:ok: the field list is the format string in kernel/src/drivers/pci.rs, and the parse sites it transcribes are probe_device() and decode_bar() in that same file; raw_class is a plain field read with no from_u8 call, confirmed by grep -n raw_class kernel/src/drivers/pci.rs. -->
Every field is a transcription of a value `enumerate()` already parsed out
of live config space: vendor/device from config dword `0x00`, subclass from
`0x08`, `interrupt_line` from `0x3C`, and BAR 0's address and size from
`decode_bar()`. The `class=` field prints `Device::raw_class`
(`kernel/src/drivers/pci.rs:1437`), the untouched config-dword-`0x08` byte
-- **not** `Device::class`, which is `DeviceClass::from_u8(raw_class)` and
is lossy outside that enum's 18 explicit arms (section 0a). `bar0=` prints
BAR index 0's `address`/`size` verbatim (`0x0/0x0` when BAR 0's decoded size
is 0); `irq=` prints the raw `interrupt_line` byte, whose `0xff` is the PCI
"unknown / not connected" sentinel.

<!-- claim-lint:ok: kernel/src/drivers/pci.rs carries no expected-device table and no log::error! in this function; section 4 below is a measured boot whose only effect on this output is a nine-line list either way. -->
There is no expected-device set in the kernel, no PASS/FAIL verdict, and no
`log::error!` on any boot. A boot with a different device topology -- an
ordinary `./run.sh --x86` without the optional test-binaries or ext2
images -- prints a shorter list and nothing else.

**Print path.** `serial_println!` -> `kernel/src/serial.rs::_print` ->
`SERIAL1` = COM1. `kernel/src/serial.rs` carries exactly one cfg, the
crate-level `#![cfg(target_arch = "x86_64")]` at line 1; `_print` has no
feature gate and no log-level filter anywhere on its path, so these lines
appear in the zero-feature production profile exactly as they do under
`boot_tests,testing,external_test_bins`. Measured in both profiles' clean
captures below (section 4): `grep -c PCI_FN` of each profile's
`serial_kernel.txt` (COM2, the kernel log stream) is 0, and the `PCI_FN`
lines appear only in `serial_user.txt` (COM1).

Both gate scripts capture COM1 and COM2 to `$OUTPUT_DIR/serial_user.txt`
and `$OUTPUT_DIR/serial_kernel.txt` and grep across `serial_*.txt`, so both
read this stream.

There is no `assign_bars()` step to sequence after on x86-64:
`pci::assign_bars()` is `#[cfg(target_arch = "aarch64")]`
(`kernel/src/drivers/pci.rs:1236-1237`) and its only call site is the
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
as a fourth virtio-blk.

### 2a. `docker/qemu/run-x86-boot-tests.sh`

<!-- claim-lint:ok: produced by grep -nE on docker/qemu/run-x86-boot-tests.sh for the anchored device/drive/net flag pattern, 7/7 matching lines quoted. -->
Every device/drive/net flag line in the file at HEAD, quoted verbatim with
its line number (unaffected by round 3's edits, which are further down the
file):

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
option, one `8086:100e class=02/00` by the additive implicit-default-NIC
rule below (`0` explicit flags `+ 1` for the absent NIC option `= 1`).

Derivations:

- `524` (pre-existing, unaffected by round 3):
  `EXPECTED_VIRTIO_BLOCK=$(grep -cE -- '^[[:space:]]*-device virtio-blk-pci,drive=' "${BASH_SOURCE[0]}")` -> **3** at HEAD.
- `616-622` (round 3, N1 fix -- was `609-615`): `EXPECTED_E1000_FLAGS` counts
  `^[[:space:]]*-device e1000,` (**0** at HEAD), `NIC_OPTION_FLAGS` counts
  `^[[:space:]]*-(net|netdev|nic)[[:space:]]` (**0** at HEAD), and the
  additive branch at `618-622` resolves `EXPECTED_E1000` to `0 + 1 =` **1**.

<!-- claim-lint:ok: the 8086:100e reading is the capture quoted in section 4a of this document; the branch is in docker/qemu/run-x86-boot-tests.sh. -->
**Where the implicit e1000 comes from.** QEMU attaches its own default NIC
for `-machine pc` whenever no `-net`/`-netdev`/`-nic` option is given
**regardless of how many explicit `-device e1000,...` flags are present**
(section 0b); `-nic none` is what suppresses it, and this script passes
neither a NIC option nor an explicit e1000 flag. The model of that default
on the beast host's QEMU is measured, not assumed: `8086:100e` at
`00:03.0`, in the clean capture quoted in section 4. The same absent-option
rule is what the pre-existing `CENSUS_NETWORK -ge 1` leg (line 550) already
relies on; the new leg tightens it from ">= 1 network device" to "exactly N
`8086:100e`", where N is now additive rather than either/or.

Identity table at `587-588`; assertions at:

| line | assertion |
|---|---|
| 629 | `test -n "$PCI_FN_LINES"` |
| 632 | `test -n "$PCI_FN_TOTAL_LINE"` |
| 640 | `test "$PCI_FN_LINE_COUNT" -eq "$PCI_FN_TOTAL_VALUE"` |
| 649 | `test "$EXPECTED_VIRTIO_BLOCK" -ge 1` |
| 650 | `test "$EXPECTED_E1000" -ge 1` |
| 651 | `test "$MATCHED_VIRTIO_BLK" -eq "$EXPECTED_VIRTIO_BLOCK"` |
| 652 | `test "$MATCHED_E1000" -eq "$EXPECTED_E1000"` |
| 675 | `test -n "$PCI_FN_FACT_VIOLATIONS"` |
| 676 | `test "$PCI_FN_FACT_VIOLATIONS" -eq 0` |

(The nine lines above each moved down by exactly 7 from round 2's citations, the net
growth of the rewritten branch comment at 0b; the assertions and their
order are otherwise unchanged from round 2.)

### 2b. `docker/qemu/run-x86-prod-profile-boot-test.sh`

<!-- claim-lint:ok: produced by grep -nE on docker/qemu/run-x86-prod-profile-boot-test.sh for the anchored device/drive/net flag pattern, 9/9 matching lines quoted. -->
Every device/drive/net flag line in the file at HEAD, quoted verbatim with
its line number (moved down by 4 from round 2's `953-963`, the net growth
of the rewritten branch comment at 0b, which sits above this block):

```
957:    -drive "if=none,id=hd,format=raw,readonly=on,file=$BREENIX_ROOT/$UEFI_IMG" \
958:    -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
959:    -drive "if=none,id=placeholder,format=raw,readonly=on,file=$OUTPUT_DIR/placeholder.img" \
960:    -device virtio-blk-pci,drive=placeholder,disable-modern=on,disable-legacy=off \
961:    -drive "if=none,id=ext2disk,format=raw,readonly=on,file=$BREENIX_ROOT/target/ext2.img" \
962:    -device virtio-blk-pci,drive=ext2disk,disable-modern=on,disable-legacy=off \
963:    -netdev user,id=net0 \
964:    -device e1000,netdev=net0,mac=52:54:00:12:34:56 \
967:    -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
```

Which resolves to: three `-device virtio-blk-pci` flags -> three
`1af4:1001 class=01/00`; one `-device e1000` flag -> one
`8086:100e class=02/00`; the `-netdev` line is a backend, and its presence
is what zeroes the implicit-NIC term on this script; `isa-debug-exit` is
ISA, not PCI, and is correctly absent.

Derivations (`264`/`265` unaffected by round 3, both before the rewritten
branch comment): `264` -> `EXPECTED_VIRTIO_BLK` = **3**; `265` ->
`EXPECTED_E1000_FLAGS` = **1**; `282` (was `278`) -> `NIC_OPTION_FLAGS` =
**1**; the additive branch at `283-287` (was `279-283`) -> `EXPECTED_E1000`
= `1 + 0 =` **1** (the `+0` because `NIC_OPTION_FLAGS` is nonzero, so the
implicit-NIC term drops out and the else-arm reduces to the explicit-flags
count alone). Identity table at `262-263`; observed-values row at `809`
(was `805`).

Assertions:

| line | assertion |
|---|---|
| 1034 | `test -n "$PCI_FN_LINES"` |
| 1035 | `test "$(marker_count 'PCI_FN_TOTAL ')" -eq 1` |
| 1040 | `test "$PCI_FN_LINE_COUNT" -eq "$PCI_FN_TOTAL_VALUE"` |
| 1044 | `test "$EXPECTED_VIRTIO_BLK" -ge 1` |
| 1045 | `test "$EXPECTED_E1000" -ge 1` |
| 1046 | `test "$(marker_count "$PCI_FN_VIRTIO_BLK_ID")" -eq "$EXPECTED_VIRTIO_BLK"` |
| 1047 | `test "$(marker_count "$PCI_FN_E1000_ID")" -eq "$EXPECTED_E1000"` |
| 1069 | `test -n "$PCI_FN_FACT_VIOLATIONS"` |
| 1070 | `test "$PCI_FN_FACT_VIOLATIONS" -eq 0` |

(The nine lines above each moved down by exactly 4 from round 2's citations.)

### 2c. The per-function fact predicate

`PCI_FN_FACT_VIOLATIONS` (boot_tests `662-674`, was `655-667`; production
`1056-1068`, was `1052-1064`) is an `awk` block over the matched `PCI_FN`
lines, unchanged in substance from round 2 -- only its line position moved.
For each matched line it reads `bar0=<addr>/<size>` and `irq=<line>` out of
the text and counts a violation when the address is `0x0`, the size is
`0x0`, or the interrupt line is `0xff` -- and also when any of the three
fields is missing from the line at all.

### 2d. Failure routing

Unchanged from round 2. Each arm above is a `test` under `set -e`, so a
failure reaches each script's `ERR` trap and produces that script's
canonical verdict line plus its serial dump; `grep -c BUS_ENUM_CATALOG`
over both scripts and the kernel tree is 0 at HEAD.

---

## 3. The enumerated topology, per script

Both gates enumerate nine functions. Read from the clean runs of section 4;
the classification of each is the class/subclass byte pair the kernel
printed -- now `Device::raw_class`, byte-faithful (section 0a) -- not an
inference.

`run-x86-boot-tests.sh`
(`serials/boot-tests-run1-serial_user.txt`, lines 38-47, committed):

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

`run-x86-prod-profile-boot-test.sh`
(`serials/prod-profile-run1-serial_user.txt`, lines 31-40, committed): the same
first five functions, byte for byte, then `00:03.0`, `00:04.0`, `00:05.0`
as the three `1af4:1001 class=01/00` virtio-blk functions and `00:06.0` as
the `8086:100e class=02/00` e1000. (The two profiles slot the NIC and the
three virtio-blk disks differently -- `boot_tests` puts the NIC at
`00:03.0` and the disks at `04`-`06`; `prod` puts the disks at `03`-`05`
and the NIC at `06` -- because `prod`'s QEMU invocation lists the `-netdev`
device after all three `-drive`/`-device virtio-blk-pci` pairs while
`boot_tests` has no explicit NIC flag at all, so QEMU's implicit default
attaches at the first free slot. Neither gate's matchers read the slot, so
this reordering does not matter to either gate -- see below.)

<!-- claim-lint:ok: the matchers in docker/qemu/run-x86-boot-tests.sh and docker/qemu/run-x86-prod-profile-boot-test.sh read the vendor:device and class fields only; no drive= token is read anywhere in either. -->
Nothing in either gate ties a particular slot to a particular `drive=`
flag, and this document does not claim one. The count of functions matching
each identity is what is asserted.

Two facts from that table worth stating plainly, carried from round 2 and
still true at HEAD: `00:01.3` enumerates with `irq=0x0a`, not `255`, and
`00:01.1` is a MassStorage function, not a bridge or the display
controller. Neither affects the assertions -- the `irq != 0xff` predicate
only ever runs against a function already matched on vendor:device *and*
class/subclass, and neither of those two functions matches either
identity.

---

## 4. Both profiles executing, at the pushed HEAD

Both runs on beast, Incus container `breenix-x86`, clone
`/root/breenix-busgate`, tree clean at commit `60fad834` (the N1 fix, the
last commit before this doc's own rewrite began -- `608dcd97`, the ratchet
widening, is a test-only file and does not affect either gate's runtime
behavior). One QEMU at a time. Each quantity below that is per-run-volatile
(F4/N3, section 0d) is a citation into a file committed alongside this
document, at `docs/planning/green-program/bus/serials/`, not a flat number.

### 4a. `boot_tests` profile

`./docker/qemu/run-x86-boot-tests.sh 1`, exit status **0**. Full run log
committed at `serials/boot-tests-run1-gate.log` (438 lines). Gate stdout,
that file's lines 407-419, verbatim:

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
STRAND_CENSUS: threads_saved_blocked=11 stranded=0 lines=18260
x86 userspace gate: PASS - exited=109 expected>=104 nonzero=0 allowlist=0
```

Verdict, `serials/boot-tests-run1-gate.log`, line 438: `x86 frame-custody gate
run 1: PASS`.

The same ten lines are in the boot's own committed capture at
`serials/boot-tests-run1-serial_user.txt`, lines 38-47;
`serials/boot-tests-run1-serial_kernel.txt` contains 0 of them
(`grep -c PCI_FN` = 0, verified against the committed file). Capture sizes
for this run, as `wc -l` of the committed files themselves --
`serials/boot-tests-run1-serial_user.txt` = **1015** lines,
`serials/boot-tests-run1-serial_kernel.txt` = **17245** lines (`1015 +
17245 = 18260`, matching the `STRAND_CENSUS` line above; round 2 published
17150 for this figure, a re-checker measured 17032, this round measures
17245 -- three different numbers across three runs, which is exactly why
this figure is now a citation into a committed file rather than a flat
fact; `serial_user.txt`'s own 1015 has reproduced identically across all
three measurements to date).

### 4b. Zero-feature production profile

`./docker/qemu/run-x86-prod-profile-boot-test.sh` (no `--features`), exit
status **0**. Full run log committed at `serials/prod-profile-run1-gate.log`
(333 lines). Verdict, that file's line 249:

```
PASS: x86 production profile reached steady state with the teardown census at rest
```

Observed-values row, line 251:

```
  PCI_FN blk/e1000/total lines: 3/1/9
```

The boot's own committed capture, `serials/prod-profile-run1-serial_user.txt`, lines 31-40,
verbatim:

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

`serials/prod-profile-run1-serial_kernel.txt` contains 0 `PCI_FN` lines.
Capture sizes, as `wc -l` of the committed files --
`serials/prod-profile-run1-serial_user.txt` = **147** lines,
`serials/prod-profile-run1-serial_kernel.txt` = **3236** lines (round 2
published 149/3595; a re-checker measured 149 again for `serial_user.txt`
but 3560 for `serial_kernel.txt`; this round measures 147/3236 -- the user
capture has now varied too, by 2 lines, confirming it is not the invariant
N3's re-check took it for, merely one that had not yet been observed to
move).

This is the measurement behind section 1's print-path claim: the
zero-feature build, which compiles no test-framework registry at all, emits
the same ten lines on the same port.

---

## 5. Mutations

Five, each on a scratch, uncommitted copy or a `git checkout`-reverted
in-place edit, each with the verdict line and facts the run actually
printed, each now scoped to a committed capture. Each of the five was re-run this
round against the round-3 HEAD (the round-2 originals' line-number
citations no longer matched the post-N1/N2 script bytes).

### 5a. Kernel: the e1000 device ID the fact line reports

Scratch edit to `kernel/src/drivers/pci.rs`, inside
`dump_enumerated_functions()` only (reverted via `git checkout --` after
both runs below):

```diff
             dev.vendor_id,
-            dev.device_id,
+            if dev.class == DeviceClass::Network { 0x9999u16 } else { dev.device_id },
             dev.raw_class,
```

Under that build the NIC's fact line reads `PCI_FN 00:03.0 8086:9999
class=02/00 ...` (boot_tests) / `PCI_FN 00:06.0 8086:9999 class=02/00 ...`
(prod), and the production gate's observed row reads `PCI_FN
blk/e1000/total lines: 3/0/9`.

**Production gate, mutated, exit 1.** Committed at
`serials/mut5a-prod-run-gate.log`, lines 249-254:

```
x86 production-profile gate: FAIL (set -e abort at ./docker/qemu/run-x86-prod-profile-boot-test.sh:1047, exit 1)
  failing command: test "$(marker_count "$PCI_FN_E1000_ID")" -eq "$EXPECTED_E1000"
  preserved failing serial: /tmp/breenix_x86_prod_profile_failures/20260904T094601Z_1513607
  PCI_FN blk/e1000/total lines: 3/0/9
```

`1047` is the e1000 identity equality (section 2b's table).

**`boot_tests` gate, mutated, exit 1.** Committed at
`serials/mut5a-boottests-run-gate.log`, lines 418-419:

```
x86 frame-custody gate run 1: FAIL (set -e abort at ./docker/qemu/run-x86-boot-tests.sh:652, exit 1)
  failing command: test "$MATCHED_E1000" -eq "$EXPECTED_E1000"
```

`652` is the e1000 identity equality (section 2a's table). The facts that
run printed, committed at `serials/mut5a-boottests-run-gate.log`, lines 407-415,
show `PCI_FN 00:03.0 8086:9999 class=02/00 ...` in place of the healthy
`8086:100e`, with every other function unchanged from section 4a's clean
run.

Section 0e's naming, applied to this run's assertion order
(`docker/qemu/run-x86-boot-tests.sh`): the dump-completeness check (640)
and the two `-ge 1` floors (649 virtio-blk, 650 e1000) both passed; 651 (the
virtio-blk identity equality) passed; 652 (the e1000 identity equality) is
the one that fired, and it is the first assertion after 651.

What this proves: the identity half of the equality is live on both gates
-- change the vendor:device the kernel reports for the NIC and both gates
redden, through their ERR traps, naming the assertion. What it does not
prove: it does not exercise `probe_device()`'s read of config space. That
the printed values are read from hardware rather than written by the check
is visible in the data instead -- the three byte-identical virtio-blk
functions print three different BAR addresses and two different interrupt
lines (section 4), and the two profiles print different slot assignments
for the same device set (section 3).

### 5b. `boot_tests` script: one `-device virtio-blk-pci` flag removed

Scratch copy `docker/qemu/r3-mutb1.sh` with line 367
(`-device virtio-blk-pci,drive=testdisk,...`) deleted; its `-drive` left in
place. Derived `EXPECTED_VIRTIO_BLOCK` drops 3 -> 2.

The boot then really has two virtio-blk functions, so the derived
expectation and the observed count fall together -- the census doing its
job, not a red. Committed at
`serials/mut5b-boottests-flag-deleted-run-gate.log`, lines 407-416:

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
moves the ext2 root off virtio index 2 and the boot fails. Committed at
`serials/mut5b-boottests-flag-deleted-run-gate.log`, lines 417-418:

```
x86 frame-custody gate run 1: FAIL (set -e abort at ./docker/qemu/r3-mutb1.sh:681, exit 1)
  failing command: test "$passed" = true
```

(`r3-mutb1.sh:681` is `run-x86-boot-tests.sh:682` -- the copy is one line
shorter.) This mutation demonstrates that the expectation tracks the bytes
*downward in step with reality*, and it does not by itself demonstrate that
the new assertion can fail. 5c and 5d do that.

### 5c. `boot_tests` script: a flag made invisible to the census only

Scratch copy `docker/qemu/r3-mutb2.sh` with line 367's flag value quoted:

```diff
-        -device virtio-blk-pci,drive=testdisk,disable-modern=on,disable-legacy=off \
+        -device "virtio-blk-pci,drive=testdisk,disable-modern=on,disable-legacy=off" \
```

QEMU receives an identical argv element, so the boot still enumerates nine
functions with three virtio-blk; the anchored census stops seeing the flag,
so the derived count drops 3 -> 2. Derived and observed now disagree, and
the gate reddens, exit 1. Committed at
`serials/mut5c-boottests-flag-quoted-run-gate.log`, lines 407-408:

```
x86 frame-custody gate run 1: FAIL (set -e abort at ./docker/qemu/r3-mutb2.sh:542, exit 1)
  failing command: test "$CENSUS_VIRTIO_BLOCK" -eq "$EXPECTED_VIRTIO_BLOCK"
```

`r3-mutb2.sh:542` is the same line in the unmutated script, unaffected by
round 3's edits: the **pre-existing** count census, which shares the
`EXPECTED_VIRTIO_BLOCK` derivation the new leg reuses, and which sits
earlier in the file. So on this gate the shared derivation is verified live,
and it is the older assertion that catches it first. That is why 5d exists.

### 5d. Production script: the explicit e1000 flag made invisible to the census

Scratch copy `docker/qemu/r3-mutb3.sh` with line 964's flag value quoted.
QEMU still attaches the NIC, but `EXPECTED_E1000_FLAGS` drops 1 -> 0 while
`NIC_OPTION_FLAGS` stays 1, so `EXPECTED_E1000` resolves to `0 + 0 = 0`.
Exit 1. Committed at
`serials/mut5d-prod-e1000-flag-quoted-run-gate.log`, lines 249-254:

```
x86 production-profile gate: FAIL (set -e abort at ./docker/qemu/r3-mutb3.sh:1045, exit 1)
  failing command: test "$EXPECTED_E1000" -ge 1
  preserved failing serial: /tmp/breenix_x86_prod_profile_failures/20260904T104916Z_1652435
  PCI_FN blk/e1000/total lines: 3/1/9
```

The production gate carries no other PCI assertion, so this is the new leg
and nothing else. It reddens at the `-ge 1` floor (1045) rather than at the
equality (1047), which is the floor doing exactly its job: it says the
script's own bytes stopped declaring a NIC at all. The observed row still
reads `3/1/9` -- the NIC genuinely enumerated, `3/1/9` is what a *healthy*
boot prints -- and that is precisely the point: the floor fires on a
healthy boot's own data because the derivation, not the hardware, is what
broke.

<!-- claim-lint:ok: this leg took four attempts on beast before it isolated cleanly; the first two runs of the unmodified attempt (before OUTPUT_DIR isolation) collided with a concurrent, unrelated lane (clone /root/breenix-775) also running the production gate against the same shared $OUTPUT_DIR path (/tmp/breenix_x86_prod_profile), corrupting both runs' console.sock and observed-values reads; a third clean-window attempt hit a second, unrelated flake at the liveness PROMPT_AFTER/PROMPT_BEFORE check. The evidence above is the fourth attempt, run against a scratch copy with OUTPUT_DIR overridden to a private path (/tmp/breenix_x86_prod_profile_r3busgate), which produced a clean, uncontaminated result matching the predicted line and assertion exactly. -->
**A note on how this leg's evidence was obtained.** The production gate's
`$OUTPUT_DIR` is a fixed path
(`docker/qemu/run-x86-prod-profile-boot-test.sh:194`,
`/tmp/breenix_x86_prod_profile`), not unique per invocation or per clone.
Two earlier attempts at this specific leg collided with an unrelated,
concurrently-running lane on the same shared beast container (clone
`/root/breenix-775`) also exercising the production gate against that same
path -- both processes' `console.sock` and serial captures overlapped,
producing corrupted `0/0/0` observed-value reads that had nothing to do
with this mutation. A third attempt, in a confirmed-clear window, hit an
unrelated liveness-check flake (`PROMPT_AFTER`/`PROMPT_BEFORE`, a
console-responsiveness timing check, section 0's arithmetic fix touches
neither). The evidence quoted above is a fourth attempt, run from a scratch
copy with `OUTPUT_DIR` overridden to a private path so it could not collide
with any other lane's run; it isolated cleanly and reproduced the predicted
line and assertion exactly. This is a pre-existing hazard of the shared
`$OUTPUT_DIR` path across concurrent lanes on one build host, not a defect
introduced by this branch, and not this document's to fix -- noted here
only because it is why this leg's evidence took four attempts to gather
honestly rather than one.

### 5e. Structural ratchet: the self-derived arm is not a hole

Round 2's own widening (admitting an exact-but-self-derived `-eq "$VAR"`
shape) still holds: `tests/teardown_structure.rs` still requires each
`marker_count` assertion in the production gate to end in a literal `-eq 0`
/`-eq 1`, or in an equality against a variable the script derives from its
own bytes. Round 3 widened that predicate again, in the same file, to admit
the additive-arithmetic shape N1's fix introduced (`$((VAR + N))`) --
detailed in section 0c, including the new mutation leg
(`"arithmetic derivation hand-pinned"`) that holds the wider arm shut. Both
widenings' mutation legs live inside the same test case,
`x86_production_profile_gate_ratchet_is_not_vacuous`, and both continue to
redden their respective mutations at HEAD (section 6c).

---

## 6. Builds, syntax, and the structure family

### 6a. Where the gate runs were made

Each gate run in sections 4 and 5 ran on beast, Incus container
`breenix-x86`, clone `/root/breenix-busgate`. Sections 4 and 5a-5d ran at
commit `60fad834` (the N1/N2 fixes, before the ratchet widening or this
document existed); section 6c's structure-family run ran at `608dcd97`
(after the ratchet widening) to exercise the widened predicate. After each
mutation the clone was restored and left clean
(`git status --porcelain` empty; `grep -c 0x9999 kernel/src/drivers/pci.rs`
= 0; no `docker/qemu/r3-mut*.sh` scratch files remaining; 0 QEMU processes
left running, confirmed by PID-specific checks against `pgrep -fa
qemu-system-x86_64`, not a name-wide kill per the workflow's standing
rule).

### 6b. Builds

On beast, at HEAD (`608dcd97`), each followed by
`grep -cE '^(warning|error)'` over its own log:

| build | exit | warning/error lines |
|---|---|---|
| `cargo build --release --features boot_tests,testing,external_test_bins --bin qemu-uefi` | 0 | **0** |
| `cargo build --release --bin qemu-uefi` (no `--features` flag) | 0 | **0** |

aarch64, on this Mac (unaffected by the round-3 code changes' *arch* --
`pci.rs` is shared, so it was rebuilt to confirm; the two gate scripts and
`tests/teardown_structure.rs` are x86-only and test-only respectively, and
do not participate in this build at all):
`cargo build --release --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
-> exit 0. `grep -cE '^(warning|error)'` = **1**, and that one line is the
pre-existing toolchain future-incompat note about `core v0.0.0`, unrelated
to this branch. Confirmed by touching only `kernel/src/drivers/pci.rs` and
rebuilding: only the `kernel` crate recompiled (each dependency was
already built), and the artifact was written -- the shared file with the
new `raw_class` field compiles cleanly on the arch that does not itself read it.
(Round 3 dropped the round-2 claim that pinned this build's `Compiling
kernel v0.1.0` line to a specific log-line number -- section 0d explains
why that figure has no fixed answer to scope.)

### 6c. Syntax and the structure family

`bash -n docker/qemu/run-x86-boot-tests.sh` -> exit 0;
`bash -n docker/qemu/run-x86-prod-profile-boot-test.sh` -> exit 0 (measured
on this Mac at HEAD, `608dcd97`).

All 25 `tests/*_structure.rs` binaries at HEAD (`608dcd97`, on beast): **25/25
ok, 0 failed, 499 test cases passed** -- unchanged from round 2's total,
because the round-3 ratchet widening (section 0c) added a mutation leg
inside an existing test case rather than a new case.
`teardown_structure` alone: 81 passed, 0 failed (unchanged from round 2's
own post-widening total, for the same reason). Specifically re-verified:
`x86_production_profile_gate_verdict_discipline_holds` (the case N1's fix
regressed, section 0c) and `x86_production_profile_gate_ratchet_is_not_vacuous`
(the case carrying both the round-2 and the round-3 mutation legs) both
pass at HEAD.

### 6d. claim-lint

```
claim-lint: scripts/claim-lint.py --files kernel/src/drivers/pci.rs                          -> exit 1 (19 findings, == the 19 at bfbb7575)
claim-lint: scripts/claim-lint.py --files docker/qemu/run-x86-boot-tests.sh                  -> exit 1 (31 findings, == the 31 at bfbb7575)
claim-lint: scripts/claim-lint.py --files docker/qemu/run-x86-prod-profile-boot-test.sh      -> exit 1 (60 findings, == the 60 at bfbb7575)
claim-lint: scripts/claim-lint.py --files tests/teardown_structure.rs                        -> exit 1 (133 findings, == the 133 at bfbb7575)
claim-lint: scripts/claim-lint.py --files docs/planning/green-program/bus/BUS-X86-ENUM-GATE-2026-09-04.md -> exit 0
```

`kernel/src/drivers/mod.rs` is untouched by round 3 (no diff against
round 2's HEAD), so its round-2 baseline (3 findings, matching bfbb7575)
still holds and is not re-run here. Each round-3 source edit that added
prose -- the `raw_class` field comment, the `dump_enumerated_functions` doc
comment, both gate scripts' rewritten branch comments, and the two new
`teardown_structure.rs` doc comments -- carries its own `claim-lint:ok`
annotation where the added text would otherwise trip the linter (confirmed
by each file's finding count matching its `bfbb7575` baseline exactly:
no new findings introduced, no pre-existing findings silently
discharged). Round notes:
`/private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/green-bus/bus-r3-notes.md`.

---

## 7. What this does not claim

- **The virtio-net swap was already caught.** Round 1's motivating example
  -- a virtio-net function landing where a virtio-blk was expected -- is
  caught on `run-x86-boot-tests.sh` by the pre-existing census:
  `Device::is_virtio_block()` (`kernel/src/drivers/pci.rs:222`, moved from
  round 2's `:212` by the `raw_class` field addition) keys on vendor *and*
  device ID, so the swap drops `CENSUS_VIRTIO_BLOCK` from 3 to 2 and fails
  the assertion at line 542 (unaffected by round 3's edits, which sit
  further down the file).
- **What is actually new**: per-function BAR-0 and interrupt-line facts on
  both gates; an exact `8086:100e` identity where the older census only
  required "at least one Network-class function", now derived by an
  additive formula that stays correct when an explicit e1000 flag and the
  implicit default NIC coexist (round 3, N1); a subclass-precise virtio-blk
  identity; a byte-faithful printed class field that does not round-trip
  through a lossy enum for out-of-range codes (round 3, N2); and -- the
  real gap round 2 closed -- any PCI assertion at all on the production
  gate, which carried no such assertion before that round.
- **No per-drive identity.** No fact here ties `00:04.0` to `drive=hd`
  rather than to `drive=ext2disk`. The gates count functions per identity;
  they do not establish which QEMU drive backs which slot.
- **Not a proof that config space was read.** See 5a. The check is a
  transcription, and its non-vacuity as a *hardware* reading rests on the
  data varying per function and per profile, not on a mutation.
- **One boot per profile in section 4, four attempts for one leg in section
  5d.** These are single runs at HEAD, not a soak; section 4 establishes
  that the leg executes and passes in both profiles, not a failure rate.
  Section 5d's repeated attempts were host-contention and an unrelated
  flake, disclosed in place rather than smoothed over (section 5d's note).
- **x86-64 only.** `dump_enumerated_functions()` is
  `#[cfg(target_arch = "x86_64")]` and is called only from the x86-64 arm
  of `drivers::init()`. aarch64's own `[pci]` dump in
  `kernel/src/drivers/mod.rs` is untouched, and no aarch64 gate reads
  `PCI_FN`.
- **No assertion touches `./run.sh --x86`.** The kernel prints the
  same facts there; no expectation is attached to that path.
- **The shared `$OUTPUT_DIR` collision noted in 5d is not this branch's to
  fix.** It is a pre-existing property of both gate scripts (round 2's own
  bytes and earlier), unrelated to the N1/N2 fixes, and is disclosed as an
  evidentiary note, not filed as a defect of this row.
