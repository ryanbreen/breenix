# GK208B diagnostic shader binding (M7 PR 7.3 unit 6)

This unit adds program binding after the final clean CE upload. It does not
issue a draw. The CE reader drains and releases channel 0 and its BAR alias
before the graphics channel is created. The graphics channel stays active
through its release controls and shader binding, then drains once. Its fifth GPFIFO entry
binds A197, sets the program-region upper/lower GPU address, enables vertex
slot 1 and fragment slot 5, sets their SPH offsets, and sets their GPR counts
from the committed asset JSON. The existing epoch/nonce report releases and
SET_REFERENCE follow those methods on subchannel 0.

The header offsets are 0x30 and 0x1b0; their first instructions are at 0x80
and 0x200. The committed metadata supplies 8 vertex and 4 fragment registers.
The graphics VM already maps this allocation before channel admission.
The channel record is captured after binding and FIFO quiesce;
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
330-second deadline require the completed acceptance capture.

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

## Failed-boot lifecycle repair

The failed acceptance capture `20261028T051946Z-metal` reports successful
shader upload read-backs followed by an unconsumed GR binding stream. Its
post-probe PFIFO GR engine state at 0x2640 is 0x40000000: the faulted bit in
MIT nouveau v6.8 `nvkm/engine/fifo/gk104.c:gk104_engn_status`.
The earlier channel-drained snapshot has that word clear. PGRAPH trap words
before and after binding are clear. These observations locate the failure
between the first GR drain and the attempted binding, but do not identify
the internal hardware reason for the scheduler fault.

The orchestration had drained GR, reused channel 0 for CE, then rebound the
retained GR session. FIFO quiesce does not retire FECS's current context:
MIT `vendor/kepler/nouveau/hub.fuc:ctx_load` retains it, and graphics-session
retirement requires reset. The repair finishes shader upload before graphics
admission and submits the fifth entry on the same active graphics session.
The host production-slice check covers this ownership order, shader binding,
injected submission/drain errors, and reset-before-reclamation. It does not
emulate GPU execution. A failed drain retains storage and the original error;
the read-only unwind snapshot includes FIFO binding, preempt and PBDMA words.

The userspace wait probe now reaps a child that exits before queuing and
prints `GT710_KILLED_WAIT_ERROR`, allowing the console to continue. It emits
no successful killed-wait receipt on this path, so the existing scorer still
rejects the missing evidence.

## Program-fetch permissions

Boot `20261028T054537Z-metal` at `30688b63` still failed binding after
the lifecycle repair. Its unwind snapshot has PFIFO interrupt 0x50000000
(including VM-fault bit 28), engine state 0xc000c000, and PBDMA GET 5 with
reference 4. PGRAPH trap words are clear. The capture did not retain the
MMU fault address/reason, so those words alone do not identify the fault.

The graphics constructor mapped shader code with `privileged: true`,
alongside the internal context buffers. The shader allocation now has ordinary
program-fetch permissions; internal context buffers retain their privileged
mappings. The program VA is passed explicitly to the constructor, rather
than inferred from backing order. `tests/gt710_shader_mapping_test.py` runs
the production mapping loop and PTE encoder with varied program addresses.
Its first completed run rejected the shader PTE's privilege bit. This is a
permission check, not a simulated hardware acceptance.

The source is nouveau v6.8 `nvkm/subdev/mmu/vmmgf100.c:249-265,314-316`
(MIT, Copyright 2017 Red Hat Inc.), checked at the pinned revision listed
above. GR `gf100.c:383` explicitly requests privileged context-buffer maps.
The existing read-only snapshot now includes pending MMU fault records from
FIFO `gf100.c:696-714`, without acknowledging them, so any subsequent
failure retains its address, client and reason before teardown.
