# Native board evidence, schema 1

E19 milestone 4 PR 4.1 identifies the board and connected sink. It leaves the
firmware console active. Allocation, native modes, latching, display CRC and
post-handoff firmware-write accounting belong to PRs 4.2–4.5.

The boot CPU emits one `GT710_NATIVE <JSON>` line after live DDC succeeds on
an active on-board TMDS route. `GT710_PROBE` lines report failure prerequisites;
they cannot pass the inventory case. The existing boot record binds this
capture to the kernel commit, tools commit, PCI BAR census and uninterrupted
console transcript. The board case requires those kernel and tools commits
to match, a clean completed boot, and recomputes its verdict from the retained
capture. Use the existing host `gt710-suite --boot ID --sha SHA` collector after
a console suite boot; it does not execute the suite again.

| Field | Source and meaning |
|---|---|
| `schema`, `phase` | `1`, `board`; this is inventory evidence |
| `bdf` | PCI display function, joined to the firmware framebuffer owner and claim |
| `boot0`, `chipset`, `revision` | BAR0 BOOT0 and its decoded GK208B chipset/revision |
| `post` | Raw 0x2240c; bit 1 is the devinit-complete indicator |
| `rom_bytes`, `rom_sha256` | Length and SHA-256 of the legacy PCIR image read through the direct PROM aperture; ROM bytes are not emitted or redistributed |
| `bit_version`, `dcb_version`, `i2c_version`, `connector_version`, `pll_version` | Parsed, bounded table versions |
| `dcb_entry`, `dcb_conf`, `i2c_entry` | Selected route's raw table records |
| `connector`, `connector_type`, `drive`, `sor` | Board connector index/type, DDC port and SOR, decoded from those tables |
| `sor_state`, `head` | Active 0x640200 + SOR*0x20, including TMDS protocol and head mask; asynchronous state is not used |
| `head_total`, `head_clock` | Active 0x640414 / 0x640450 + head*0x300, raw inherited totals and pixel clock |
| `ddc` | `live`; read from the actual routed pins during this boot |
| `edid`, `edid_sha256` | Complete hexadecimal base and advertised extension blocks, and SHA-256; each block checksum is checked |
| `modes` | Sink-advertised 1280x800 and 2560x1600 timings, each with width, height, clock_khz, hsync_start/end, htotal, vsync_start/end, vtotal and EDID flags |

The scorer independently derives detailed timings and the supported DMT
standard timing from the raw EDID and compares the complete records. Missing
modes, a truncated block, wrong checksum/hash, stale duplicate record, inactive
SOR, contradictory route or absent POST bit fail the case. EDID is sink
capability evidence; it does not establish that new timings were programmed
or that pixels reached the connector. Active registers here describe the
inherited mode, not native modeset acceptance.

The implementation supports digital EDID 1.3/1.4, up to three extension blocks,
base detailed timings, CTA detailed timings, and DMT 1Ch in the base standard
timing list. Unsupported descriptions do not invent modes. Missing support
or a physical sink prerequisite is retained as a failed inventory result.
The metal record retains EDID privately; public summaries omit sink identifiers.

The ROM window and shared-pad state are restored after access. DDC stretches
are bounded; NACK/timeout returns no partial block and releases the lines.
No firmware framebuffer, scanout, clock, output or channel programming is
performed by this inventory path.

Source revisions, file hashes and notices: [board provenance](provenance/gt710-board.md)
and the repository's `THIRD_PARTY_NOTICES`.
