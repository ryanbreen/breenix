# B3 — prepared single-hunk mutations for the Prove slot

Two legs' census assertions have never been mutation-proven (review finding B3):
`run-x86-boot-tests.sh`'s census (a real boot exercised it, but only via an
accidental self-count bug that has since been fixed — a false-positive red,
not a mutation proof) and `run-aarch64-service-sequence-gate.sh`'s census arm
(never even EXECUTED once, by either the implement or confirm slot — every
run against that script substituted `run-aarch64-full-test.sh
--boot-tests-only` instead).

Both mutations below are SCRIPT-level (mutate the already-boot-derived shell
variable holding the parsed census value, not the kernel's log-emission
line), matching the technique the confirm slot used for `run-x86-gate.sh`'s
own mutations under the same Fable/ultracode source-edit restriction this fix
slot also operates under (`kernel/src/**` edits are blocked by the harness;
`docker/qemu/*.sh` edits are not). This is disclosed, not hidden, and is
equally falsifying: the assertion under test reads the shell variable, not
the kernel source, so mutating the variable after a REAL boot has already
populated it from REAL kernel output exercises the identical downstream
comparison a wrong kernel-emitted count would.

Apply each mutation with the exact `sed -i` command below (or by hand), run
the gate, capture the FAIL output, then revert with `git checkout --
<file>` and confirm `git diff --stat <file>` is empty before moving to the
next mutation. Do not leave any mutation applied when done.

---

## Mutation 1 — `run-x86-boot-tests.sh`, VirtIO-block count

Run on beast (the canonical x86 build host). One boot is enough — the
mutation only needs to be observed reddening once.

**Apply:**
```bash
cd <x86 scratch clone>
sed -i '' '/^    test "\$CENSUS_VIRTIO_BLOCK" -eq "\$EXPECTED_VIRTIO_BLOCK"\$/i\
    CENSUS_VIRTIO_BLOCK=$((CENSUS_VIRTIO_BLOCK + 99))
' docker/qemu/run-x86-boot-tests.sh
# (on Linux/beast, drop the '' after -i)
git diff docker/qemu/run-x86-boot-tests.sh   # confirm exactly one inserted line
```

**Run:** `./docker/qemu/run-x86-boot-tests.sh 1`

