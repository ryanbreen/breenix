# A.5 u6c: ARM data-abort core-dump status

Issue #2926: the ARM uncaught data-abort fallback passed a bare negative signal
to process termination. Use the existing `fault_death_exit_code` helper, as
x86 does, so waitpid reports the core-dump bit for SIGSEGV and SIGBUS.

Read the full catch-up plan, U1/U2/U16/U19/U28/U29/U31, G5, the debugging
protocol, ARM ledger lines 243, 257, 458 and coordinator ruling 459, and the
u5 report. Continued `arm-catchup/a-5` from `6959f1d9`. Fetch found main at
`d3041e9e`, already an ancestor; no merge or rebase was needed.

## Code and retained evidence

The retained u5 capture is Vigil `20261113T040422Z-qemu-87249`, commit
`be737c42`. Its private serial at `/tmp/a5-u5-baseline-be737c42/serial.txt`
records an EL0 data abort (DFSC `0x7`) at line 5516, then
`desktop/crash-isolated FAIL ... status=11 expected=139` at line 5566.

`kernel/src/arch_impl/aarch64/exception.rs:1028-1035` classifies the abort,
attempts signal delivery, then calculates the fallback termination status.
Before this repair it used `-(el0_sig as i32)`. The resulting status is passed
to peer termination, exit tracing and `exit_process_and_retire` at lines
1249-1252. The adjacent handled-fault route already uses the shared helper
(`act_on_raised_el0_fault_signal`, line 268).

x86's page-fault fallback uses `fault_death_exit_code(SIGSEGV)` at
`kernel/src/interrupts.rs:1558`. Its other user-fault paths use that helper
at lines 640 and 1440. `kernel/src/signal/delivery.rs:1758-1770` sets bit
`0x80` for CoreDump default actions, producing -139 for SIGSEGV.
`kernel/src/syscall/wait.rs:290-295` preserves that bit; `complete_wait` calls
this encoder at line 413. The shared code already implements the expected
encoding, so this unit changes only the ARM producer.

## Check first, then repair

First commit `2bb4f535` adds `tests/arm64_abort_wait_status_test.py`. Its one
host test compiles the production abort classification and status calculation,
shared default-action and fatal-exit helpers, and shared wait-status encoder.
It checks uncaught unmapped and misaligned accesses (SIGSEGV and SIGBUS),
requiring wait status 139 and 135 respectively, plus parity with the helper
x86 uses. No guest, device or scheduler execution is claimed by this harness.

Second commit `229ea3dd` changes only the ARM termination calculation.
On `6959f1d9` the check's Rust harness compiled with `-Dwarnings`, then failed at the
unmapped-access assertion: actual 11, expected 139. After the one-line ARM
repair, the unchanged test passed, including SIGBUS. Evidence:
`/tmp/a5-u6c-red.log` (exit 1) and `/tmp/a5-u6c-green.log` (exit 0).

## Validation and reach

Both release build commands use `CARGO_BUILD_JOBS=4` and `taskpolicy -b` on
this Mac:

```sh
cargo build --release --target aarch64-breenix-kernel.json \
  -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64
cargo build --release --target x86_64-unknown-none \
  -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel
```

ARM completed with exit 0, no source warnings and the existing pinned-core
future-incompatibility notice (`/tmp/a5-u6c-arm-build.log`). The x86 release
kernel build completed with exit 0 and zero warning/error records
(`/tmp/a5-u6c-x86-kernel-build.log`). No userspace or image rebuild is needed
for this kernel-only change. x86 consumes the committed
Kepler assets through the production build script's Python hash verifier;
no asset toolchain is rebuilt or installed (G5).

The first x86 invocation mistakenly used the userspace target
`x86_64-breenix.json` (os=breenix). It exited 101 with 108 errors and 3
warnings because kernel GPU definitions gated on `target_os="none"` were
excluded. This was a command error; no source was changed to accommodate it.
`Cargo.toml`'s kernel artifact specifies `x86_64-unknown-none`, used by the
corrected command above. The failed invocation is retained at
`/tmp/a5-u6c-x86-build.log`.

The runtime change reaches ARM uncaught data-abort termination; x86 source and
shared production code are unchanged. No gui-apps case, manifest, scorer,
threshold, GPU asset or build recipe changes. No boots, debugger attachments,
PR creation or merge in this unit. Guest acceptance remains with the final-head
acceptance unit; #2926 stays open pending that result. #2927 remains for its
subsequent unit under coordinator ruling 459.

Claim-lint invocations:

- `python3 scripts/claim-lint.py --base 6959f1d9` -> exit 0 before the check
  commit and after the repair; 0 findings in this unit's changed lines.
- `python3 scripts/claim-lint.py --commit-msg /tmp/a5-u6c-check-msg.txt` -> exit 0.
- `python3 scripts/claim-lint.py --commit-msg /tmp/a5-u6c-fix-msg.txt` -> exit 0.
- `git diff --check` -> exit 0 before the check commit and after the repair.
- Final unit-diff claim lint, report-message lint and whitespace checks ->
  exit 0; claim-lint reports 0 findings in this unit's changed lines.

The x86 runtime is unchanged, so this repair reaches no R240 suite. Tests and
builds are not repeated for documentation commits or the push.
