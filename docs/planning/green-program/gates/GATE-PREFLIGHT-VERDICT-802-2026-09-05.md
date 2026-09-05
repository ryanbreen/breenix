# The x86 production-profile gate's base-dir preflight now fails through the verdict path (#802)

## What was red

`tests/teardown_structure.rs::x86_production_profile_gate_verdict_discipline_holds`
failed on `main` at `2a444455` (the #801 merge): 82 passed, 1 failed. The test
prints its reason on stderr before it panics:

```
x86 production-profile gate gained a pre-empting exit: exit 1
```

#801 added two preflight checks on the operator-controlled `BREENIX_GATE_TMP`
to `docker/qemu/run-x86-prod-profile-boot-test.sh`, next to the assignments
that derive their subjects (review findings F6 and F7 on #797). At that point
in the script — line 208 and line 224 on `main` — the only way to stop was a
bare `exit 1`, because `report_gate_failure` is defined ~550 lines further
down and the `ERR` trap that calls it is installed after that definition.

## The rule

The ratchet's scan reads, verbatim (`tests/teardown_structure.rs:15336-15346`):

```rust
    // No exit may pre-empt the verdict. The trap's re-raise is the only one this
    // gate needs, and it runs after a verdict has already been found false.
    for line in script.lines() {
        let statement = line.trim();
        if statement.split_whitespace().next() == Some("exit")
            && statement != "exit \"$exit_code\""
        {
            eprintln!("x86 production-profile gate gained a pre-empting exit: {statement}");
            return Err(());
        }
    }
```

It sits inside `validate_x86_prod_profile_harness`, whose doc comment says why
this gate in particular is pinned this way: it "is the only x86 boot that ever
executes the shipped zero-feature kernel, so a silent abort inside it is worse
than in the boot-test harness: there is no second x86 gate that would catch the
same regression."

The rule is about the *verdict*, not about the token `exit`. The gate records
`reached=false`, raises `reached=true`, and spends the verdict at
`test "$reached" = true`; the machinery that turns a red into readable output
is `report_gate_failure`, which prints the `x86 production-profile gate: FAIL
(...)` line, names the failing command, preserves the serial and re-raises the
status. An `exit` that runs outside that machinery can end the gate with no
verdict line behind it, which is the state the rule exists to keep out.

## The fix

The two checks moved to a `BASE-DIR PREFLIGHT` block placed immediately after
the `ERR` trap is installed, so they are the first commands to run under the
handler, and they reject with the `echo` + bare `false` shape this script
already uses for its missing-userspace-artifact preflight further down:

```bash
trap 'report_gate_failure "$LINENO" "$BASH_COMMAND"' ERR

# --- BASE-DIR PREFLIGHT (#797 F6/F7, routed through the verdict path by #802) -
...
case "$BREENIX_GATE_TMP" in
    /*) ;;
    *) echo "x86 production-profile gate preflight: BREENIX_GATE_TMP must be an absolute path, got: $BREENIX_GATE_TMP" >&2
       false ;;
esac
if [ "${#CONSOLE_SOCK_PATH}" -gt 107 ]; then
    echo "x86 production-profile gate preflight: console socket path \"$CONSOLE_SOCK_PATH\" is ${#CONSOLE_SOCK_PATH} chars, over the AF_UNIX sun_path limit of 107 -- shorten BREENIX_GATE_TMP" >&2
    false
fi
```

A bare `false` under `set -e`/`set -E` fires the `ERR` trap, so the rejection
is spent through `report_gate_failure`: the operator sees the specific
diagnostic (which value, how long, what to shorten) followed by the gate's own
`FAIL` verdict line and a nonzero status. The script now carries one `exit`,
the trap's re-raise `exit "$exit_code"`, which the rule admits by name.

The assignments the checks judge (`BREENIX_GATE_TMP`, `OUTPUT_DIR`,
`CONSOLE_SOCK_PATH`) stay where #801 put them, because `report_gate_failure`
reads `OUTPUT_DIR` and `BREENIX_GATE_TMP` in its own failure path and must see
them defined.

## The preflight purpose is unchanged

F6 is about cd-order: `OUTPUT_DIR` is computed before this script's
`cd "$BREENIX_ROOT"`, so a relative `BREENIX_GATE_TMP` would resolve
differently depending on which side of that `cd` read it. The preflight block
sits at line 799-831 and the `cd` is at line 944, so the absolute-path check
still runs on the same side of the `cd` it did before. F7 is about failing
before a wasted build: the first `rm` in this script is the `rm -f` of the
stale UEFI image at line 965, immediately ahead of the `cargo build`, so both
checks still reject a bad value ahead of the build and the boot. A second, timed pass over the
same two invocations (`date +%s.%N` either side, transcript
`serials/802-2026-09-05/preflight-timing.txt`) returned in 0.030 s and
0.029 s, and neither left its `$OUTPUT_DIR` behind (`ls -d` on each reported
"No such file or directory" afterwards), so the build step was not reached.

## The sibling `exit` the scan cannot see

The absolute-path check's `exit 1` was on a line beginning with the case label
`*)`, so `split_whitespace().next()` returned `Some("*)")` and the scan did not
report it — only the `sun_path` check's line-leading `exit 1` reddened the
test. Both are converted here. Leaving the first one in place would have kept
the same defect in the gate in a shape this ratchet happens not to read, which
is a worse state than the red that was filed.

## Scope disclosure: the seven sibling scripts

#797 put the same absolute-path guard into eight gate scripts, and the
`sun_path` guard into two. This change converts the two guards in
`run-x86-prod-profile-boot-test.sh` only. The other seven scripts keep the
`case ... exit 1` / `if ... exit 1` shape #797 gave them (claim-lint:ok: #802,
`grep -c 'BREENIX_GATE_TMP must be an absolute path'` returns 1 in each of the
8 files, of which 1 is this gate). That is a scope statement, not a claim that
those seven are correct: `x86_production_profile_gate_verdict_discipline_holds`
is the one ratchet in `tests/` that pins verdict discipline for a gate script,
and it names this gate alone, so whether each sibling's preflight should also
print that gate's own `FAIL` line is a judgement about that gate's verdict
model rather than a red this branch is carrying.

## Evidence

### Mac (this branch's head)

| command | result |
|---|---|
| `scripts/run-structure-tests.sh teardown_structure` | exit 0, 83 passed / 0 failed |
| `tests/*_structure.rs` sweep, 29 files | 29 of 29 green, 542 cases, 0 failed |
| `scripts/claim-lint.py` | exit 0 |

The single test named in #802 is inside that 83 and passes at this head; the
82/1 split quoted at the top is `main` at `2a444455`.

### Beast (`breenix-x86` Incus VM, clone `/root/breenix-health`)

Head `4b6b82d42462635a5296f54524144518db5fa6b6`; the gate script's sha256 in
the clone, `131d1304db8a1a78b4d36db06022a311d6a007458c5ba72d192a953c29ba0764`,
matches this branch's working tree. Full transcript:
`serials/802-2026-09-05/driver-transcript.txt`.

1. **Clean build.** `cargo build --release --features
   testing,external_test_bins --bin qemu-uefi` returned 0 and
   `grep -E "^(warning|error)"` over its log printed no line (grep exit 1 =
   no match), per the driver transcript's STEP 1 block.

2. **Default gate run**, `BREENIX_GATE_TMP=/root/gate-tmp-802`, 1 run: exit 0
   with the verdict line

   ```
   PASS: x86 production profile reached steady state with the teardown census at rest
   ```

   sha256 of the image it booted (R17):
   `d1b3eb0e4461845bcb01381e8c6a66439c54924f7641df9ccfc242ac21abd71e`
   (`target/release/build/breenix-a14bb21948d9e08d/out/breenix-uefi.img`).
   Liveness sample in the same run: console prompt count 1 -> 2 over 60s.
   Framing and marker census: `serials/802-2026-09-05/gate-run1-head.txt` and
   `gate-run1-tail.txt`.

3. **Simulated preflight failure, over-length path.** `BREENIX_GATE_TMP` set
   to a 109-character directory, which makes the console socket path 147
   characters:

   ```
   x86 production-profile gate preflight: console socket path "/root/gate-tmp-802-ggg...ggg/breenix_x86_prod_profile/console.sock" is 147 chars, over the AF_UNIX sun_path limit of 107 -- shorten BREENIX_GATE_TMP
   x86 production-profile gate: FAIL (set -e abort at docker/qemu/run-x86-prod-profile-boot-test.sh:830, exit 1)
     failing command: false
   ```

   exit status 1; the timed repeat of this invocation took 0.030 s. This is
   the condition the F7 preflight checks, and the gate's `FAIL` verdict line
   is now on the way out.

4. **Simulated preflight failure, relative path.** `BREENIX_GATE_TMP` set to
   `relative-not-absolute`:

   ```
   x86 production-profile gate preflight: BREENIX_GATE_TMP must be an absolute path, got: relative-not-absolute
   x86 production-profile gate: FAIL (set -e abort at docker/qemu/run-x86-prod-profile-boot-test.sh:826, exit 1)
     failing command: false
   ```

   exit status 1; the timed repeat took 0.029 s. This is the F6 leg, which the
   ratchet's scan could not see before.

Both rejection legs returned before the build step, so neither produced a
serial for `report_gate_failure` to preserve; the handler's `compgen -G`
lookup over `$OUTPUT_DIR/serial_*.txt` matches no file and the handler skips
its preservation block, which is why the transcripts above are three lines
rather than a serial tail.

## Claim-lint

```
claim-lint: scripts/claim-lint.py -> exit 0
```
