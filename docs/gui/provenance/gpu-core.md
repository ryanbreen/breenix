# GPU core provenance

This G1 record is **incomplete and blocks landing**. The checked VirtIO specification
omits payload layouts the core uses. A command's number is not a derivation of its fields.
No Linux uapi header is accepted as their source, and this record does not certify the
existing core as clean-room complete.

## Sources

[V] is [VirtIO 1.3 CSD01](https://docs.oasis-open.org/virtio/virtio/v1.3/csd01/virtio-v1.3-csd01.html).
All interface headers below are pinned at virglrenderer revision
`451505ad0aa449290f2c0672da6877d57ee770e5`; line ranges are inclusive:

- [P: virgl_protocol.h](https://github.com/utmapp/virglrenderer/blob/451505ad0aa449290f2c0672da6877d57ee770e5/src/virgl_protocol.h).
- [H: virgl_hw.h](https://github.com/utmapp/virglrenderer/blob/451505ad0aa449290f2c0672da6877d57ee770e5/src/virgl_hw.h).
- [D: p_defines.h](https://github.com/utmapp/virglrenderer/blob/451505ad0aa449290f2c0672da6877d57ee770e5/src/gallium/include/pipe/p_defines.h).
- [T: p_shader_tokens.h](https://github.com/utmapp/virglrenderer/blob/451505ad0aa449290f2c0672da6877d57ee770e5/src/gallium/include/pipe/p_shader_tokens.h).
- [L: TGSI language documentation](https://docs.mesa3d.org/gallium/tgsi.html), only language,
  register, declaration, semantic and instruction definitions.

Only interface facts were consulted, never surrounding implementation. No captured
stream or legacy shader is an authority. No legacy `gpu_pci.rs`, `virgl.rs` or `svga3.rs`
is a source for this core.

## VirtIO values and layouts

| Core values or layout | Source |
|---|---|
| PCI ID 0x1050 (0x1040 + type 16), VERSION_1 bit 32 | [V] §§4.1.2, 5.7.1, 6 |
| VIRGL bit 0, control queue 0 | [V] §§5.7.2–5.7.3 |
| Configuration scanouts at byte 8, capsets at 12 | [V] §5.7.4, consecutive le32 fields |
| Command types 0x0100, 0x0102, 0x0106, 0x0108, 0x0109, 0x0200, 0x0201, 0x0202, 0x0204, 0x0206, 0x0207 | [V] §5.7.6.7 enumeration |
| OK_NODATA=0x1100, DISPLAY_INFO=0x1101, CAPSET_INFO=0x1102, CAPSET=0x1103, first error=0x1200 | [V] §5.7.6.7 enumeration |
| Control header size 24, type/flags/fence/context at 0/4/8/16, fence flag bit 0 | [V] §5.7.6.7; completion semantics §5.7.6.5 |
| Display: 16 entries of 24 bytes; x/y/width/height/enabled/flags | [V] §5.7.6.8 GET_DISPLAY_INFO |
| Capset IDs 1, 2; info request 8 bytes, response 16 bytes; set request 8 bytes after header | [V] §5.7.6.8 GET_CAPSET_INFO, GET_CAPSET |
| Unref: resource/padding; attach backing: resource/count and 16-byte address/length/padding entries | [V] §5.7.6.8 RESOURCE_UNREF, RESOURCE_ATTACH_BACKING |
| CTX_CREATE: length/init/name at 0/4/8 after header, 64-byte name | [V] §5.7.6.9 CTX_CREATE |
| CTX_DESTROY: no extra payload | §5.7.6.9 names it but does not explicitly define its payload |
| CTX_ATTACH_RESOURCE: resource/padding, 8 bytes after header | **Unestablished**: §5.7.6.9 omits the layout |
| RESOURCE_CREATE_3D: 48 bytes after header, resource/target/format/bind/width/height/depth/array/level/samples/flags/padding | **Unestablished**: §5.7.6.9 omits the layout |
| TRANSFER_FROM_HOST_3D: 48 bytes after header, six-word box, u64 offset, resource/level/stride/layer stride | **Unestablished**: §5.7.6.9 omits the layout |
| SUBMIT_3D: size/padding before opaque stream | **Unestablished**: §5.7.6.9 omits the layout |
| Descriptor size 16, address/length/flags/next at 0/8/12/14, NEXT=1, WRITE=2 | [V] §2.7.5 |
| Available flags/index/entries at 0/2/4, u16 entries; used flags/index/entries at 0/2/4, 8-byte entries; u16 wrapping indices | [V] §§2.7.6, 2.7.8, 2.7.13–2.7.14 |
| Status, feature negotiation, queue publication, ISR acknowledgement, reset | [V] §§2.1–2.4, 3.1, 4.1.4–4.1.5 |

The current [OASIS GPU specification source, lines 752–766](https://github.com/oasis-tcs/virtio-spec/blob/f9abfd55cb663837dda1153b826216dcf4d25b84/device-types/gpu/description.tex#L752-L766)
also only names/describes the four 3D commands. Immediately before them it defines
CTX_CREATE's fields. Citing §5.7.6.9 for the missing field offsets would be false
attribution. A coordinator ruling must identify an allowed complete interface source
or authorize recorded black-box layout derivation before this requirement can close.

## VirGL values and layouts

| Core values or layout | Exact header lines |
|---|---|
| BGRA8=1, RG32_FLOAT=29, R8=64, RGBA8=67 | H:36, 64, 105, 108 |
| Bind render-target bit 1, sampler-view bit 3, vertex-buffer bit 4 | H:635–638 |
| BUFFER=0, TEXTURE_2D=2 | D:156–168 |
| Vertex=0, fragment=1, triangles=4 | D:445–453, 459–475 |
| RGBA mask=15; clear colour 0 bit 2 | D:104–108, 217 |
| Object blend=1, rasterizer=2, DSA=3, shader=4, vertex elements=5, surface=8 | P:61–75 |
| Commands create=1, bind=2, viewport=4, framebuffer=5, vertex buffers=6, clear=7, draw=8, inline write=9, constant buffer=12, bind shader=31 | P:79–111 |
| Header command bits 0–7, object 8–15, length 16–31; representable length mask 0xFFFF | P:157–158; core chunk sizes stay below it |
| Blend 11 words, per-target mask shifted 27 | P:170–194 |
| DSA 5 words, depth/alpha/stencil disabled by zero fields | P:197–214 |
| Rasterizer 9 words, half-pixel-centre bit 29 | P:217–260 |
| Shader handle/stage/text-length/token-count/stream-output count, text follows | P:273–291 |
| Viewport first slot, three scales, three translations | P:294–301 |
| Framebuffer count/depth surface/colour handles | P:304–307 |
| Vertex elements handle plus four words per element; vertex buffer stride/offset/resource | P:310–321 |
| Constant buffer stage/index/data | P:330–332 |
| CLEAR buffers/RGBA/double depth/stencil | P:263–270 |
| DRAW_VBO 12-word fields | P:342–356 |
| Surface handle/resource/format/level/layers | P:369–376 |
| Inline write resource/level/usage/stride/layer stride/box/data | P:520–532 |
| Bind shader handle/stage | P:550–552 |

Zero blending, dithering, depth/stencil, culling, multisampling, smoothing and offsets;
all RGBA channels; one instance; no indexing, restart or stream output are Breenix
§1.5 profile choices, not host policies.

Capset offsets are sums of H's four-byte fields and arrays, not driver constants:

| Field | Byte offset/size | Header lines |
|---|---|---|
| sampler mask | 4/64 | H:708–715 (16 u32 mask words) |
| render mask | 68/64 | H:714–716 |
| GLSL level | 264/4 | H:671–719 (four masks and one boolean word) |
| v1 extent | 308 | H:712–730 |
| capability_bits | 392/4 | H:756–779 |
| max_texture_2d_size | 484/4 | H:779–791 |
| capability_bits_v2 | 688/4 | H:791–802 |
| renderer | 696/64 | H:802–804 |

Format f uses mask word f/32, bit f%32. GLSL level 130 is checked against Khronos
GLSL 1.30 §§4.1.3 and 8.7 for integer types/operations and texel fetch. Region copy
is a base command in P's enumeration, not a guessed feature bit.

## Shader derivation and black-box grammar

The vertex program needs MOV from IN[0] to POSITION OUT[0], then END. The fragment
program needs MOV from CONST[0], supplied with the draw colour, to COLOR OUT[0], then
END. L defines these operations, declarations and semantics. No older program is a template.

Each program needs 11 TGSI binary tokens: header + processor (2); ordinary declaration
and range (2); semantic declaration, range and semantic (3); MOV, destination, source
(3); END (1). Layout sources: T:37–56, 137–158, 226–234, 649–683, 752–770,
796–807. Register files: T:77–98; semantic enumerations: T:174–224. Operand counts
come from L's MOV/END descriptions, not an implementation metadata table.

On 2026-10-17 UTC, the unmodified installed renderer at the header revision above was
called through its public init, context-create and submit APIs. The host supplied an
ANGLE GLES context through EGL callbacks; initialization returned 0. Each candidate
had a fresh context and one CREATE_OBJECT SHADER: handle 1, appropriate stage,
complete text length including NUL, 11 tokens, no stream output. Text was packed into
little-endian words with zero padding. Return 0 means accepted; 22 means refused.
This establishes grammar acceptance, not pixel correctness or host attestation.

Vertex candidate:

```text
VERT
DCL IN[0]
DCL OUT[0], POSITION
0: MOV OUT[0], IN[0]
1: END
```

Fragment candidate:

```text
FRAG
DCL OUT[0], COLOR
DCL CONST[0]
0: MOV OUT[0], CONST[0]
1: END
```

| Experiment (one change from vertex unless stated) | Return | Fact established |
|---|---|---|
| Vertex candidate, final newline and NUL | 0 | Stage, declarations, bracketed registers, semantics, MOV/END and decimal-colon labels accepted together |
| Fragment candidate | 0 | FRAG, COLOR, CONST[0] accepted together |
| Remove both instruction labels | 0 | These programs allow unlabelled instructions |
| Replace `0:` with `entry:` | 22 | This word-label form is refused |
| Replace VERT with VERTEX | 22 | This alternative stage spelling is refused |
| Remove MOV's operand comma | 22 | This MOV form requires the comma |
| Prefix `DCL IN[0]` with `0: ` | 22 | A declaration cannot use that instruction-label form |
| Replace IN[0] with IN0 | 22 | This unbracketed register spelling is refused |
| Remove `, POSITION` | 0 | Acceptance does not enforce the intended semantic; retain POSITION from L |
| Remove final newline, retain NUL | 0 | A final newline is optional |

The selected follow-up candidates use the accepted unlabelled form. The committed
core still has its older indented literals; their replacement is paused at the VirtIO
source blocker. This record does not claim that implementation change landed.

## Breenix and platform values

- Syscall operations, arguments, enumerations, records and reserved fields are Breenix's
  `docs/gui/gpu-abi.md`, implementing gpu.md §§1.3–1.5 and 1.8. Errno values are Breenix's
  existing syscall ABI. Work limits, four in-flight submissions, watchdog and timing
  tolerances are gpu.md §1.4.1; no host source determines these policies.
- 192 MiB backing quota, 256 handles, 128 MiB maximum buffer, 1 MiB readback bands,
  256-byte row alignment, 16-frame command chunks, 1,024 rectangles per inline write
  and 256 queue descriptors are Breenix G1 sizing choices. Copy quota charging is G6.
- Pages are 4,096 bytes through Breenix's ARM64 memory API. Cache lines are 4 shifted
  by CTR_EL0.DminLine bits 19:16; cache maintenance uses the Arm Architecture Reference
  Manual's CTR_EL0, DC CVAC/CIVAC and DSB definitions. Address translation is Breenix's API.
- PCI command offset 4, INTx-disable bit 10 and interrupt-pin offset 0x3D are PCI Local
  Bus Specification configuration-header facts. The QEMU virt INTx mapping, SPI
  3 + ((slot + pin - 1) mod 4), with GIC SPI base 32, is read from the machine's
  exposed device-tree interrupt-map, a black-box platform interface.
- IEEE 754 supplies 1.0=0x3F800000, 0.5=0x3F000000, sign bit 31 and conversion rules;
  8-bit UNORM supplies 255. Viewport half-size 8,192 (shift 13) spans the contract's
  16,384 domain. Six two-float vertices (48 bytes) cover a rectangle with two triangles:
  Breenix geometry design. Nonoverlapping vertex regions avoid write/read hazards.
- Poison 0x3C5AA5C3 and its complement implement §1.8. Key mixer 0x9E3779B97F4A7C15,
  first resource ID 0x100 and initial ID/counter 1 are Breenix bookkeeping choices.
  Undefined command 0x02FF is a Breenix fault outside V's defined enumeration.
- No Y flip is a Breenix viewport policy checked by suite pixels; it is not inferred
  from renderer implementation. Resource holds, handle namespaces, publication and
  per-context fencing are Breenix's own design under §1.4.1.
