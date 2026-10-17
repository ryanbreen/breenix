# GPU core provenance

This record names the source of every value the clean-room GPU core in `kernel/src/gpu/` uses: each
number, enumeration, structure layout, field offset and shader-grammar fact. The rules it follows are
`docs/gui/gpu.md` section 1.9. A value with no row here is not landed; the lane that adds a value adds its
row in the same PR. The core never uses `gpu_pci.rs`, `virgl.rs` or `svga3.rs`, and no legacy file, captured
stream or older program is a source for any row.

## Sources

Each source is pinned. Line ranges are inclusive and refer to the pinned revision; a lane that reads a
different revision records it here first.

| Key | Source | Pin | What it may supply |
|---|---|---|---|
| V | [VirtIO 1.3 CSD01](https://docs.oasis-open.org/virtio/virtio/v1.3/csd01/virtio-v1.3-csd01.html) | the CSD01 publication | every virtio-gpu command, structure, flag and enumeration, and the transport (status, features, virtqueues, reset), cited by section |
| U | Linux uapi `include/uapi/linux/virtio_gpu.h`, BSD-licensed | a tagged Linux release, recorded with the first row that cites it | only the field names, order and widths of four request layouts V names without defining: `CTX_ATTACH_RESOURCE`, `RESOURCE_CREATE_3D`, the 3D transfer layout of `TRANSFER_TO_HOST_3D` and `TRANSFER_FROM_HOST_3D`, and `SUBMIT_3D` |
| P | `src/virgl_protocol.h` in virglrenderer | UTM's fork at `451505ad0aa449290f2c0672da6877d57ee770e5` (`docs/gui/gpu.md` section 1.6.1) | VirGL command numbers, object types and field layouts |
| H | `src/virgl_hw.h` in virglrenderer | as P | the capability-set layout and VirGL format numbers |
| D | `src/gallium/include/pipe/p_defines.h` in virglrenderer | as P | enumeration values the VirGL wire carries (targets, shader stages, primitives, masks) |
| T | `src/gallium/include/pipe/p_shader_tokens.h` in virglrenderer | as P | TGSI token layouts, register files and semantic enumerations |
| L | [TGSI documentation](https://docs.mesa3d.org/gallium/tgsi.html) | the page as read, with the date | instruction, register, declaration and semantic definitions, from which programs are written |
| B | black-box experiments on the device or the unmodified host renderer | the experiment's date, renderer pin and inputs | shader-text grammar facts, and confirmation of each U layout |
| O | Breenix's own design: `docs/gui/gpu.md`, `docs/gui/gpu-abi.md`, Breenix's syscall ABI and memory API | the contract section | policies, limits, sizes, bookkeeping and the syscall 416 ABI |
| A | other published specifications: PCI Local Bus, the Arm Architecture Reference Manual, IEEE 754, Khronos GLSL | the document and section | configuration-header offsets, cache maintenance, float encodings, shading-language levels |

P, H, D and T supply numeric values, enumerations and structure layouts only, never code, control flow or
policy. U is treated the same way and supplies nothing outside its four layouts. L supplies the
meaning of instructions; it is not a source for the text form virglrenderer accepts, which only B
supplies.

## How each value is recorded

- **A virtio-gpu value or layout:** one row naming V's section. For one of U's four layouts, one row per
  field with U's line, and a B row confirming the whole layout on the device: the request the core sent,
  that the device accepted it, and the data it returned that showed the fields were read as intended.
- **A VirGL value or layout:** one row naming its header key (P, H, D or T) and the line range that
  defines it. An offset computed from a header's field sizes (a capability-set offset, for example) shows
  the sum and the lines of every field in it.
- **A shader program:** the program text, the L sections of each instruction, declaration and semantic it
  uses, and its token layout's T lines. A program identical to an older one in the tree is acceptable when
  this derivation produced it; the row says so.
- **A shader-grammar fact:** one B row per experiment: the renderer pin, the candidate text, the single
  change from the accepted baseline, the result (accepted or refused, with the value returned) and the fact
  it establishes. The label format is one of these facts. Acceptance shows grammar only, not pixel
  correctness.
- **A Breenix choice:** one row naming the contract section (O) that sets it, or stating that it is a
  sizing or bookkeeping choice of the core.
- **A value from another specification:** one row naming the document and section (A).

## VirtIO values and layouts

| Value or layout | Source |
|---|---|

## The four layouts from U

| Request and field | U line | Black-box confirmation (B) |
|---|---|---|

## VirGL values and layouts

| Value or layout | Header and lines |
|---|---|

## Shader programs

| Program | Instructions, declarations and semantics (L) | Token layout (T) |
|---|---|---|

## Shader-grammar facts

| Experiment (renderer pin, change from the baseline) | Result | Fact established |
|---|---|---|

## Breenix and platform values

| Value | Source |
|---|---|
