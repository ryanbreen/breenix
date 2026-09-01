# #739 — gdb_chat.py hardcoded PIE base offset — fix evidence

## What was wrong

`breenix-gdb-chat/scripts/gdb_chat.py` used a fixed `KERNEL_BASE_X86 =
0x10000000000` (1 TiB) as the x86_64 kernel's runtime load base when calling
`add-symbol-file`. The bootloader crate actually loads the kernel PIE at a
runtime-chosen free virtual address slot (`Mapping::Dynamic` in
`bootloader`'s `common/src/load_kernel.rs`) and only reveals the real value
once it has run, by printing `virtual_address_offset: 0x...` on serial. This
value is not knowable at the moment `gdb_chat.py` loads symbols (QEMU is
still halted at the reset vector via `-s -S`), so the fixed constant was
always a guess -- one that is wrong whenever the bootloader picks a
different free slot than the guess. When wrong, GDB gives NO error:
`info symbol`/`backtrace` silently resolve against the wrong base (or fail
to resolve at all), with nothing to distinguish that from a genuinely
correct session.

Real historical evidence of two different offsets from otherwise-identical
boots: `../702-rca/RCA-2026-08-31.md` ("Harness correctness notes") and the
#737 specimen itself, `../702-rca/anomaly_exited_114/serial_user.log.txt`,
line 16: `virtual_address_offset: 0x8000000000` -- different from
`gdb_chat.py`'s hardcoded `0x10000000000`.

## The fix

`gdb_chat.py`:
- `KERNEL_BASE_X86` is still the initial guess used at connect time (QEMU is
  halted before the bootloader has run, so nothing better is available yet),
  but the `start()` response now says so explicitly:
  `"symbols": "loaded at UNVERIFIED guessed base ..."` and
  `"symbols_verified": false`.
- New method `GDBChat.resync_symbols()` and stdin special command
  `resync-symbols`: parses the real `virtual_address_offset:` line out of
  the accumulated serial output, and if it differs from the guess in use,
  drops the symbol table loaded at the wrong base (`remove-symbol-file -a`)
  and reloads it at the confirmed base. Idempotent -- a second call with the
  same serial content is a no-op and issues no further GDB commands.
- `CLAUDE.md`'s "Symbol Loading" section and GDB Chat Tool description no
  longer state the fixed base as fact; they document `resync-symbols` as a
  required step before trusting `info symbol`/`backtrace`.

`gdb_session.sh` (found while validating the fix live through the documented
interface, fixed in the same commit since it directly blocked using
`resync-symbols` through the documented `gdb_session.sh` interface):
- `start_session()`'s background pipeline used to merge gdb_chat.py's stdout
  (one JSON object per line, the wire format `send_command`/`start_session`
  parse) with its stderr (free-form `[INFO]`/`[DEBUG]` diagnostic writes)
  into the same `OUTPUT_FILE` via `2>&1`. Depending on stdout/stderr
  interleaving timing, a stderr line could land as line 1 (breaking
  `start_session`'s `head -1 | json.load` readiness check) or get counted as
  a command's response (breaking `send_command`'s "did a new line appear"
  polling). Now stdout and stderr go to separate files
  (`output.jsonl` / `stderr.log`).
- `stop_session()`'s x86_64 QEMU cleanup used `pkill -9 qemu-system-x86_64`
  (no `-f`). `qemu-system-x86_64` is 19 characters; `pkill` without `-f`
  matches only the truncated 15-character `comm` field, so this could never
  match anything -- the x86_64 branch's QEMU cleanup was silently a no-op.
  Changed to `pkill -9 -f "qemu-system-x86_64"` (safe from the self-match
  gotcha documented in `../702-rca/RCA-2026-08-31.md`, since
  `gdb_session.sh`'s own invocation argv never contains that literal
  pattern).

## Evidence in this directory

- `unit-test-resync-symbols.txt` -- a standalone Python script that loads
  `GDBChat` directly (no live QEMU/GDB) and feeds it the REAL captured
  serial output from the #737 specimen (`virtual_address_offset:
  0x8000000000`), starting from the class-default guess
  (`0x10000000000`, deliberately wrong for this fixture). Asserts
  `resync_symbols()` discovers the mismatch, corrects `kernel_base_x86` to
  `0x8000000000`, reissues `add-symbol-file` at the corrected base, reports
  `resynced: true`, and that a second call is a no-op (`resynced: false`,
  no further GDB commands issued). This is the "boot picked a different
  offset than the guess, and the tool corrects it" case from the acceptance
  criteria, proven against genuine historical mismatch data rather than a
  synthetic fixture.
- `unit-test-resync-symbols-output.txt` -- that script's output
  (`ALL ASSERTIONS PASSED`).
- `live-two-boot-test-beast-20260901.txt` -- a live end-to-end run on beast
  (`breenix-x86` container, `main`@`8b02ea29`'s already-built release
  binary), driven entirely through the documented `gdb_session.sh`
  interface: two independent boots, each `start` (halted at reset, base
  UNVERIFIED) -> `continue` (interrupted via SIGINT once serial shows the
  bootloader has run) -> `resync-symbols` -> `info symbol $pc` / `backtrace`.
  Before resync, `info symbol $pc` correctly and loudly reports "No symbol
  matches $pc" (nothing has executed against the loaded symbol table yet)
  rather than silently returning something plausible-looking. After resync,
  both boots resolve to real, correct kernel function names
  (`kernel::arch_impl::x86_64::timer::calibrate + 372`,
  `kernel::logger::LogFrameBuffer::write_pixel + 593`) with a working
  backtrace. Both boots on this host/QEMU/OVMF combination happened to pick
  the same free slot (`0x10000000000`, matching the class default) --
  `resync-symbols` correctly reports `"resynced": false, "verified": true`
  for that case (confirmed the guess rather than blindly trusting it),
  which is the companion case to the corrected-mismatch unit test above.
- `probe-six-more-boots-beast-20260901.txt` -- six additional quick boots
  (offset-only, same host) for additional samples; all six also landed on
  `0x10000000000` on this particular beast container/QEMU/OVMF combination.
  The boot-to-boot slot choice is a property of the UEFI memory map handed
  to the bootloader (host/QEMU/OVMF/build specific), not something this
  fix's test window can force either way -- the mismatch-correction case is
  covered by the unit test above using real historical data from a
  different combination that did pick a different slot.

## What was NOT changed

`gdb_chat.py`'s pre-existing "no automatic interrupt" design (a plain
`continue` blocks until the target stops or the 300s timeout) is untouched.
Interrupting an in-flight `continue` still requires sending `SIGINT` to the
`gdb_pid` from the `start()`/`resync-symbols` response directly (a separate,
pre-existing mechanism already noted in `gdb_chat.py`'s own `execute()`
docstring: "the agent can always issue Ctrl+C via a separate mechanism if
needed"). This is unrelated to #739's scope (the wrong base, not the lack of
an interrupt command) and was exercised as-is by the live test above.
