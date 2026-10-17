# GPU core provenance

This record names the source of every value the clean-room GPU core in `kernel/src/gpu/` uses: each
number, enumeration, structure layout, field offset and shader-grammar fact. The rules it follows are
`docs/gui/gpu.md` section 1.9. A value with no row here is not landed; the lane that adds a value adds its
row in the same PR. The core never uses `gpu_pci.rs`, `virgl.rs` or `svga3.rs`, and no legacy file, captured
stream or older program is a source for any row.

The core is `kernel/src/gpu/` and the `gpu-abi` crate (`libs/gpu-abi`), whose layouts the kernel and
libbreenix share. The VirtIO PCI transport it drives (`kernel/src/drivers/virtio/pci_transport.rs`) is
Breenix's existing driver for every modern VirtIO PCI device and is outside this record; the core adds one
accessor to it, the ISR byte's address (V §4.1.4.5).

## Sources

Each source is pinned. Line ranges are inclusive and refer to the pinned revision; a lane that reads a
different revision records it here first.

| Key | Source | Pin | What it may supply |
|---|---|---|---|
| V | [VirtIO 1.3 CSD01](https://docs.oasis-open.org/virtio/virtio/v1.3/csd01/virtio-v1.3-csd01.html) | the CSD01 publication | every virtio-gpu command, structure, flag and enumeration, and the transport (status, features, virtqueues, reset), cited by section |
| U | Linux uapi `include/uapi/linux/virtio_gpu.h`, BSD-licensed | Linux tag `v6.12` | only the field names, order and widths of four request layouts V names without defining: `CTX_ATTACH_RESOURCE`, `RESOURCE_CREATE_3D`, the 3D transfer layout of `TRANSFER_TO_HOST_3D` and `TRANSFER_FROM_HOST_3D`, and `SUBMIT_3D` |
| P | `src/virgl_protocol.h` in virglrenderer | UTM's fork at `451505ad0aa449290f2c0672da6877d57ee770e5` (`docs/gui/gpu.md` section 1.6.1) | VirGL command numbers, object types and field layouts |
| H | `src/virgl_hw.h` in virglrenderer | as P | the capability-set layout and VirGL format numbers |
| D | `src/gallium/include/pipe/p_defines.h` in virglrenderer | as P | enumeration values the VirGL wire carries (targets, shader stages, primitives, masks) |
| T | `src/gallium/include/pipe/p_shader_tokens.h` in virglrenderer | as P | TGSI token layouts, register files and semantic enumerations |
| L | [TGSI documentation](https://docs.mesa3d.org/gallium/tgsi.html) | Mesa revision `7e5b708afd421c1944aeca74d4be6b7996674378`, `docs/gallium/tgsi.rst` ([immutable text](https://chromium.googlesource.com/external/gitlab.freedesktop.org/mesa/mesa/+/7e5b708afd421c1944aeca74d4be6b7996674378/docs/gallium/tgsi.rst)) | instruction, register, declaration and semantic definitions, from which programs are written |
| B | black-box experiments on the device or the unmodified host renderer | the experiment's commit, run id or harness invocation, renderer pin, inputs and output excerpt below | shader-text grammar facts, and confirmation of each U layout |
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
| PCI vendor 0x1AF4; device id 0x1050, 0x1040 plus device type 16 | V §4.1.2 (0x1040 plus the device id), §5.7.1 (16) |
| Device status: reset by writing 0, complete when it reads back 0; ACKNOWLEDGE 1, DRIVER 2, DRIVER_OK 4, FEATURES_OK 8, DEVICE_NEEDS_RESET 64 | V §2.1, §2.4.1, §2.4.2 |
| Initialisation order: reset, ACKNOWLEDGE, DRIVER, features, FEATURES_OK and its re-read, queue setup, DRIVER_OK | V §3.1.1 |
| Feature VIRTIO_GPU_F_VIRGL bit 0; VIRTIO_F_VERSION_1 bit 32 | V §5.7.3; V §6 |
| Control queue index 0 | V §5.7.2 |
| Device configuration: num_scanouts at byte 8, num_capsets at 12 (four le32 fields) | V §5.7.4 |
| ISR status byte: reading it acknowledges and deasserts INTx; bit 0 queue interrupt | V §4.1.4.5 |
| Queue size: a power of two no larger than the device's maximum (the core takes at most 256) | V §2.7; O, sizing |
| Descriptor: 16 bytes, addr le64 at 0, len le32 at 8, flags le16 at 12, next le16 at 14; NEXT 1, WRITE 2 | V §2.7.5 |
| Available ring: flags le16 at 0, idx le16 at 2, ring le16 entries from 4; idx written after the entries | V §2.7.6, §2.7.13.3 |
| Used ring: flags le16 at 0, idx le16 at 2, 8-byte elements (id le32, len le32) from 4; the driver reads the index before the element | V §2.7.8, §2.7.14 |
| Indices wrap at 2^16; slot = index modulo queue size | V §2.7.6, §2.7.8 |
| A returned chain's descriptors are free again; a used entry names the head of a chain the driver made available, and len is the bytes written into its device-writable buffers | V §2.7.8, §2.7.13, §2.7.14 |
| A device fault: DEVICE_NEEDS_RESET set, a used entry naming no chain the driver made available, or a used length past the request's writable buffer | V §2.1, §2.7.8; the policy is O §1.4.1 |
| Control header: 24 bytes; type le32 at 0, flags le32 at 4, fence_id le64 at 8, ctx_id le32 at 16, ring_idx u8 at 20 (0), padding | V §5.7.6.7 |
| VIRTIO_GPU_FLAG_FENCE bit 0; a fenced request's response copies fence_id and comes only after processing completes | V §5.7.6.5, §5.7.6.7 |
| Commands GET_DISPLAY_INFO 0x0100, RESOURCE_UNREF 0x0102, RESOURCE_ATTACH_BACKING 0x0106, GET_CAPSET_INFO 0x0108, GET_CAPSET 0x0109, CTX_CREATE 0x0200, CTX_DESTROY 0x0201, CTX_ATTACH_RESOURCE 0x0202, RESOURCE_CREATE_3D 0x0204, TRANSFER_FROM_HOST_3D 0x0206, SUBMIT_3D 0x0207 | V §5.7.6.7, `enum virtio_gpu_ctrl_type` |
| Responses OK_NODATA 0x1100, OK_DISPLAY_INFO 0x1101, OK_CAPSET_INFO 0x1102, OK_CAPSET 0x1103; errors from ERR_UNSPEC 0x1200 | V §5.7.6.7 |
| 0x02FF, test point 27's command: inside the 3D command range and assigned to no command | V §5.7.6.7 (the range ends at 0x0209); the use is O §1.8 |
| GET_DISPLAY_INFO: header-only request; response of 16 `virtio_gpu_display_one` of 24 bytes (rect x, y, width, height, enabled, flags) after the header | V §5.7.6.8 |
| GET_CAPSET_INFO: capset_index, padding; response capset_id, capset_max_version, capset_max_size, padding | V §5.7.6.8 |
| GET_CAPSET: capset_id, capset_version; response capset_data after the header | V §5.7.6.8 |
| Capability set ids VIRGL 1 and VIRGL2 2 | V §5.7.6.8 |
| RESOURCE_UNREF: resource_id, padding | V §5.7.6.8 |
| RESOURCE_ATTACH_BACKING: resource_id, nr_entries, then `virtio_gpu_mem_entry` entries of 16 bytes (addr le64, length le32, padding) | V §5.7.6.8 |
| CTX_CREATE: nlen, context_init (0: the device chooses the context type), debug_name[64]; the context is the header's ctx_id | V §5.7.6.9 |
| CTX_DESTROY: the header alone, naming the context by ctx_id; §5.7.6.9 gives it no request data and the core sends none | V §5.7.6.7, §5.7.6.9 |

## The four layouts from U

Each layout follows the 24-byte control header. Lines are `virtio_gpu.h` at Linux `v6.12`.

| Request and field | U line | Black-box confirmation (B) |
|---|---|---|
| `CTX_ATTACH_RESOURCE` (`virtio_gpu_ctx_resource`): resource_id le32 at 0 | 299 | B1 |
| `CTX_ATTACH_RESOURCE`: padding le32 at 4 (0) | 300 | B1 |
| `RESOURCE_CREATE_3D` (`virtio_gpu_resource_create_3d`): resource_id at 0 | 268 | B2 |
| `RESOURCE_CREATE_3D`: target at 4 (D's texture target) | 269 | B2 |
| `RESOURCE_CREATE_3D`: format at 8 (H's VirGL format) | 270 | B2 |
| `RESOURCE_CREATE_3D`: bind at 12 (H's bind flags) | 271 | B2 |
| `RESOURCE_CREATE_3D`: width at 16, height at 20 | 272–273 | B2 |
| `RESOURCE_CREATE_3D`: depth at 24 (1), array_size at 28 (1) | 274–275 | B2 |
| `RESOURCE_CREATE_3D`: last_level at 32 (0), nr_samples at 36 (0), flags at 40 (0), padding at 44 | 276–279 | B2 |
| 3D transfer (`virtio_gpu_transfer_host_3d`): box at 0, six le32 x, y, z, w, h, d (`virtio_gpu_box`) | 249–250, 256 | B3 |
| 3D transfer: offset le64 at 24 | 257 | B3 |
| 3D transfer: resource_id at 32, level at 36 (0), stride at 40, layer_stride at 44 (0) | 258–261 | B3 |
| `SUBMIT_3D` (`virtio_gpu_cmd_submit`): size le32 at 0 (bytes of the stream that follows), padding at 4 | 306–307 | B4 |

B1 to B4 are two runs of one experiment on 2026-10-17, made at G1 core initialisation on QEMU's
`virtio-gpu-gl-pci` (the GL QEMU of `docs/gui/gpu.md` section 1.6.1, virglrenderer at P's pin) with polled
requests in contexts of their own (ids 0xF000 to 0xF003), before any process ran, by code that is not part of
the core. Every resource was 4×3 with one page of backing at a 256-byte row pitch. Each readback poisoned the
page with 0xA5, transferred one box to `offset` = y × 256 + x × 4 and showed bytes `offset − 8` to
`offset + 15`. Every request was answered OK_NODATA (0x1100) and every fenced one echoed its fence. Clears are
given as their (B, G, R, A) bytes.

The device does not report a command it cannot apply in a context: in run 1 a SUBMIT_3D whose surface named a
resource not attached to the context, and in run 2 a transfer box past the resource's width, were each answered
OK_NODATA, and after either one every later command in that context, transfers included, did nothing. Only
results from before such a command are used below. The core never sends one: the validator refuses foreign
handles, every resource is attached to every context of its process, and READBACK checks its rectangle against
the target.

| Experiment | Request and result | Fact established |
|---|---|---|
| B1 | Run 2. Resource 0xF004 (B8G8R8A8) attached to context 0xF003 only, by CTX_ATTACH_RESOURCE with resource_id 0xF004 and padding 0. Cleared to (11, 22, 33, 44) in 0xF003, the box (1, 1, 2, 1) read back `11 22 33 44 11 22 33 44` at 260 with poison either side. The same transfer in context 0xF002, created fresh and never given the resource, left the page all poison. | resource_id at 0 names the resource the request attaches; the request is accepted with padding 0 at 4 |
| B2 | Run 1. Resource 0xF001: RESOURCE_CREATE_3D with target 2, format 1 (B8G8R8A8), bind RENDER_TARGET and SAMPLER_VIEW, width 4, height 3, depth 1, array_size 1, last_level 0, nr_samples 0, flags 0. Cleared to (11, 22, 33, 44), the box (1, 1, 2, 1) read back `11 22 33 44` per pixel. Resource 0xF002, identical but format 67 (R8G8B8A8), read back `33 22 11 44` per pixel. Run 2: the box (3, 2, 1, 1), the last pixel, read back the cleared colour at 524, and the box (3, 0, 2, 1), one pixel past a width of 4, wrote nothing. | target, format and bind are read at 4, 8 and 12: the resource is a 2D render target whose bytes follow the format field's channel order; width at 16 is read as 4; height at 20 holds at least the 3 rows read back; the zero fields give one level and one sample |
| B3 | Run 1: the box (1, 1, 2, 1) at offset 260 (le64), stride 256, level 0, layer_stride 0, z 0, d 1 wrote exactly bytes 260 to 267 of the page and left 252 to 259 and 268 to 275 poisoned. Run 2: the box (3, 2, 1, 1) at offset 524 wrote exactly bytes 524 to 527. Multi-row boxes with the target's pitch as stride are every READBACK of the G1 suite boot, whose `device` and `submit` cases read back the canvases they expect. | the box's x, y, w and h at 0 to 15, the u64 offset at 24, resource_id at 32 and stride at 40 are read as laid out; level, layer_stride, z and d with the values 0, 0, 0 and 1 select the single 2D level |
| B4 | Run 2. A stream of 28 words (a surface, the framebuffer, a green clear, a blue clear) sent as SUBMIT_3D with size 76, which ends after the green clear, left resource 0xF001 green (`00 ff 00 ff`); the same stream with size 112 left it blue (`ff 00 00 ff`). | size at 0 is the stream's length in bytes, and the device executes no command past it; padding at 4 is 0 |

## VirGL values and layouts

| Value or layout | Header and lines |
|---|---|
| Object types BLEND 1, RASTERIZER 2, DSA 3, SHADER 4, VERTEX_ELEMENTS 5, SURFACE 8 (NULL is 0 at 62) | P:61–70 |
| Context commands CREATE_OBJECT 1, BIND_OBJECT 2, DESTROY_OBJECT 3, SET_VIEWPORT_STATE 4, SET_FRAMEBUFFER_STATE 5, SET_VERTEX_BUFFERS 6, CLEAR 7, DRAW_VBO 8, RESOURCE_INLINE_WRITE 9 | P:78–88 |
| SET_CONSTANT_BUFFER 12 | P:78–91 |
| BIND_SHADER 31 | P:78–111 |
| Command header: command bits 0–7, object type 8–15, payload length in words 16–31; the longest payload P allows, 65,532 words, is above every length the core encodes (a chunk is at most 16,384 words) | P:157–158 |
| DESTROY_OBJECT payload: the handle; the header names the object type | P:170 |
| Blend: 11 words (VIRGL_MAX_COLOR_BUFS 8 plus 3): handle, S0, S1, one S2 per colour buffer; S0 dither bit 2 and every enable 0; S2 colour mask at bits 27–30, blending off | P:161, 174–194 |
| DSA: 5 words: handle, S0 (depth and alpha tests off), S1 and S2 (stencil off), alpha reference | P:197–214 |
| Rasterizer: 9 words: handle, S0, point size, sprite coordinate enable, S3, line width, offset units, scale and clamp; half-pixel centre S0 bit 29, every other S0 bit 0 (fill, cull none, no scissor, multisample or smoothing) | P:217–260 |
| CLEAR: 8 words: buffers, four colour words, depth as two words of a double, stencil | P:262–270 |
| Shader object: handle, type, offset word (the text's byte length in bits 0–30, continuation bit 31 clear), token count, stream-output count 0, then the text | P:273–282 |
| SET_VIEWPORT_STATE: start slot, then scale x, y, z and translate x, y, z of one viewport | P:294–301 |
| SET_FRAMEBUFFER_STATE: colour buffer count, depth surface handle (0, none), colour surface handles | P:304–307 |
| Vertex elements: handle, then per element source offset, instance divisor, vertex buffer index, source format | P:310–315 |
| SET_VERTEX_BUFFERS: stride, offset, resource per buffer | P:318–321 |
| SET_CONSTANT_BUFFER: shader type, index, then the data | P:330–332 |
| DRAW_VBO: 12 words: start, count, mode, indexed, instance count, index bias, start instance, primitive restart, restart index, minimum index, maximum index, count from stream output | P:342–356 |
| Surface object: handle, resource, format, texture level, texture layers | P:369–376 |
| RESOURCE_INLINE_WRITE: resource, level, usage, stride, layer stride, box x, y, z, w, h, d, then the data | P:521–532 |
| BIND_SHADER: handle, type | P:550–552 |
| Formats B8G8R8A8_UNORM 1, R32G32_FLOAT 29, R8_UNORM 64, R8G8B8A8_UNORM 67 | H:36, 64, 105, 108 |
| Bind flags RENDER_TARGET bit 1, SAMPLER_VIEW bit 3, VERTEX_BUFFER bit 4 | H:636–638 |
| Texture targets BUFFER 0, TEXTURE_2D 2 | D:157–161 |
| Shader stages VERTEX 0, FRAGMENT 1 | D:445–447 |
| Primitive TRIANGLES 4 | D:459–464 |
| Colour mask RGBA 0xF | D:104–108 |
| Clear buffer bit COLOR0, bit 2 | D:217 |

Capability-set offsets are sums of H's field sizes, not driver constants. A format mask is 16 le32 words
(H:708–710); the boolean set is one word of 32 one-bit fields (H:671–705).

| Field | Byte offset and size | Sum and header lines |
|---|---|---|
| max_version | 0, 4 | H:713 |
| sampler format mask | 4, 64 | 4 (H:713); H:714 |
| render format mask | 68, 64 | 4 + 64 (H:713–714); H:715 |
| glsl_level | 264, 4 | 4 + 4 × 64 (four masks, H:714–717) + 4 (H:718); H:719 |
| end of `virgl_caps_v1` | 308 | 268 + 10 × 4 (H:720–729) |
| capability_bits | 392, 4 | 308 + 9 floats (H:758–766) + 5 × 4 (H:767–771) + 4 × 4 (H:772–775) + 3 × 4 (H:776–778) = 392; H:779 |
| max_texture_2d_size | 484, 4 | 396 + 8 × 4 (H:780) + 8 × 4 (H:781–788) + 2 × 12 (H:789–790) = 484; H:791 |
| capability_bits_v2 | 688, 4 | 488 + 3 × 4 (H:792–794) + 2 × 24 (H:795–796) + 3 × 4 (H:797–799) + 2 × 64 (H:800–801) = 688; H:802 |
| renderer | 696, 64 | 692 + 4 (H:803); H:804 |

A format's support is bit `f % 32` of mask word `f / 32` (H:708–710, formats as above). Region copy needs no
capability bit: RESOURCE_COPY_REGION is a base context command (P:96), so any VirGL capability set offers it.
A capability set shorter than 308 bytes has no v1 fields (H:712–730). The GLSL level 130 the profile needs
is the first with integer types and operations and integer texel fetch (A: Khronos GLSL 1.30, sections 4.1.3
and 8.7).

## Shader programs

Each program is written from L's definitions; its text form is the one B accepted (next section).

| Program | Instructions, declarations and semantics (L) | Token layout (T) |
|---|---|---|
| `solid` vertex: `VERT`, `DCL IN[0]`, `DCL OUT[0], POSITION`, `MOV OUT[0], IN[0]`, `END` | MOV ("MOV (Move)", Core ISA: dst = src per component); Declaration (a register of a file, with a Declaration Semantic token when the semantic flag is set); TGSI_SEMANTIC_POSITION (a vertex shader output holding the clip-space position). END: the program's end, TGSI_OPCODE_END (T:474, "aka HALT"); L describes no END instruction, so its text form is B's | 11 tokens: header (T:37–41) and processor (T:52–56); input declaration (T:137–152) and range (T:154–158); output declaration, range and semantic (T:226–234); MOV instruction (T:649–661) with one destination (T:796–803) and one source (T:752–764) register, the operand counts L's MOV gives; END instruction (T:649–661). Register files INPUT 2, OUTPUT 3 (T:72–76); POSITION 0 (T:174–175); opcodes MOV and END (T:372–474) |
| `solid` fragment: `FRAG`, `DCL OUT[0], COLOR`, `DCL CONST[0]`, `MOV OUT[0], CONST[0]`, `END` | MOV, Declaration and Declaration Semantic as above; TGSI_SEMANTIC_COLOR (a fragment output holding an R, G, B, A colour); the constant file supplies constant 0, set by SET_CONSTANT_BUFFER (P:330–332) with the draw's colour | 11 tokens: header and processor; output declaration, range and semantic; constant declaration and range; MOV with one destination and one source; END. Register file CONSTANT 1 (T:72–74); COLOR 1 (T:174–176) |

The token count is the shader object's token-count field (P:281). The two programs are the minimal
pass-throughs and match the tree's older text up to its labels and indentation; this derivation, not the
older text, produced them, and the experiments below establish that the unlabelled, unindented form is
accepted.

## Experiment provenance and host-library use

Coordinator addendum 2 (2026-10-17), rulings 11–13, settles finding 8 of #1677:
host tools may call third-party libraries through their public APIs, and each black-box experiment
records its commit, Vigil run id or harness invocation, and the output that established the fact.
The device experiments below used temporary, uncommitted experiment code on top of the named commit;
the commit identifies their base, not a committed harness. Their request inputs are specified in B1–B4
above. These are individual device observations, not a passing suite or boot-path gate.

| Run | Base commit | Vigil run id | Use |
|---|---|---|---|
| Run 1 | `c28938bc0ef4b4e5ea8709c3fae13f833bd13905` | `20261017T023742Z-qemu` | B2's channel-order comparison and B3's offset/stride observation, before the unattached-resource submit |
| Run 2 | `c28938bc0ef4b4e5ea8709c3fae13f833bd13905` | `20261017T032249Z-qemu` | B1, B2's last-pixel observation, B3's last-pixel offset and B4's size comparison |

The following are excerpts from those runs' `[gpu-exp]` serial output. Byte spans retain their poison
bytes so the data written and its boundaries remain distinguishable.

| Experiment | Run and output excerpt |
|---|---|
| B1 | Run 2: `CTX_ATTACH_RESOURCE d to context 3 -> 0x1100 echoed=0`; `TRANSFER_FROM_HOST_3D d in context 3 (attached) -> 0x1100 echoed=11`; bytes at offset 260: `a5 a5 a5 a5 a5 a5 a5 a5 [11 22 33 44 11 22 33 44] a5 a5 a5 a5 a5 a5 a5 a5`. `TRANSFER_FROM_HOST_3D d in context 2 (never attached) -> 0x1100 echoed=12`; its entire 24-byte excerpt stayed `a5`. Brackets delimit the written bytes here; the serial printed a comma-separated array. |
| B2 | Run 1: `RESOURCE_CREATE_3D id=0xf001 format=1 4x3 -> 0x1100 echoed=0`; `RESOURCE_CREATE_3D id=0xf002 format=67 4x3 -> 0x1100 echoed=0`; the respective transfer excerpts at offset 260 contain `11, 22, 33, 44, 11, 22, 33, 44` and `33, 22, 11, 44, 33, 22, 11, 44`. Run 2: `TRANSFER_FROM_HOST_3D a box(3,2,1,1) last pixel -> 0x1100 echoed=5`; bytes at offset 524: eight `a5`, `ff, 00, 00, ff`, twelve `a5`. |
| B3 | Run 1: `TRANSFER_FROM_HOST_3D a box(1,1,2,1) -> 0x1100 echoed=2`; `bytes at offset 260-8..+16: [a5, a5, a5, a5, a5, a5, a5, a5, 11, 22, 33, 44, 11, 22, 33, 44, a5, a5, a5, a5, a5, a5, a5, a5]`. Run 2's offset-524 excerpt is the B2 last-pixel observation above. |
| B4 | Run 2: `SUBMIT_3D green+blue, size 76 of 112 -> 0x1100 echoed=1`; `TRANSFER_FROM_HOST_3D a after short submit -> 0x1100 echoed=2`; written bytes `00, ff, 00, ff, 00, ff, 00, ff`. `SUBMIT_3D green+blue, full size -> 0x1100 echoed=3`; `TRANSFER_FROM_HOST_3D a after full submit -> 0x1100 echoed=4`; written bytes `ff, 00, 00, ff, ff, 00, 00, ff`. Both offset-260 excerpts retain eight `a5` on each side. |

B3's additional multi-row confirmation and the committed shader programs' drawing confirmation are
from commit `41fea50111f7d9d3dc2e8e3794167dcf275a83c2`, Vigil `20261017T041050Z-qemu`:
`SUITE gpu DONE passed=34 failed=136 skipped=0 total=170`, with device 12/12 and submit 22/22.
The 136 failures are the explicitly unwritten later-milestone checks; this is not a green full suite.

The shader-grammar experiment ran at 2026-10-17 01:13:49 UTC with checkout commit
`ec18d1bbc66726d14328a0e3c26174a72befaa6d`, using the unmodified renderer at P's pin. Its host-only
Python ctypes harness, named `g1-egl.py`, loaded virglrenderer and ANGLE EGL, created a 1×1 pbuffer and
GLES contexts, and supplied the renderer's version-1 fence/create/destroy/make-current callbacks.
It used `src/virglrenderer.h` at P's pin for public function signatures and the callback/context-parameter
ABI: `virgl_renderer_init`, `virgl_renderer_context_create`, `virgl_renderer_submit_cmd`,
`virgl_renderer_context_destroy` and `virgl_renderer_cleanup`. This is ordinary host-library use under
ruling 11. It supplies no guest wire values, code, control flow or policy to Breenix; nothing from the
host API header or harness is copied into the kernel, libraries or programs. Guest packet fields and
shader semantics retain their P, D, T and L sources above.

The harness invocation, rendered without machine-specific directory prefixes, was
`DYLD_LIBRARY_PATH="$QEMU_GL_FRAMEWORKS" DYLD_FRAMEWORK_PATH="$QEMU_GL_FRAMEWORKS" python3 g1-egl.py`,
where `QEMU_GL_FRAMEWORKS` denotes the pinned QEMU GL bundle's Frameworks directory; the harness set
`ANGLE_DEFAULT_PLATFORM=metal`. The next section specifies each candidate and packing procedure.
Its output excerpt (renderer diagnostics between these lines omitted) was:

```text
EGL 1 5
renderer init 0
vertex decimal labels ACCEPT 0
fragment decimal labels ACCEPT 0
no labels ACCEPT 0
word label REFUSE 22
wrong stage REFUSE 22
missing comma REFUSE 22
numbered declarations REFUSE 22
register brackets absent REFUSE 22
missing output semantic ACCEPT 0
no trailing newline ACCEPT 0
```

The unattached-resource submit and out-of-bounds transfer observations above are device facts for the
next contract PR under ruling 13. Recording their provenance here does not amend the contract.

## Shader-grammar facts

The renderer was the unmodified virglrenderer at P's pin, called on 2026-10-17 UTC through its public
initialisation, context-creation and submit interfaces, with the host supplying an ANGLE GLES context.
Each candidate went to a fresh context as one CREATE_OBJECT SHADER (handle 1, the program's stage, the
text's byte length including its NUL, 11 tokens, no stream output), the text packed into little-endian
words with zero padding. A return of 0 is acceptance; 22 is refusal. Each row is one change from the
vertex baseline unless it says otherwise; the vertex baseline is `VERT`, `DCL IN[0]`,
`DCL OUT[0], POSITION`, `0: MOV OUT[0], IN[0]`, `1: END`, one per line.

| Experiment (renderer pin, change from the baseline) | Result | Fact established |
|---|---|---|
| The vertex baseline, a final newline and a NUL | accepted (0) | Stage word, declarations, bracketed registers, the POSITION semantic, MOV and END, and decimal-colon labels are accepted together |
| The fragment baseline: `FRAG`, `DCL OUT[0], COLOR`, `DCL CONST[0]`, `0: MOV OUT[0], CONST[0]`, `1: END` | accepted (0) | FRAG, COLOR and CONST[0] are accepted |
| Both instruction labels removed | accepted (0) | Instructions need no labels; the label format is decimal followed by a colon, and it is optional |
| `0:` replaced with `entry:` | refused (22) | A word label is refused |
| `VERT` replaced with `VERTEX` | refused (22) | The stage word is `VERT` |
| MOV's operand comma removed | refused (22) | Operands are separated by a comma |
| `DCL IN[0]` given the label `0: ` | refused (22) | A declaration cannot carry an instruction label |
| `IN[0]` written `IN0` | refused (22) | Register indices are bracketed |
| `, POSITION` removed | accepted (0) | Acceptance does not check the semantic, so the semantic comes from L, not from acceptance |
| The final newline removed, the NUL kept | accepted (0) | A final newline is optional |
| The committed programs above (unlabelled and unindented, both stages), in every context the G1 suite boot created | accepted: every warm-up and every `solid` draw of `device` and `submit` produced the pixels the suite expects | The unlabelled fragment program is accepted as the vertex one is, and both programs draw |

## Breenix and platform values

| Value | Source |
|---|---|
| Syscall 416's ops, argument layouts, enumerations, record offsets, reserved fields and status codes; the BGL stream's commands and error codes; test-control points 19 to 31 and their parameters, fence depth 2 among them | O: `docs/gui/gpu-abi.md`, implementing `docs/gui/gpu.md` sections 1.3, 1.4, 1.5 and 1.8 |
| Errno values | O: Breenix's syscall ABI (the Linux numbers it follows) |
| Work limits (65,536 draws, 1,048,576 instances, 64 MiB of named buffers, 2³¹ fragment-steps, 80 MiB stream), four submissions in flight, eight contexts per process, sides up to 16,384 | O: `docs/gui/gpu.md` section 1.4.1, `docs/gui/gpu-abi.md` |
| W = 2 s (the watchdog's hang and no-progress thresholds), K = 250 ms (the watchdog examines outstanding work at least this often; kills end waits within it), R = 100 ms (the reset budget) | O: `docs/gui/gpu.md` section 1.4.1, timing bounds |
| Failed fence answers remain recorded until the context is destroyed; their count never quarantines it | O: section 1.4.1, per-context device-error quarantine |
| 192 MiB of backing per process, 256 resource handles, 128 MiB largest buffer, 1 MiB readback bands, 16-frame command chunks, 1,024 rectangles per inline write, 256 queue descriptors | O: G1 sizing choices |
| Rows aligned to 256 bytes for transfers | O: `docs/gui/gpu.md` section 1.12 (the pitch probe) |
| Guest backing counted against RLIMIT_AS with the process's mappings | O: `docs/gui/gpu.md` section 1.4 (RESOURCE_CREATE) and Breenix's resource limits |
| Poison 0x3C5AA5C3 and its complement; `unwritten` at record offset 164 | O: `docs/gui/gpu.md` section 1.8 |
| Handle mixing constants 0x9E3779B97F4A7C15, 0xBF58476D1CE4E5B9, 0x94D049BB133111EB | A: [SplitMix64](https://prng.di.unimi.it/splitmix64.c), published increment and finalizer constants; Breenix applies that arithmetic to its handle key, process id and sequence |
| First resource id 0x100; first context, serial, fence, namespace and observation 1 | O: bookkeeping choices of the core |
| The context's debug name `breenix` | O: CTX_CREATE's free-form debug name (V §5.7.6.9) |
| Capability set preference: VIRGL2, then VIRGL; each listed id kept once | O: `docs/gui/gpu.md` section 1.12 |
| Viewport half-size 8,192 (shift 13): pixel p maps to clip coordinate (p − 8192) / 8192, exact in binary32 across the 16,384 domain; scale and translate z 0.5 | O: geometry of the core; A: IEEE 754 binary32 |
| A rectangle is two triangles, six two-float vertices (48 bytes); vertex stride 8; each draw writes its own vertex region | O: geometry of the core; the per-region rule is the black-box fact recorded in `docs/gui/gpu.md` section 1.12 |
| No vertical flip: target row 0 is transfer row 0 | O: viewport policy, checked by the suite's pixels |
| 1.0 = 0x3F800000, 0.5 = 0x3F000000, sign bit 31; 8-bit UNORM k as k / 255 | A: IEEE 754 binary32; A: the OpenGL UNORM conversion rule (`docs/gui/gpu.md` section 1.5) |
| PCI configuration header: command register at 0x04, its INTx-disable bit 10; interrupt pin at 0x3D | A: PCI Local Bus Specification, configuration header |
| QEMU `virt` INTx routing: slot s, pin p to SPI 3 + ((s + p − 1) mod 4), level-high; SPI n is GIC interrupt 32 + n | B: the machine's own device tree (`interrupt-map`), read from the running machine; A: Arm GIC architecture (SPI numbering) |
| Pages of 4,096 bytes; the device address of a kernel virtual address | O: Breenix's ARM64 memory API |
| Data cache line 4 << CTR_EL0.DminLine (bits 19:16); DC CVAC to clean, DC CIVAC to clean and invalidate, DSB SY after | A: Arm Architecture Reference Manual, CTR_EL0, DC CVAC, DC CIVAC, DSB |
| The virtual counter CNTVCT_EL0 seeds the handle key | A: Arm Architecture Reference Manual, CNTVCT_EL0 |

## G2 ABI choices

| Value or layout | Source |
|---|---|
| READBACK reason at 44 (0 none, 1 unwritten, 2 fence-mismatch, 3 resource-mismatch, 4 target-invalid, 5 device-error); invalidating submission at 56 | O: `docs/gui/gpu-abi.md`, naming the refusals required by gpu.md sections 1.4.1 and 1.8; zero inputs, no change to record v2 |
| PRESENT argument: 32 bytes, target at 0, completed flush fence at 8, answer at 16, zero padding at 20 and 24 | O: `docs/gui/gpu-abi.md`, implementing the minimal G2 bind of gpu.md sections 1.7 and 1.11 |

## G2 scanout and readback implementation values

| Value or layout | Source |
|---|---|
| SET_SCANOUT 0x0103, RESOURCE_FLUSH 0x0104; OK_NODATA 0x1100 | V section 5.7.6.7, `virtio_gpu_ctrl_type` |
| SET_SCANOUT after the 24-byte header: rect x/y/width/height (four le32, offsets 0/4/8/12), scanout_id le32 at 16, resource_id le32 at 20 | V section 5.7.6.8, `virtio_gpu_rect` and `virtio_gpu_set_scanout`; scanout 0 and the full target rectangle are G2 choices |
| RESOURCE_FLUSH after the header: the same rect, resource_id le32 at 16, padding le32 at 20 (zero) | V section 5.7.6.8, `virtio_gpu_resource_flush` |
| VIRGL_BIND_SCANOUT bit 18 | H:648 at the pinned revision; targets add it to their existing render/sample flags for section 1.7 |
| Record scanout flag bit 0 and present fence/status at 184/192; resource/context ids and published generation/fences | O: gpu.md section 1.8; recorded only after both bind and flush answers with checked fence echoes |
| Output preflight one unchanged byte per 4,096-byte page through recoverable user access; tight staged BGRA output independent of caller stride | O: Breenix user-access and page APIs, implementing gpu.md section 1.4 EFAULT atomicity and section 1.8; padding is never published |
| Point 19 omits one transfer; 20 alters one returned transfer echo; 21 transfers a different owned resource; 22 appends a solid 1x1 draw at (0,0) in the complement of the poison; 28 omits one DRAW or DRAW_BUFFER | O: gpu.md section 1.8; the existing solid program/vertex grammar and wire values above encode the device draws |
| Full readback is staged until every band passed both complementary-poison observations; no pixels on any refusal | O: gpu.md sections 1.4 and 1.8 |
| Uncertain scanout bindings hold every candidate resource until a later completed bind/flush or confirmed reset; serialization survives interrupted device waits | O: gpu.md section 1.4.1, device-access hold sets |
| Status view 2560x1600; category rows at y=48+160r, case tiles at x=48+80c, size 64x112; background 0xFF181512, pending 0xFF484542, running 0xFF3894F2, pass 0xFF45C46A, fail 0xFFE84D42 | O: G2 display sizing and palette choices implementing gpu.md section 1.10; every tile uses the existing solid pipeline |

## B5: G2 device confirmation

The committed G2 checks are `userspace/programs/src/suite_gpu/readback.rs`, with the common scene
builder and exact canvas comparison in `common.rs`. Reproduce with the ARM64 production build and
`scripts/boot-interactive.sh --mode suite --suite gpu --gpu-gl --display --serial-log SERIAL`, where
SERIAL is a new output file chosen by the operator. The GL host stack and renderer pins are those of
section 1.6.1 and P above. No temporary experiment code, captured driver stream or new shader grammar
is used. The host renderer attestation was accepted.

The confirming source commit is `fca02d03be856b7ab0079e469169c92f94277ca2`, Vigil run
`20261017T044658Z-qemu`. Its producer printed `SUITE gpu DONE passed=53 failed=117 skipped=0 total=170`:
device 12/12, submit 22/22 and readback 19/19; the 117 failures are the unwritten G3–G8 checks, so this
is G2 acceptance, not a green full suite. The preceding run, `20261017T044322Z-qemu` at
`e587733f055a3b05631520d4b76d5e81be5689f8`, had all 19 readback checks passing but submit 21/22:
PRESENT rejected a valid new target before its first writer. The confirming commit repairs that
Breenix restriction and preserves `submit/fence-wait`'s generation-zero check.

| Inputs and committed check | Output establishing the fact | Fact and limit |
|---|---|---|
| `full_target`: BGRA target 47x13, target bind bits 1, 3 and 18; clear to BGRA (17,83,209,127). `present` sends SET_SCANOUT with rect (0,0,47,13), scanout 0 and the created resource, then RESOURCE_FLUSH for the same resource and rect. Each request carries a distinct fence, whose returned FENCE flag and echo are checked before accepting it. | `SUITE gpu CASE readback/full-target PASS`; `record-fields PASS` (with the same case-line prefix below) | Both specified requests are accepted as OK_NODATA (0x1100), and their fenced completions allow publication of the binding. This is the guest/device boundary; it makes no claim about the host window. |
| `record_fields`: a 17x9 BGRA target, clear plus overlapping solid rectangles, then present and read. Checks every defined field, nonzero identities, the scanout flag, actual present/readback/render command answers, all reserved/window bytes and the CAPS renderer string; repeats for monotonic observation serials. | `SUITE gpu CASE readback/record-fields PASS` | Record v2 and its zero reserved regions match section 1.8 for a presented target; this confirms the encoder and bookkeeping, not a new wire layout. |
| `format_canonical`: BGRA and RGBA targets 13x7, background 0x3C5AA5C3, rectangles (1,1,4,3) in BGRA (17,83,209,127) and (3,2,7,5) in (241,7,61,0). | `SUITE gpu CASE readback/format-canonical PASS` | Both formats, alpha included, return the exact canonical BGRA canvas. The solid programs and grammar are unchanged from G1. |
| `partial_rect`, `padded_stride`: target 73x17; boxes (1,1,3,4), (2,2,48,7), (72,16,1,1), (0,0,73,17); caller stride is 4w, or 4w+13. Device pitch remains 512. | `SUITE gpu CASE readback/partial-rect PASS`; `SUITE gpu CASE readback/padded-stride PASS` | Multiple rows and natural rows below or not aligned to 256 bytes are exact, with caller padding and tail guards untouched. This confirms the existing B3 transfer layout for these inputs. |
| `poison_unwritten`: pixels equal to the poison, its complement and a third colour; `refused`: separate owned targets and one armed transfer/fence/resource fault, then clean recovery before any explicit disarm. `fault_corrupt_pixel` and `fault_suppress_draw` compare actual device bytes with independent canvases. | `SUITE gpu CASE readback/poison-unwritten PASS`; `fault-skip-transfer PASS`, `fault-stale-fence PASS`, `fault-wrong-resource PASS`, `fault-corrupt-pixel PASS`, `fault-suppress-draw PASS` with the same case-line prefix | Two observations agree on written pixels, omitted transfers expose differing bytes, wrong identity is refused with its named reason, corruption differs at exactly (0,0), and a completed fence with a suppressed draw fails the exact comparison. Each armed point fires once and clean recovery succeeds. |
| `large_target`: full 2560x1600 target, background (17,83,209,127), one-pixel left/right/bottom edges and a 2x2 centre rectangle, as specified in the committed check. | `SUITE gpu CASE readback/large-target PASS` | Every pixel across all transfer bands is exact at the required 2560x1600 target size. No timing or later performance claim is made. |

The only new header value these requests need is H:648's SCANOUT bind bit; the two VirtIO request
layouts and command numbers above come from V. All other geometry, colours, reason codes, palette,
allocation sizes and hold-set decisions are Breenix choices, not policies learned from the host renderer.
The cache-invalidation boundary has two targeted host checks in `kernel/src/gpu/observation.rs`:
noncoherent memory exposes stale poison without invalidation, and the required structure check routes
both readback observations through that boundary. Both pass; no other host-test set was run for G2.