**Expected result:** the boot completes normally through every prior marker
(the census leg now runs BEFORE `test "$passed" = true`, per the B4 fix in
this same round, so this mutation fires regardless of how the rest of the
boot goes), then the ERR trap fires on the now-false comparison and prints:
```
x86 frame-custody gate run 1: FAIL (set -e abort at .../run-x86-boot-tests.sh:<line>, exit 1)
  failing command: test "$CENSUS_VIRTIO_BLOCK" -eq "$EXPECTED_VIRTIO_BLOCK"
```
(the real parsed value, e.g. `3`, plus 99 = `102`, against an expected `3` —
matches the shape already proven for `run-x86-gate.sh`'s equivalent
assertion: `EVIDENCE-2026-08-31.md`'s table, "reports 102 VirtIO block
device(s), self-counted expected 3").

**Revert:** `git checkout -- docker/qemu/run-x86-boot-tests.sh`

**PROVE-FILL:** <Prove slot: paste the actual FAIL line and confirm the
revert diff is empty>

---

## Mutation 2 — `run-x86-boot-tests.sh`, network floor

Same file, same clone, right after Mutation 1's revert is confirmed clean.

**Apply:**
```bash
sed -i '' '/^    test "\$CENSUS_NETWORK" -ge 1\$/i\
    CENSUS_NETWORK=0
' docker/qemu/run-x86-boot-tests.sh
git diff docker/qemu/run-x86-boot-tests.sh   # confirm exactly one inserted line
```

**Run:** `./docker/qemu/run-x86-boot-tests.sh 1`

**Expected result:**
```
x86 frame-custody gate run 1: FAIL (set -e abort at .../run-x86-boot-tests.sh:<line>, exit 1)
  failing command: test "$CENSUS_NETWORK" -ge 1
```

**Revert:** `git checkout -- docker/qemu/run-x86-boot-tests.sh`

**PROVE-FILL:** <Prove slot: paste the actual FAIL line and confirm the
revert diff is empty>

---

## Mutation 3 — `run-aarch64-service-sequence-gate.sh`, MMIO census total

This leg has never executed even once (unmutated), so this is really two
things the Prove slot needs to do, in order:

### 3a. First reach the arm at all, unmutated

```bash
./docker/qemu/run-aarch64-service-sequence-gate.sh --boots 1 --profile cortex-a72 --rebuild
```

(`--rebuild` only needed if no `boot_tests`-feature aarch64 kernel binary is
already built and current; `cortex-a72` is the faster of the two profiles
for a single confirmation boot.) Read `$OUTPUT_DIR/cortex-a72/census.tsv`
(or the printed per-profile summary) for that one boot's `bucket` column —
it must read `GREEN`, and the boot's own serial
(`$OUTPUT_DIR/cortex-a72/serial-1.txt`) must contain a `[drivers] Found 5
VirtIO MMIO devices` line with both a `network` and a `block` device in the
per-device breakdown. This is the leg's FIRST EVER real execution; capture
the serial excerpt into
`docs/planning/green-program/nic-bus/serials/` alongside a short note in the
durable EVIDENCE doc (see the placeholder left there).

**PROVE-FILL:** <Prove slot: paste the classify_serial bucket + reason for
this boot, and the matching `[drivers] Found N VirtIO MMIO devices` line>

### 3b. Then mutate it

**Apply** (single hunk, right after the value is parsed from the real
serial output captured in 3a's own boot — mutate a fresh copy of the
already-run gate to reprocess the SAME serial with the leg's own logic if
the script supports offline reclassification, otherwise re-run one more
live boot with the mutation already applied):
```bash
sed -i '' '/mmio_census_total=\$(printf .%s\\n. "\$mmio_census_line" | sed -n .s\/.\*Found \\([0-9\]\*\\) VirtIO MMIO devices\.\*\/\\1\/p.)\$/a\
        mmio_census_total=$((mmio_census_total + 99))
' docker/qemu/run-aarch64-service-sequence-gate.sh
git diff docker/qemu/run-aarch64-service-sequence-gate.sh   # confirm exactly one inserted line
```
If the sed anchor above is fragile against the live file (quoting inside a
QEMU/quoted context is easy to get wrong from a prepared doc), the reliable
fallback is a manual one-line insert immediately after the line reading
```
        mmio_census_total=$(printf '%s\n' "$mmio_census_line" | sed -n 's/.*Found \([0-9]*\) VirtIO MMIO devices.*/\1/p')
```
(currently line 824 of `docker/qemu/run-aarch64-service-sequence-gate.sh` —
re-locate by content, not line number, since this fix round may shift lines
above it) — insert directly below it:
```
        mmio_census_total=$((mmio_census_total + 99))
```

**Run:** `./docker/qemu/run-aarch64-service-sequence-gate.sh --boots 1 --profile cortex-a72`

**Expected result:** `classify_serial` for that boot reports
`CLASS_BUCKET=UNATTRIBUTED` with
`CLASS_REASON="device-enumeration census reports 104 VirtIO MMIO device(s),
self-counted expected 5 from this script's own -device flags"` (the real 5 +
99 = 104, exact same arithmetic shape as `run-aarch64-full-test.sh`'s
already-proven mutation), and the profile gate reports
`Profile cortex-a72 gate: FAILED (... UNATTRIBUTED=1 ...)`.

**Revert:** `git checkout -- docker/qemu/run-aarch64-service-sequence-gate.sh`

**PROVE-FILL:** <Prove slot: paste the actual CLASS_BUCKET/CLASS_REASON and
the profile gate's FAILED line, and confirm the revert diff is empty>

---

## After all three mutations are captured and reverted

Update `docs/planning/green-program/nic-bus/EVIDENCE-2026-08-31.md` §4's
table (already restated honestly by this fix round, with `PROVE-FILL`
placeholders — see that file) with the real FAIL lines captured above, and
flip the two `PROVE-FILL` markers there to state plainly that both legs are
now mutation-proven and the service-sequence census arm has now executed
(clean and mutated) at least once. If `git diff --stat` after all three
reverts is not empty, STOP and do not proceed to any other work — that means
a mutation was left applied.
