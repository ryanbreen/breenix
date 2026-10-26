# GK208B display-memory reservation basis

E19 PR 4.2 retains the raw inherited display and BAR controls read-only.
The PRAMIN window selector is changed temporarily and restored; physical
writes target the six owned objects. The `Access` test records writes and
checks their containment (`tests/gt710_memory_layout_test.rs`).

## G4 source lookup and selected path

The checked envytools revision is
`f102b82381f3f11cee113d16374c87091db039d9`:

- `rnndb/display/g80_pdisplay.xml` imports the MIT `rnndb/copyright.xml`.
  Its `OBJECTS` register at display offset `0x10` has no fields and is scoped
  before GF119. It does not define GK208's inherited value `0x1`.
- `rnndb/bus/pbus.xml`, with the same imported MIT grant, describes GF100+
  `HOST_MEM.BAR1` (`0x1704`) and `BAR3` (`0x1714`): bit 31 selects direct
  VRAM versus VM. The direct-mode low bits are not an active VM instance.

The checked NVIDIA open-gpu-doc revision is
`9fdf5c4062007929d9f4e6cbad9c9771fe61b880`.
`manuals/volta/gv100/dev_display_withoffset.ref.txt` has an MIT header,
Copyright (c) 2021 NVIDIA CORPORATION. At `NV_PDISP_FE_INST_MEM0`
(`0x610010`) it defines target 1 as NVM and bit 3 as validity. However, it
puts the address in a separate register. That later-generation definition
is not used to decode Kepler's combined instance/address register.

PR 4.2 therefore uses G4's conservative carveout path for the opaque raw
value `0x1`: require a control value of `0x0` for the nine inherited DMA display channels,
retain the raw bits, and do not dereference an invented instance pointer.
Other unknown values remain refused. Known initializer-style pointers and
configured push buffers remain reserved, as do VM-enabled BAR instance,
directory and table storage and their VRAM mappings.

The allocator excludes the complete low firmware-visible BAR1 aperture,
which contains the GOP framebuffer, and the upper envelope of inherited
references. It also excludes the final MiB for VBIOS storage and the first
256 KiB for VGA storage (contained in the low-aperture reservation).
These latter reservations are the checked MIT nouveau
`nvkm/subdev/fb/ramgf100.c` values `rsvd_head` and `rsvd_tail` at Linux v6.8
`e8f897f4afef0031fe618a8e94127a0934896aba`, Copyright 2013 Red Hat Inc.
Mixed-memory topologies are refused. PR 4.2 does not change VBIOS memory
training or execute memory scripts. Full VRAM allocation/accounting remains
milestone 5.

The six objects have read-only guard pages on each side. Both complementary
address-derived patterns fill the complete object set before reverse-order
full read-back, so aliasing cannot pass an immediate write/read echo.
Inherited instance pages, VGA storage and the VBIOS tail are snapshotted
before and after. The retained capture includes their byte counts and
hashes, both complete read-back hashes per object, raw controls and the
restored window selector. This is CPU physical-access evidence; it does not
claim engine translation, native scanout binding or a GPU-rendered image.

Source files above were read with their file-local or imported MIT grants.
No source implementation was adapted by this G4 fallback; the existing
nouveau adaptations retain their notices in `memory.rs` and are mapped in
`THIRD_PARTY_NOTICES`.
