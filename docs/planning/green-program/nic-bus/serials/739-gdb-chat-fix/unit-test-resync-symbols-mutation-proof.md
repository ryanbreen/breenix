# #739 review B1-B3: mutation proof for the corrected `resync_symbols()` test

The sweep-3 review (findings B1, B2, B3) established:

- **B1**: `_load_symbols_at_runtime_addr()` computed runtime addresses from
  the class constant `self.KERNEL_BASE_X86` instead of the instance
  attribute `self.kernel_base_x86` that `resync_symbols()` corrects, so the
  reload always re-added symbols at the original (possibly wrong) guess.
- **B2**: the original unit test stubbed out
  `_load_symbols_at_runtime_addr` itself -- the exact function carrying the
  B1 bug -- so its assertion (`assert any("0x8000000000" in c for c in
  sent_commands)`) could never fail regardless of whether B1 was fixed; the
  stub manufactured the string the assertion grepped for out of the
  attribute the caller had just set.
- **B3**: `self._last_symbol_text_addr` was initialised to `None` and never
  assigned anywhere, so the `remove-symbol-file` cleanup guard in
  `resync_symbols()` never fired.

## Fix

- `kernel_base_x86` (lowercase, instance attribute) is now used at both
  address-calculation sites in `_load_symbols_at_runtime_addr()`.
- `_load_symbols_at_runtime_addr()` now assigns
  `self._last_symbol_text_addr = text_addr` after emitting `add-symbol-file`.

## Non-vacuous test

`unit-test-resync-symbols.txt` (same directory) was rewritten per the
review's own suggested fix: it stubs only `_parse_elf_sections` (a leaf ELF
reader that would otherwise need a real kernel binary) and `_send_raw` /
`get_all_serial_output` (I/O boundaries). **`_load_symbols_at_runtime_addr`
-- the function under test -- is NOT stubbed**; it runs for real, and the
assertions inspect the actual `add-symbol-file` / `remove-symbol-file`
strings it sends to (fake) GDB.

## Mutation proof (run this session)

The identical test file was run twice: once against the pre-fix code at
this branch's prior HEAD (`4f32b121`, commit before this fix round) via a
scratch copy, and once against the fixed `gdb_chat.py`.

**Against the pre-fix code (`4f32b121`) -- RED:**

```
initial add-symbol-file command: ['add-symbol-file /fake/kernel.elf 0x10000001000 -s .rodata 0x10000002000 -s .data 0x10000003000 -s .bss 0x10000004000']
Traceback (most recent call last):
  ...
AssertionError: B3: _last_symbol_text_addr must be set by _load_symbols_at_runtime_addr so a later resync's remove-symbol-file guard can fire; got None
```

The test fails at the very first assertion after the simulated connect-time
load, before `resync_symbols()` is even called -- proving B3 alone would
have been caught. (B1 is caught by a later assertion in the same run, once
B3's `None` no longer short-circuits the test; both are exercised in the
fixed run below.)

**Against the fixed code -- GREEN:**

```
initial add-symbol-file command: ['add-symbol-file /fake/kernel.elf 0x10000001000 -s .rodata 0x10000002000 -s .data 0x10000003000 -s .bss 0x10000004000']
old_base       = 0x10000000000
result         = {'success': True, 'resynced': True, 'verified': True, 'old_base': '0x10000000000', 'base': '0x8000000000', 'message': 'Symbols were loaded at the WRONG base 0x10000000000; corrected to 0x8000000000'}
gdb commands   = ['remove-symbol-file -a 0x10000001000', 'add-symbol-file /fake/kernel.elf 0x8000001000 -s .rodata 0x8000002000 -s .data 0x8000003000 -s .bss 0x8000004000']
result2 (idempotent call) = {'success': True, 'resynced': False, 'verified': True, 'base': '0x8000000000', 'message': 'Already resynced at this base'}
ALL ASSERTIONS PASSED: resync_symbols() correctly discovers and corrects a wrong guess using real #737 boot data, exercising the REAL _load_symbols_at_runtime_addr() (not stubbed).
```

The corrected reload uses `0x8000001000` (`0x8000000000` + `.text` offset
`0x1000`) -- the CORRECTED base -- not `0x10000001000` (the stale guess).
`remove-symbol-file -a 0x10000001000` fires against the original address
before the corrected `add-symbol-file`, proving B3's cleanup guard is now
live. This is red-on-old / green-on-new: a mutation-style proof, not a
self-referential stub.

## #739 close comment status

The original close comment's claim "reissues `add-symbol-file` at the
corrected base" is now actually true and actually evidenced (it was not,
per review finding B2, at the time it was posted -- see the correction
comment posted on #739 in this fix round).
