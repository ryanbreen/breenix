# GK208B native preferred mode

E19 milestone 4 PR 4.3 targets the first detailed timing returned by the live
DDC sink, under coordinator ruling G3. The hardware path validates that timing
and the selected board VPLL/IEDT records before quiescing inherited channels.
It uses the PR 4.2 private allocation; inherited VRAM storage is not written.

The native core channel consumes a Breenix-owned EVO stream and private DMA
objects. A polled supervisor executes predecoded board display operations,
programs the board VPLL, selects the TMDS clock and powers the selected SOR.
Unknown opcodes, malformed operands, excessive recursion or a register outside
display MMIO refuse takeover before any script executes.

The scanout uses pitch-linear A8R8G8B8 storage. A bounded BAR1 view maps it
through private instance/table storage; the page size follows the retained
hardware configuration. This supplies CPU access for the console, without
introducing a general-purpose GPU virtual address allocator.

`GT710_MODE` records the preferred timing, active head and SOR registers,
notifier transitions, GET/PUT, raster samples, underflow/error state and the
private object addresses. The skipped-update probe submits a different shadow
address without UPDATE and requires the old active address and an incomplete
notifier; the following clean update must complete. A-B-A flips and a second
advertised mode remain PRs 4.4 and 4.5.

`GT710_PIX` rows retain the full BAR1 read-back, pitch included.
The collector reconstructs `gt710-native-scanout.bin` in the private boot store,
checks each pixel against the independent nonce pattern and hashes the complete
binary. CPU-filled patterns are display bring-up evidence, not GPU rendering.
A successful console attachment switches its geometry and backing before the
render thread starts. Its settle operation checks the active native binding
and display health; firmware-write accounting continues across that attachment.

## Permissive source audit

The file-local grants were read before adaptation. Nouveau sources are pinned
to Linux v6.8 `e8f897f4afef0031fe618a8e94127a0934896aba`:
`nvkm/engine/disp/{gf119,nv50,gk104,gk110}.c`,
`nvkm/engine/dma/usergf119.c`, `nvkm/core/ramht.c`,
`nvkm/subdev/bios/{pll,disp,init,dcb}.c`,
`include/nvkm/subdev/bios/{pll,disp}.h`, `nvkm/subdev/clk/pllgt215.c`,
`nvkm/subdev/devinit/gf100.c`, `nvkm/subdev/bar/gf100.c`,
`nvkm/subdev/fb/gf100.c`, and `nvkm/subdev/mmu/{vmmgf100,vmmgk104}.c`.
Each declares MIT; Linux integration code is excluded.

NVIDIA open-gpu-doc `classes/display/cl927d.h` declares MIT at revision
`9fdf5c4062007929d9f4e6cbad9c9771fe61b880`.
Envytools `rnndb/display/{nv_evo,g80_pdisplay}.xml` and `rnndb/bus/pbus.xml`
import the MIT grant in `rnndb/copyright.xml`, checked at revision
`f102b82381f3f11cee113d16374c87091db039d9`.
The adapted files retain original notices; `THIRD_PARTY_NOTICES` maps the sources
to `modeset.rs` and `native.rs`. The repository's remaining subsystems retain
their existing provenance.
