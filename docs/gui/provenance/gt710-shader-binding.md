# GK208B diagnostic shader binding (M7 PR 7.3 unit 6)

This unit adds program binding after the final clean CE upload. It does not
issue a draw. The CE reader drains and releases channel 0 and its BAR alias
before the existing graphics channel reactivates. Its fifth GPFIFO entry
binds A197, sets the program-region upper/lower GPU address, enables vertex
slot 1 and fragment slot 5, sets their SPH offsets, and sets their GPR counts
from the committed asset JSON. The existing epoch/nonce report releases and
SET_REFERENCE follow those methods on subchannel 0.

The header offsets are 0x30 and 0x1b0; their first instructions are at 0x80
and 0x200. The committed metadata supplies 8 vertex and 4 fragment registers.
The graphics VM already maps this allocation before channel admission.
The original channel record is captured before this additional submission;
this unit does not change the `GT710_GR` schema or its pending markers.

PGRAPH interrupt/trap/data-exception/GPC-exception words at 0x400100,
0x400108, 0x400110 and 0x400118 are read before binding, while waiting, and
after submission, before drain/reset. A nonzero word stops the probe;
this path does not acknowledge or clear a trap. A failed submission retains
channel ownership for the existing drain/reset cleanup.

`GT710_SHADER` is one JSON record with eight committed asset hashes,
code address/offsets/GPRs, the eight two-poison read-back digests (four upload
cases), release receipt, before/after trap words, error, and
`draw: "pending-7.4"`. `GT710_SHADER_BULK` records the byte length and SHA-256
of the newline-terminated `GT710_SHADER_IMAGE` rows. Those rows enter the
existing network-upload buffer, including the final newline, and contain
the bytes from the device read-backs. Serial carries hashes, not image bytes.

The host literal stream check is `tests/gt710_shader_bind_test.rs`, compiled
with `rustc --edition=2021 --test`. Hardware execution and the console-suite
330-second deadline remain unmeasured in this no-boot unit.

## Checked sources

The MIT file-local grants were read; THIRD_PARTY_NOTICES records the source
revisions, copyright and consumers. `nvc0_state_validate.c` was not read or
used. The following SHA-256 values identify the source bytes checked.

| Source | SHA-256 |
|---|---|
| NVIDIA open-gpu-doc cla197.h | `a0efb2656a3810ae3e26918e763b2cb0df62c5ad9f7cd4f693ba8170fb33b7ad` |
| Mesa nvc0_program.c | `394dbcc6a3ebaf67f8bcf8e2513d2f6ea5e2a8d5c64ad8f82eaad57ab4808c6d` |
| Mesa nvc0_screen.c | `1bf8122916bd3bdf2cb1fe5d0584f40ebf4eb96bc9bce9a200f14b0d4d452a85` |
| Nouveau GR gf100.c | `2bfb440995838d50d6497d6eedfbf1899be22e4fbeacbd23cfbfc189f5be7602` |
