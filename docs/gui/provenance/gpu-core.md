# GPU core provenance

This record names the source of every value the clean-room GPU core in `kernel/src/gpu/` uses: each
number, enumeration, structure layout, field offset and shader-grammar fact. The rules it follows are
`docs/gui/gpu.md` section 1.9. A value with no row here is not landed; the lane that adds a value adds its
row in the same PR. The core never uses `gpu_pci.rs`, `virgl.rs` or `svga3.rs`, and no legacy file, captured
stream or older program is a source for any row.

The core is `kernel/src/gpu/` and the `gpu-abi` crate (`libs/gpu-abi`), whose layouts the kernel and
libbreenix share. The VirtIO PCI transport it drives (`kernel/src/drivers/virtio/pci_transport.rs`) is
Breenix's existing driver for modern VirtIO PCI devices and is outside this record; the core adds two
accessors to it, the ISR byte's address (V §4.1.4.5) and the control queue's cached notification address
(V §4.1.4.4).

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
| ISR status byte: reading it acknowledges and deasserts INTx; bit 0 queue interrupt, bit 1 device configuration change | V §4.1.4.5 |
| A device that sets DEVICE_NEEDS_RESET after DRIVER_OK sends a configuration change notification, so the core reads the status register only after an interrupt whose ISR byte has bit 1 set; a device that stops answering meets the watchdog | V §2.1.2; the policy is O §1.4.1 |
| Notifying the device: after the available index is written, a 16-bit write of the queue index (0) to the queue's notification address, `queue_notify_off` times `notify_off_multiplier` past the notification capability | V §2.7.13.4, §4.1.4.4 |
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
| Shader object: handle, type, offset word (the text's byte length in bits 0–30 initially; subsequent commands carry byte offset with continuation bit 31 set), token count, stream-output count 0, then text | P:273–282 |
| Shader commands carry at most 4096 bytes including the command header and five shader fields; a program's ordered continuation commands share submission chunks up to the encoder's 64 KiB limit | O: core sizing choice; P:273–282 supplies continuation framing |
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

## G2 ABI

| Value or layout | Source |
|---|---|
| READBACK `reason` at 44 with codes 0 to 10 (success, unwritten, fence-mismatch, resource-mismatch, target-invalid, device-error, device-hung, device-reset, target-lost, target-dropped, binding-unknown); `failed_submission` at 56, nonzero only for target-invalid | O: `docs/gui/gpu-abi.md`, the `READBACK` table |
| RESOURCE_CREATE: 48 bytes, scanout intent flag bit 0 at 32, three reserved le32 words at 36; flag valid only for targets | O: `docs/gui/gpu-abi.md` `RESOURCE_CREATE`, ruling 19 (contract PR #1696) |
| PRESENT: 32 bytes, `target` at 0, `fence` out at 8 (a context fence `ns << 32 \| n`), `answer` out at 16, reserved at 20, `context` in at 24 | O: `docs/gui/gpu-abi.md`, `PRESENT`; `docs/gui/gpu.md` section 1.7 |

## G2 scanout and readback implementation values

| Value or layout | Source |
|---|---|
| SET_SCANOUT 0x0103, RESOURCE_FLUSH 0x0104; OK_NODATA 0x1100 | V section 5.7.6.7, `virtio_gpu_ctrl_type` |
| SET_SCANOUT after the 24-byte header: rect x/y/width/height (four le32, offsets 0/4/8/12), scanout_id le32 at 16, resource_id le32 at 20 | V section 5.7.6.8, `virtio_gpu_rect` and `virtio_gpu_set_scanout`; scanout 0 and the full target rectangle are G2 choices |
| RESOURCE_FLUSH after the header: the same rect, resource_id le32 at 16, padding le32 at 20 (0) | V section 5.7.6.8, `virtio_gpu_resource_flush` |
| VIRGL_BIND_SCANOUT bit 18 | H:648 at the pinned revision for the bit; O: `gpu-abi.md` `RESOURCE_CREATE` and ruling 19 for policy: only targets created with scanout intent add it to their render/sample flags |
| PRESENT sends SET_SCANOUT unfenced and RESOURCE_FLUSH fenced, both in the named context, as one fenced operation numbered in that context's sequence with its `SUBMIT`s; display ownership is tested by thread group in the critical section that queues them; a target with no completed write, or created without scanout intent, is refused EINVAL | O: `docs/gui/gpu-abi.md` `PRESENT`, `docs/gui/gpu.md` section 1.7; rulings 17 and 19 (contract PR #1696) |
| Record flags bit 0, `present_fence` 184 (that context fence) and `present_status` 192 (the flush's answer), set only for the target of the latest completed bind while no bind is outstanding | O: gpu.md section 1.8 |
| Output preflight: one aligned word in each page of the argument, the record and each pixel row (excluding the stride's gaps) is compare-and-exchanged with the value read from it, through the recoverable user-access routines | O: Breenix user-access API, implementing `docs/gui/gpu-abi.md` READBACK's EFAULT; no byte is stored that was not read |
| Point 19 omits one transfer; 20 alters one returned transfer echo; 21 sends one transfer naming another idle target of the process with the same row pitch that contains the box (without a matching target it stays armed), holding that target's backing, and is observed as unwritten; 22 adds a solid 1x1 draw at (0,0) in the complement of the poison to the written target whose handle it is armed for; 28 omits one DRAW or DRAW_BUFFER of the target whose handle it is armed for; armed with target 0, each applies to any | O: gpu.md section 1.8; the existing solid program/vertex grammar and wire values above encode the device draws |
| The whole rectangle's pixel bytes are staged in frames, copied through 512-byte stack chunks, and published only after the bands passed both complementary-poison observations; observation failures publish no pixels, while a late EFAULT may leave validated pixels as specified by the ABI. Temporary frames are charged to the guest-backing quota and RLIMIT_AS, may fail ENOMEM, and are included in COUNTERS.resource_bytes while held; freeing staging releases both charges | O: gpu.md sections 1.4 and 1.8 and `gpu-abi.md` `READBACK`/`COUNTERS`; ruling 18 (contract PR #1696) |
| Resources a sent bind could have installed is held until the latest bind's flush completes OK or a confirmed reset; until then a readback of a possible scanout is refused binding-unknown | O: gpu.md section 1.4.1, device-access hold sets; gpu-abi.md reason 10 |
| Status view 2560x1600; category rows at y=48+160r, case tiles at x=48+80c, size 64x112; background 0xFF181512, pending 0xFF484542, running 0xFF3894F2, pass 0xFF45C46A, fail 0xFFE84D42; drawn between cases after DISARM_ALL, and reported once per change when it cannot be drawn | O: G2 display sizing and palette choices implementing gpu.md section 1.10; the tiles use the existing solid pipeline |

## B5: G2 device confirmation

The committed G2 checks are `userspace/programs/src/suite_gpu/readback.rs`, with the common scene
builder and exact canvas comparison in `common.rs`. The historical runs below predate the final
recorder and creation-intent fixes; they are not attributed the reproduction command of a later run.
The fresh validation entry below gives its own exact commands, source commit and run ids. The GL host stack and renderer pins are those of
section 1.6.1 and P above. No temporary experiment code, captured driver stream or new shader grammar
is used. A reproducible confirming run must retain its `host-renderer.json` beside the serial and record `display:true`; the historical runs below do not retain that file and record `display:false`, so their renderer attestation and host-window behavior cannot be independently confirmed. Fresh validation belongs in the PR comment.

The historical source commit is `06e8637b6d087fdae385e06a04a337d65cc87321`, Vigil run
`20261017T055655Z-qemu`. Its producer printed `SUITE gpu DONE passed=53 failed=117 skipped=0 total=170`:
device 12/12, submit 22/22 and readback 19/19; the 117 failures are the unwritten G3–G8 checks, so this
is G2 acceptance, not a green full suite. The same commit booted without the GL device, Vigil run
`20261017T060608Z-qemu`, printed `[gpu] no GPU core device: no virtio-gpu PCI device` and
`SUITE gpu DONE passed=1 failed=169 skipped=0 total=170`: every case ran, `device/version` passed, and
each other case failed naming its layer, the rendering ones with `missing the GPU core with a device
admitted to the profile (G1): CAPS status 1 (no virtio-gpu PCI device)`.

| Inputs and committed check | Output establishing the fact | Fact and limit |
|---|---|---|
| `full_target`: BGRA target 47x13 (now explicitly created with scanout intent), target bind bits 1, 3 and 18; clear to BGRA (17,83,209,127). `present` sends SET_SCANOUT with rect (0,0,47,13), scanout 0 and the created resource, unfenced, then a fenced RESOURCE_FLUSH for the same resource and rect, in the case's context. The flush's returned FENCE flag and echo are checked before the binding is published. | `SUITE gpu CASE readback/full-target PASS`; `record-fields PASS` (with the same case-line prefix below) | Both specified requests are accepted as OK_NODATA (0x1100), and the fenced flush completes the binding. This is the guest/device boundary; it makes no claim about the host window. |
| `record_fields`: a 17x9 BGRA target, clear plus overlapping solid rectangles, then present and read. The current check additionally WAITs on the returned PRESENT fence and compares it exactly with offset 184 (the historical run predates those assertions). Checks the defined fields, nonzero identities, the scanout flag, the present fence at 184, actual present/readback/render command answers, the reserved/window bytes and the CAPS renderer string; repeats for monotonic observation serials. | `SUITE gpu CASE readback/record-fields PASS` | Record v2 and its reserved regions set to 0 match section 1.8 for a presented target; this confirms the encoder and bookkeeping, not a new wire layout. |
| `format_canonical`: BGRA and RGBA targets 13x7, background 0x3C5AA5C3, rectangles (1,1,4,3) in BGRA (17,83,209,127) and (3,2,7,5) in (241,7,61,0). | `SUITE gpu CASE readback/format-canonical PASS` | Both formats, alpha included, return the exact canonical BGRA canvas. The solid programs and grammar are unchanged from G1. |
| `partial_rect`, `padded_stride`: target 73x17; boxes (1,1,3,4), (2,2,48,7), (72,16,1,1), (0,0,73,17); caller stride is 4w, or 4w+13. Device pitch remains 512. | `SUITE gpu CASE readback/partial-rect PASS`; `SUITE gpu CASE readback/padded-stride PASS` | Multiple rows and natural rows below or not aligned to 256 bytes are exact, with caller padding and tail guards untouched. This confirms the existing B3 transfer layout for these inputs. |
| `poison_unwritten`: pixels equal to the poison, its complement and a third colour; `refused`: two owned 13x7 targets and one armed transfer, fence or resource fault, then clean recovery before any explicit disarm. `fault_corrupt_pixel` and `fault_suppress_draw` compare actual device bytes with independent canvases. | `SUITE gpu CASE readback/poison-unwritten PASS`; `fault-skip-transfer PASS`, `fault-stale-fence PASS`, `fault-wrong-resource PASS`, `fault-corrupt-pixel PASS`, `fault-suppress-draw PASS` with the same case-line prefix | Two observations agree on written pixels; an omitted transfer and a transfer naming the other target both leave the observed backing poisoned and are refused unwritten (reason 1); a stale echo is refused fence-mismatch; corruption differs at exactly (0,0), and a completed fence with a suppressed draw fails the exact comparison. Each armed point fires once and clean recovery succeeds. |
| `large_target`: full 2560x1600 target, background (17,83,209,127), one-pixel left/right/bottom edges and a 2x2 centre rectangle, as specified in the committed check. | `SUITE gpu CASE readback/large-target PASS` | The exact canvas comparison covers 4,096,000 pixels across the transfer bands at the required 2560x1600 target size, staged in charged frames. No timing or later performance claim is made. |

The only new header value these requests need is H:648's SCANOUT bind bit; the two VirtIO request
layouts and command numbers above come from V. The remaining geometry, colours, reason codes, palette,
allocation sizes and hold-set decisions are Breenix choices, not policies learned from the host renderer.
The cache-invalidation boundary and the readback path have host checks in `tests/gpu_observation.rs`:
noncoherent memory exposes stale poison without invalidation, both readback observations go through
that boundary. Source structure checks inspect the direct readback path for pixel-byte staging through charged frames and preflight compare-and-exchange. Their scope excludes heap metadata: `Pages` owns a frame-descriptor `Vec`, and `start_sync` copies hold lists with `to_vec()`. Only pixel bytes avoid heap vectors; the structure checks are distinct from the behavioral noncoherent-memory test.

## B6: contract-3 confirmation

Both profiles booted the clean source commit `396535337916928b836f08490091168e36c3ea54`
with the same ARM64 production kernel and disk, built once. The build commands were
`taskpolicy -b "$CHECKOUT/userspace/programs/build.sh" --arch aarch64`,
`taskpolicy -b "$CHECKOUT/scripts/create_ext2_disk.sh" --arch aarch64`, and, from CHECKOUT,
`taskpolicy -b cargo build --release --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`,
with `CARGO_BUILD_JOBS=4` exported. CHECKOUT is the absolute checkout of that source commit;
GL_RUN and NO_GL_RUN are separate, fresh absolute output directories. The exact boot
flags and quiet-wait setting used by the cited runs were:

```sh
BREENIX_QUIET=0 "$CHECKOUT/scripts/boot-interactive.sh" --no-build --mode suite --suite gpu --gpu-gl --display --serial-log "$GL_RUN/serial.txt" --gate-timeout 900 < /dev/null
BREENIX_QUIET=0 "$CHECKOUT/scripts/boot-interactive.sh" --no-build --mode suite --suite gpu --display --serial-log "$NO_GL_RUN/serial.txt" --gate-timeout 900 < /dev/null
```

| Profile and Vigil run | Producer output and retained metadata |
|---|---|
| GL, `20261017T071145Z-qemu` | `SUITE gpu DONE passed=53 failed=117 skipped=0 total=170`; device 12/12, submit 22/22, readback 19/19. `SUITE gpu CASE readback/full-target PASS ms=8` includes the new unwritten/offscreen PRESENT refusals, context-fence sequence and invalid creation flags. `SUITE gpu CASE readback/large-target PASS ms=125` includes staging ENOMEM under RLIMIT_AS, unchanged outputs/counters, recovery and released charges. Host renderer attestation accepted, 128 records. |
| Without GL, `20261017T071227Z-qemu` | `SUITE gpu DONE passed=0 failed=170 skipped=0 total=170`; 170/170 case records fail naming a missing layer `(G1)` through `(G8)`. |

Both native QEMU windows were observed open. The GL window displayed the GPU status tiles,
including the three green G1/G2 rows. The recorder stored `display:true` before each VM
started; both records report dirty=0 and the source commit above. The GL run retains
`host-renderer.json` beside its serial and a byte-identical copy in its Vigil run folder,
archived automatically by `vigil-record.sh` before finishing. No historical record was
relabelled. Both launchers exited 1: later G3–G8 checks remain unwritten, so neither result
is acceptance of the full GPU effort. The unrelated launcher-order expectation conflict
was tracked in #1697; ruling 21 subsequently authorized GL-first ordering in its check.

## G3 ABI

| Value or layout | Source |
|---|---|
| UPLOAD: 64 bytes, `context` 0, `resource` 8, `x, y, width, height` 16 to 28, `pixels` 32, `pixels_len` 40, `stride` 48, `source` 52, `purpose` 56, reserved 60; sources 0 B8G8R8A8, 1 R8G8B8A8, 2 R8, 3 B8G8R8, 4 R8G8B8; purposes 1 Image, 2 Fixture | O: `docs/gui/gpu-abi.md` `UPLOAD` (contract PR #1700) |
| Record v2 `canonical` 1: one byte per pixel of an R8 texture; a texture's render fields name its last upload, `render_kind` 0x0205 | O: `docs/gui/gpu.md` section 1.8, `docs/gui/gpu-abi.md` `READBACK` (#1700) |
| Stream commands COPY 5 (12 words) and DRAW_SAMPLED 6 (6 + 6n words); refusal codes 15 format mismatch, 16 wrong command, 17 samples its target; `copy` 1 step per fragment, a COPY 1 step per pixel per device copy | O: `docs/gui/gpu-abi.md`, the BGL command stream (#1700) |
| Test points 29 (`wrong-texture`, armed by texture handle; the process's other live texture created first) and 30 (`skip-upload-visibility`, armed by texture handle) | O: `docs/gui/gpu-abi.md`, test-control points (#1700) |
| An R8 texel `r` sampled as (R `r`, G 0, B 0, A 255) | A: [OpenGL ES 3.0.6](https://registry.khronos.org/OpenGL/specs/es/3.0/es_spec_3.0.pdf), section 3.9.2.1, table 3.24 (RED maps to R, 0, 0, 1); [Direct3D HLSL Texture Object](https://learn.microsoft.com/en-us/windows/win32/direct3dhlsl/dx-graphics-hlsl-to-type), Remarks, paragraph beginning "The default value for missing components"; O: `docs/gui/gpu.md` section 1.3, sampling (#1700); B: B8 and B9 below |

## G3 VirtIO and VirGL values

| Value or layout | Source |
|---|---|
| TRANSFER_TO_HOST_3D 0x0205 | V §5.7.6.7, `enum virtio_gpu_ctrl_type` |
| TRANSFER_TO_HOST_3D's request layout: the 3D transfer layout of U (lines 249–261) that TRANSFER_FROM_HOST_3D uses, box, offset, resource_id, level, stride and layer_stride | U, as in the four-layout table above; B9 confirms it on the device |
| Object types SAMPLER_VIEW 6, SAMPLER_STATE 7 | P:61–69 |
| Context commands SET_SAMPLER_VIEWS 10, RESOURCE_COPY_REGION 17, BIND_SAMPLER_STATES 18 | P:78–97 |
| Sampler state: 9 words: handle, S0, LOD bias, minimum and maximum LOD, four border-colour words; S0 wrap S, T and R at bits 0, 3 and 6 (3 bits each), minifying filter bit 9, mip filter bits 11–12, magnifying filter bit 13, compare mode bit 15, compare function bits 16–18, seamless cube map bit 19, anisotropy bits 20–24 | P:390–407 |
| Sampler view: 6 words: handle, resource, format, first and last layer (word 4), first and last level (word 5), swizzle with R, G, B and A selectors of 3 bits from bits 0, 3, 6 and 9 | P:411–423 |
| SET_SAMPLER_VIEWS: shader type, start slot, then the view handles | P:426–429 |
| BIND_SAMPLER_STATES: shader type, start slot, then the state handles | P:432–435 |
| RESOURCE_COPY_REGION: 13 words: destination resource, level, x, y, z, then source resource, level, x, y, z, width, height, depth | P:453–466 |
| Wrap CLAMP_TO_EDGE 2, the mip filter that selects no mip level 2, filter NEAREST 0 | D:181–184, D:194–197, D:202–203 |
| Swizzle selectors X 0, Y 1, Z 2, W 3: the identity swizzle word 0x688 | T:679–682 numbers TGSI's selectors X to W 0 to 3; that the sampler view's selectors read the same numbers is B: B8 and B9, where each channel of B8G8R8A8, R8G8B8A8 and R8 textures lands in its own byte |
| Texture bind SAMPLER_VIEW only; copy scratch textures RENDER_TARGET and SAMPLER_VIEW; neither has guest backing | H:636–638 for the bits; O: no transfer names either resource |

## G3 Breenix values

| Value | Source |
|---|---|
| Uploads stage the clipped source rows through 768-byte stack chunks in frames charged to both guest-backing quota and RLIMIT_AS, before converting or writing backing; EFAULT leaves textures and buffers unchanged | O: G3 sizing choice; `docs/gui/gpu-abi.md` `UPLOAD`, EFAULT |
| One device draw per DRAW_SAMPLED instance, each with its own texel-offset constant; up to 1,024 instances' vertices per inline write | O: G3 geometry choice; the vertex-region rule of G1 |
| A submission whose copies overlap within one target gets one device-only scratch texture per target format, sized to the largest such copy, created and attached like its vertex buffer and held with it | O: `docs/gui/gpu.md` section 1.3, copies |
| Each context has a third internal resource, a device-only 1×1 B8G8R8A8 texture that warm-up's `copy` draw samples; contexts warm 4 programs (`solid`'s and `copy`'s pairs) | O: `docs/gui/gpu.md` section 1.4.1, program warm-up |
| A submission's holds end once the device has returned its requests and its fence is published (or a confirmed reset ends them), so a fence kept unpublished keeps the resources it names | O: `docs/gui/gpu.md` section 1.4.1, hold sets |
| A texture upload takes the texture's backing for itself (the readback mark) and waits for unretired submissions of the process's other contexts that hold the texture | O: `docs/gui/gpu.md` section 1.4.1, uploads are ordered (#1700) |

## G3 shader programs

| Program | Instructions, declarations and semantics (L) | Token layout (T) |
|---|---|---|
| `copy` vertex: `VERT`, `DCL IN[0]`, `DCL OUT[0], POSITION`, `MOV OUT[0], IN[0]`, `END` | as `solid`'s vertex program, derived the same way: each catalogue pipeline is its own program pair (`docs/gui/gpu.md` section 1.3) | 11 tokens, as `solid`'s |
| `copy` fragment: `FRAG`, `DCL IN[0], POSITION, LINEAR`, `DCL OUT[0], COLOR`, `DCL SAMP[0]`, `DCL SVIEW[0], 2D, FLOAT`, `DCL CONST[0]`, `DCL TEMP[0]`, `IMM[0] INT32 {0, 0, 0, 0}`, `MOV TEMP[0], IMM[0]`, `F2I TEMP[0].xy, IN[0]`, `UADD TEMP[0].xy, TEMP[0], CONST[0]`, `TXF OUT[0], TEMP[0], SAMP[0], 2D`, `END` | TGSI_SEMANTIC_POSITION for a fragment input: the fragment's window position (L, "TGSI_SEMANTIC_POSITION"); F2I truncates toward 0 (L, "F2I - Float to Signed Integer"), so a pixel centre `p + 0.5` gives `p`; UADD returns the low 32 bits of the sum (L, "UADD - Integer Add"); TXF fetches one texel at an integer coordinate and level, ignoring the sampler state (L, "TXF - Texel Fetch"); SVIEW declares the sampler view and its return type (L, "Declaration Sampler View") | 39 tokens: header and processor (2); input declaration, range, interpolation (T:166–169, the declaration's Interpolate flag T:145) and semantic (4); output declaration, range and semantic (3); sampler declaration and range (2); sampler-view declaration, range and sampler-view token (T:253–259) (3); constant declaration and range (2); temporary declaration and range (2); immediate (T:284–290) and four values (T:292) (5); MOV, a destination and a source (3); F2I (3); UADD, a destination and two sources (4); TXF, its texture token (T:714–718, the instruction's Texture flag T:658), a destination and two sources (5); END (1). Files TEMPORARY 4, SAMPLER 5, IMMEDIATE 7, SAMPLER_VIEW 11 (T:77–84); LINEAR (T:111); FLOAT (T:249); INT32 (T:278); opcodes TXF, F2I, UADD (T:450, 476, 486); TEXTURE_2D (T:694) |

The fragment program's L sections are in the pinned `tgsi.rst`: TXF at line 858, F2I at 1024, UADD at 1056,
TGSI_SEMANTIC_POSITION at 3043 and Declaration Sampler View at 3508. The constant holds the draw's texel
offset `(sx − x0, sy − y0)` as 32-bit integers (SET_CONSTANT_BUFFER carries raw words, P:330–332), one
draw per DRAW_SAMPLED instance. With pixel centres at half-integers the truncated window position is the
pixel, and since the texel the sum names lies inside the texture, the 32-bit wrapping sum is exact.

## G3 shader-grammar facts

The renderer was the unmodified virglrenderer at P's pin, run on 2026-10-17 at 08:22:59 UTC with checkout
commit `84651e17df7c2e8c31c4c5b0057499462c935abc` (the source tested, not a commit containing the harness), through the same public-API harness pattern as G1's
(ruling 11): a host-only Python ctypes harness, committed at `tools/gpu-experiments/g3-egl.py`, with `exp-grammar.py`, `exp-render.py` and `copy_fs.txt` in the same directory, that loads virglrenderer and ANGLE EGL,
creates a 1×1 pbuffer and GLES contexts with the version-1 callbacks, and submits words with
`virgl_renderer_submit_cmd`. To reproduce with the committed harness (experiments `exp-grammar`
and `exp-render`), without machine-specific directory prefixes, use
`DYLD_LIBRARY_PATH="$QEMU_GL_FRAMEWORKS" DYLD_FRAMEWORK_PATH="$QEMU_GL_FRAMEWORKS" QEMU_GL_FRAMEWORKS="$QEMU_GL_FRAMEWORKS" python3 tools/gpu-experiments/g3-egl.py tools/gpu-experiments/<experiment>.py`
with `ANGLE_DEFAULT_PLATFORM=metal`, `QEMU_GL_FRAMEWORKS` denoting the pinned QEMU GL bundle's Frameworks
directory. Each candidate went to a fresh context as one CREATE_OBJECT SHADER (handle 1, the fragment
stage, the text's length with its NUL, the stated token count), the text packed into little-endian words
with 0 bytes as padding; 0 is acceptance and 22 refusal. The baseline is the committed `copy` fragment program
above with 39 tokens.

| Change from the baseline | Result | Fact established |
|---|---|---|
| (the baseline itself) | accepted (0) | `POSITION, LINEAR` on a fragment input, `SAMP`, `SVIEW[0], 2D, FLOAT`, `TEMP`, `IMM[0] INT32 {…}`, swizzled `.xy` destinations, F2I, UADD and four-operand TXF are accepted together |
| token count 38, and 100 | accepted (0) both | these two capacities are accepted for this small program; they do not establish acceptance of an arbitrarily small capacity (B10); the layout count comes from T |
| `, LINEAR` removed | accepted (0) | the interpolation qualifier is optional to the parser; the program keeps it from L |
| `DCL SVIEW[0], 2D, FLOAT` removed | accepted (0) | the parser does not require the view declaration; the program keeps it, as L asks for typed views |
| `INT32` removed from `IMM[0]` | refused (22, "Expected immediate type") | an immediate names its type |
| `INT32` replaced with `UINT32` | accepted (0) | UINT32 is also an immediate type |
| `, 2D` removed from TXF | refused (22, "Expected `,`") | TXF names its texture target as its last operand |
| `FLOAT` replaced with `UNORM` in SVIEW | accepted (0) | the return-type word is parsed from L's list |

## B8: G3 host-renderer pixel experiments

The same harness and run, experiment `exp-render`. Its resources come from
`virgl_renderer_resource_create` and join the contexts through `virgl_renderer_ctx_attach_resource`;
their memory comes from `virgl_renderer_resource_attach_iov`, and their transfers from
`virgl_renderer_transfer_write_iov` and `virgl_renderer_transfer_read_iov` at a 256-byte stride. The command words follow the
encoder's layout for each step: the context's state objects, the `copy` program pair, a sampler state with S0
`sampler_s0(2, 0, 2)`, a sampler view with the identity swizzle, and per draw SET_CONSTANT_BUFFER with the
offset, RESOURCE_INLINE_WRITE of six vertices and DRAW_VBO. Bytes are given B, G, R, A for B8G8R8A8.

| Inputs | Output excerpt | Fact |
|---|---|---|
| A 5×3 B8G8R8A8 texture, texel (x, y) = (40x + 1, 80y + 2, 7, 255 − x), uploaded whole; an 8×6 B8G8R8A8 target cleared to (0x33, 0x33, 0x33, 0xFF); one `copy` draw of the rectangle (2, 1)–(6, 4) with offset (−1, −1) | `write 0`, `submit 0`, `read 0`; row 1 `333333ff 333333ff 290207fe 510207fd 790207fc a10207fb 333333ff 333333ff`, rows 2 and 3 the same texels with G 0x52 and 0xA2 | pixel (x, y) reads texel (x − 1, y − 1): the truncated window position is the pixel, row 0 is the target's top row (the same orientation as `solid`), the constant is read as integers, and B8G8R8A8 texels keep their bytes |
| A 64×2 R8 texture with bytes 0 to 63 and 100 to 163, a 4×2 R8G8B8A8 target, one `copy` draw of (0, 0)–(4, 2) with offset (60, 0) | `3c0000ff 3d0000ff 3e0000ff 3f0000ff`, `a00000ff a10000ff a20000ff a30000ff` (R8G8B8A8 bytes R, G, B, A) | an R8 texel `r` reads as (R `r`, G 0, B 0, A 255), and an R8 row of 64 bytes transfers at a 256-byte stride |
| RESOURCE_COPY_REGION within the 8×6 target: source (2, 1) 3×2, destination (0, 4) | rows 4 and 5 `290207fe 510207fd 790207fc 333333ff …` and `295207fe 515207fd 795207fc …` | the 13 words are read as laid out: exactly the named 3×2 rectangle moves |
| The texture's row 0 re-uploaded as (9, 9, 9, 9) in context 1, then a `copy` draw of (6, 0)–(8, 2) with offset (−6, 0) in context 2, which the texture, target and vertex buffer were also joined to | context 2's readback row 0 ends `09090909 09090909`, row 1 `015207ff 295207fe` | an upload made in one context is visible to a later draw in another on this renderer; the contract still has `UPLOAD` wait for other contexts' submissions naming the texture, since the device states no order between contexts |

## B9: G3 device confirmation

The committed G3 checks are `userspace/programs/src/suite_gpu/textures.rs`. Each boot below was built and
booted in a worktree whose Rust fork was set up with `scripts/rust-fork.sh sync`, with
`CARGO_BUILD_JOBS=4` exported, by `taskpolicy -b "$CHECKOUT/userspace/programs/build.sh" --arch aarch64`,
`taskpolicy -b "$CHECKOUT/scripts/create_ext2_disk.sh" --arch aarch64` and, from CHECKOUT,
`taskpolicy -b cargo build --release --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`,
then
`BREENIX_QUIET=0 "$CHECKOUT/scripts/boot-interactive.sh" --no-build --mode suite --suite gpu --gpu-gl --display --serial-log "$RUN/serial.txt" --gate-timeout 900 < /dev/null`.
CHECKOUT is the absolute checkout of the source commit and RUN a fresh absolute output directory. Each
record reports `display:true` and dirty 0, and each run folder keeps `host-renderer.json`; the launcher
accepted the attestation, `ANGLE Metal Renderer`, with 170 records for the first two runs, 178 for the
third, whose added case steps create more contexts, and 180 for the fourth. The fourth run was built and
booted in one step by
`CARGO_BUILD_JOBS=4 BREENIX_BUILD_BACKGROUND=1 "$CHECKOUT/scripts/boot-interactive.sh" --mode suite --suite gpu --gpu-gl --display --serial-log "$RUN/serial.txt" < /dev/null`,
which runs the same three builds under `taskpolicy -b` before booting.

| Source commit and Vigil run | Producer output |
|---|---|
| `309ade3b02459d78daf6677e3e5ebbf6104acc85`, `20261017T083611Z-qemu` | `SUITE gpu DONE passed=71 failed=99 skipped=0 total=170`: device 12/12, submit 22/22, readback 19/19, textures 17/19, `pipelines/copy` PASS. `textures/quota FAIL … RESOURCE_CREATE of a 4096x4096 texture in format 0 returned ENOMEM`; `textures/readback-uploaded FAIL … CONTEXT_CREATE returned ENOMEM` |
| `b71c5aa7a39993f26c3384e3a06684c46e833457`, `20261017T084718Z-qemu` | `SUITE gpu DONE passed=71 failed=99 skipped=0 total=170`: device 12/12, submit 22/22, readback 19/19, textures 17/19, `pipelines/copy` PASS. `textures/quota FAIL … the case crashed: killed by signal 11`; `textures/fault-skip-upload FAIL … CONTEXT_CREATE returned ENOMEM` |
| `9afc54cded748521ec09e9478fb0a779d65d08e0`, `20261017T112047Z-qemu` | `SUITE gpu DONE passed=73 failed=97 skipped=0 total=170`: device 12/12, submit 22/22, readback 19/19, textures 19/19, `pipelines/copy` PASS. `SUITE gpu VALUE textures/upload-visibility hung-upload-ms=2000ms`, `reset-ms=4006ms`; `SUITE gpu CASE textures/upload-visibility PASS ms=4116`; `SUITE gpu CASE textures/quota PASS ms=62` |
| `507d7515977d99c523a15cd4c8504608f10ec667`, `20261017T122823Z-qemu` | `SUITE gpu DONE passed=73 failed=97 skipped=0 total=170`: device 12/12, submit 22/22, readback 19/19, textures 19/19, `pipelines/copy` PASS. `SUITE gpu VALUE textures/immutable-while-pinned ordering-upload-ms=13ms`, `ordering-in-flight-after-upload=0`, `ordering-draws=1024`; `SUITE gpu CASE textures/immutable-while-pinned PASS ms=4021` |

Facts the passing cases establish on the device, beyond B8's: TRANSFER_TO_HOST_3D with U's layout writes
the box at `offset` = y × pitch + x × bytes per pixel with the 256-byte-aligned pitch as stride, including
B8G8R8A8 textures 48 wide and R8 textures 64 wide, whose natural rows of 192 and 64 bytes are not multiples
of 256 (`upload-full`,
`upload-subrect`, `upload-clip`, `upload-stride`, `unaligned-pitch`, `readback-uploaded`); TRANSFER_FROM_HOST_3D
reads textures as it reads targets; RESOURCE_COPY_REGION between and within targets, and through a scratch
texture for overlapping rectangles (`copy-region`, `copy-overlap`); a submission whose fence is retained unpublished samples earlier texels and buffer snapshots
(`immutable-while-pinned`); because the depth point may hold an already completed answer, this is not
evidence that commands execute in context order; points 29 and 30 are caught only by the
exact comparison and by reading the texture back (`fault-wrong-texture`, `fault-skip-upload`).

The failed-upload experiment (ruling 25) is `upload-visibility`'s last part, from the third run. Point 31 with
parameter 1, armed before a submission of another context, stops the kernel taking the device's answers once that
submission's answer arrives, which the case sees as the point's FIRED count rising. A whole-texture `UPLOAD` then
sends its TRANSFER_TO_HOST_3D, and a 30 ms ITIMER_REAL signal interrupts its wait: EINTR. A second whole-texture
`UPLOAD` is ended by the watchdog: EIO after `hung-upload-ms=2000ms`. After each, a submission of a third,
live context naming the texture as a `COPY` source or a `DRAW_SAMPLED` texture is refused with code 14
(`INVALID_TARGET`), so a sent and failed upload leaves its texture invalid on this device. The stall
ends in the watchdog's device reset (`reset-ms=4006ms`), after which a new context draws and reads back exactly.
Readback of either texture cannot show the invalid state in that window: it waits for the outstanding transfer,
which only the reset ends, and the reset marks the texture lost.

The same-context ordering experiment (ruling 26) is `immutable-while-pinned`'s first part, from the fourth run.
No point is armed. One context sends a submission of 1024 `copy` draws, each sampling the whole of a 1024×1024
texture into a 1024×1024 target (2^30 fragment-steps). A zero-timeout `WAIT` on its fence immediately
before a one-texel `UPLOAD` to that texture in the same context returned ETIMEDOUT, so the upload was issued
while the submission was unanswered. The target then read back exactly as the texture was before the
upload, and a submission sent after the upload returned read the new texel. On this device, then, a
submission sent before an upload in the same context samples the texture as it was before. The submission's
fence had been answered by the time the `UPLOAD` returned (`ordering-in-flight-after-upload=0`), so the run
does not show whether the device answers a transfer before the work sent ahead of it in that context.
Had the fence already been answered at the `WAIT`, the case would have reported that run as
`ordering-finished-early-draws` and repeated it once with 2048 draws (2^31, the work cap), failing if that run
too were answered first.

The first two runs' quota failures came from ARM64's frame allocator then spanning 0x4400_0000 to 0x5000_0000
(`[boot] Initializing memory management (0x44000000-0x50000000)`), 192 MiB for the whole kernel, equal to the
per-process quota, so the failed case's textures starved the next case until the reclamation sweep. With the frame
pool taken from the firmware's memory description (ruling 23, #1704), `textures/quota` passes in the third run.
These runs are G3 evidence, not a green full suite: the G4–G8 checks are still unwritten.

## G4 profile inputs and arithmetic

| Value or layout | Source |
|---|---|
| PROBE 7, SCISSOR 8, their word layouts, routine selectors 0–24, result-word selection, operand domains, refusal codes 18 and 19, filtered modifier bit 31 and point 24's validator injection | O: proposed contract PR #1709, `docs/gui/gpu-abi.md`, BGL command stream. G4 depends on that contract PR. |
| Probe work weight 65,536 steps per pixel; scissor clips drawing but leaves COPY independent; BEGIN restores target bounds | O: #1709; the weight budgets the bounded arithmetic program, rather than a host-renderer policy |
| Creation warms four programs, `solid`'s and `copy`'s vertex and fragment pairs; a context's first PROBE creates `probe_int`'s pair in that submission, and its draw warms them | O: `docs/gui/gpu.md` sections 1.3 and 1.4.1 as amended by #1709; the probe is one fragment program with 25 integer-selected branches |
| Probe result bytes: extract bits 16–23, 8–15, 0–7 and 24–31 for the R, G, B and A output components, convert unsigned integers to floats, multiply by binary32 `1/255` (0x3B808081) | O: #1709's canonical BGRA word; A: IEEE 754 binary32 round-to-nearest-even; L: U2F and MUL |
| NaN result 0x7FC00000, finite truncation domain, signed-zero and gradual-underflow rules | O: #1709; A: IEEE 754-2019 sections 3.4 and 4.3.1 <!-- claim-lint:ok: #1709 numerical requirements, not measured results. --> |

`tools/gpu-profile/math.rs` is Breenix's integer model and TGSI generator. It was written from the
contract's formulas and IEEE 754, without implementation sources. It constructs SSA operations, evaluates
those operations for the host model, and emits them using L's instruction definitions. Comparisons are
normalized to 0 or 1 with AND; final word selection uses integer bit operations (B11). Products use 16×16 partial products with explicit carries.
The three-word division model unrolls 96 restoring steps; its shader encoder expresses that recurrence as a
96-iteration loop, retaining the shifted-out remainder bit for a divisor with its high bit set.

The sticky-bit shift is derived here by OR-reducing discarded bits into the low retained bit so
rounding preserves whether any discarded bit was nonzero; the former name `jam` named that operation,
not an implementation source. The routine is now named `sticky_shift`.

Binary32 add/subtract align significands with guard, round and sticky bits, normalize, and round ties to
even; multiplication uses the two-word significand product and the same rounding step. The model's native
f32 comparisons were run before shader emission: `binary32_model_matches_native_arithmetic` checks 14×14
special-value pairs and 2,000 seeded pairs for add, subtract and multiply;
`binary32_ceil_truncate_and_comparisons` checks 4,000 seeded pairs (truncation for inputs in its domain).
The other model checks compare wide arithmetic with u128, line and circle membership with the contract's
walks, and large-coordinate literals. The truncated three-word product also has 1,000 seeded comparisons
with native wrapping u128 multiplication before its revised shader is emitted. These host checks are reproducible with CARGO_BUILD_JOBS=4 exported and
`taskpolicy -b rustc --edition=2021 --test -O "$CHECKOUT/tools/gpu-profile/math.rs" -o "$MODEL_TESTS"` and
`taskpolicy -b "$MODEL_TESTS" --test-threads=1`, where MODEL_TESTS is an absolute temporary output filename and CHECKOUT is absolute.
They validate the model; device readback remains the profile suite's judgment of the shader.

The predicates use the contract's widths. Line membership uses the major-axis step k and minor displacement
`floor((2*k*minor + major)/(2*major))`, with endpoint bounds and direction checked; a point line is handled
separately. Circles use section 1.5's squared-distance and outline predicates. Rounded corners use §5.6's
pixel-centre formula. Triangles use signed three-word edge functions at pixel centres and the top-left rule.
These probe routines do not supply G5's promised closed-form proofs or drawing pipelines.

## G4 shader derivation

| Program or operation | Published definition | Token layout |
|---|---|---|
| Probe vertex program | L: MOV (line 71), POSITION declaration; the pass-through is derived as solid's is | T:37–56, 137–158, 174–175, 226–234, 649–661, 752–764, 796–803; 11 tokens |
| `kernel/src/gpu/virgl/profile/rNN.tgsi` (one program per routine, ruling 48) | L: MUL 111; U2F 1009; F2I 1024; UADD 1056; UMUL 1088; AND 1192; OR 1205; XOR 1218; SHL 1283; USHR 1313; USLT 1384; USEQ 1453; USNE 1481; BGNLOOP 1643; ENDLOOP 1653; BRK 1668; UIF 1684; ENDIF 1698; the declarations and POSITION/COLOR semantics used by solid and copy | T:37–56 header/processor; 137–169 declaration/range/interpolation; 226–234 semantics; 274–292 UINT32 immediates and their values; 649–688 instructions and optional label; 752–803 registers. TEMPORARY 4 and IMMEDIATE 7 are T:77–80; the opcode enumeration is T:434–518. |

The generated text is a reviewable file without a NUL; the shader encoder supplies its wire terminator.
Its header declares the five constant vectors (sixteen operands, result-word selector, the routine number, which
the per-routine programs do not read, and B19's runtime constant 0),
window position, colour output and temporaries. Sources selecting one component use a four-component
replicated swizzle, as B10 establishes. The file is generated with
`taskpolicy -b rustc --edition=2021 -O "$CHECKOUT/tools/gpu-profile/math.rs" -o "$GENERATOR"`, then
`taskpolicy -b "$GENERATOR" "$CHECKOUT/kernel/src/gpu/virgl/profile"`, with CARGO_BUILD_JOBS=4 exported; GENERATOR and CHECKOUT are absolute paths.
The integer model and generator share operations; the suite's literal expectations share neither.

At `a66cc5986b0e70f85098572e4565e3eb630460fc`, the token capacity was 23,233: 13 header/declaration words, 141×5 immediate words,
22,482 instruction/register words, and a 33-word allowance for the control instructions' optional label
tokens (T:649–688). The text plus terminator was 252,216 bytes, below P:157–158's command payload limit.
The review revision eliminates repeated expressions and constant operations, and uses low-word UMUL for
terms discarded above bit 95. The generator prints the revised size and token budget when run; the
emitted-ISA host tests execute that generated text, including loops and integer rewrites. Shader text now
uses the continuation framing above rather than requiring one large command. Context creation queues only the fixed rendering catalogue's warm-up (section 1.4.1); the probe program's compilation is charged to its context's first PROBE, under the same watchdog and case deadlines.
Ruling 48 splits `probe_int` into one program per routine, `profile/r00.tgsi` to `r24.tgsi`, so that each
compiles within the 2 s watchdog: the combined program's first PROBE compiled for 3,146 ms on Parallels. Each
program is the routine's own emitted text through the same integer rewrites, with no routine dispatch; the
generator prints each program's size and token count, and the emitted-ISA tests run each of the 25 routines'
own programs. A context creates a routine's vertex and fragment pair on that routine's first PROBE, so `warmed`
grows by two per routine probed.
The earlier B10 candidate at `09922c96` had capacity 21,940 and 240,200 text bytes including its terminator;
its rejected 21,257 field omitted 130×5 immediate words and the label allowance. B10 records those original
inputs; B11 records the integer-only selection program at `41766187`, and B12 the runtime literal construction. These capacities are layout-derived
allocation budgets, not pixel-correctness claims.

## B10: G4 shader-text grammar experiments

This section records the candidate and six-experiment script at commit `09922c96`; CHECKOUT below denotes
that checkout when reproducing B10. B11 uses the revised current program.

The unmodified renderer is P's pin, with ANGLE on Metal, through the public API harness
`tools/gpu-experiments/g3-egl.py` (ruling 11). The committed experiment is
`tools/gpu-experiments/profile-grammar.py`. From the checkout, reproduce with
`QEMU_GL_FRAMEWORKS="$FRAMEWORKS" DYLD_LIBRARY_PATH="$FRAMEWORKS" DYLD_FRAMEWORK_PATH="$FRAMEWORKS" taskpolicy -b python3 "$CHECKOUT/tools/gpu-experiments/g3-egl.py" "$CHECKOUT/tools/gpu-experiments/profile-grammar.py"`.
CHECKOUT is absolute; FRAMEWORKS is the pinned GL bundle's Frameworks directory. Each candidate goes to a
fresh context as a CREATE_OBJECT SHADER, fragment stage, no stream output, text length including its NUL,
packed little-endian with padding. Return 0 is acceptance, 22 refusal. Acceptance establishes grammar and
capacity for these inputs; it does not establish pixels.

The small baseline is the literal `base` string in the experiment: FRAG, COLOR output, CONST[0], TEMP[0],
UINT32 immediate `{0, 1, 4294967295, 0}`, MOV, UADD with a scalar destination and replicated sources,
U2F and END. Its stated capacity is 25 tokens. The combined baseline is the generated text above with
capacity 21,940. Renderer diagnostic output between the named results is omitted here.

| Candidate, one change from its baseline | Result | Fact established |
|---|---|---|
| Small baseline itself | `G4 replicated scalar baseline ACCEPT 0` | UINT32's unsigned decimal maximum, a scalar destination and four-component replicated source swizzles are accepted together |
| `CONST[0].xxxx` changed to `CONST[0].x` | `G4 scalar source shorthand REFUSE 22`, `Expected register swizzle component` | A single-component source shorthand is refused; the generator replicates the component |
| Before U2F, insert `UIF IMM[0].yyyy`, `MOV TEMP[0], IMM[0]`, `ENDIF` (capacity 31) | `G4 integer conditional ACCEPT 0` | The integer conditional block's text form is accepted |
| Before U2F, insert `BGNLOOP`, `BRK`, `ENDLOOP` (capacity 28) | `G4 bounded loop syntax ACCEPT 0` | Those loop delimiters and break text are accepted |
| Combined baseline itself | `G4 combined baseline ACCEPT 0` | The generated program, constant/temporary ranges and sequential immediates are accepted together at the corrected capacity |
| First `IMM[0] UINT32` changed to `IMM[0]` | `G4 missing immediate type REFUSE 22`, `Expected immediate type` | The immediate type remains required in this program |

The capacity defect was isolated without changing the candidate text: the combined program with capacity
21,257 was refused 22, and with capacity 50,000 was accepted 0. Its individual branches 0, 11, 12, 23 and 24,
with the same declarations and capacity 50,000, were accepted 0. The T-based recount above repaired the
omitted fields. B7's small-program count variations therefore do not establish that an arbitrarily small
capacity is accepted. The committed `tools/gpu-experiments/profile-capacity.py` reconstructs those exact candidates from
`09922c9652d3820b82e1c1cad007180a7f08f25a`. Run it with B10's invocation, replacing
`profile-grammar.py` with `profile-capacity.py`. Its named outputs are `G4 B10 capacity 21257 REFUSE 22`
and `G4 B10 capacity 50000 ACCEPT 0`; it also reconstructs the five branch candidates
with their original declarations and capacity.

## B11: integer word selection

The emitted word copy is UADD with an integer 0. Word selection uses the identity
`b XOR ((a XOR b) AND m)`, where USNE of the condition and 0 supplies m (0 or UINT_MAX).
This expresses selection in L's integer ISA, including operands whose bits encode floating-point NaNs;
there is no untyped MOV or UCMP in the final fragment text. The final byte conversion and the fragment
position conversion retain their specified floating-point operations. Two temporary registers are reserved
above the modeled program's registers for the mask and difference, so a selection whose destination aliases
an input reads that input before overwriting it.

The three-word product computes the low product, accumulates the two middle-word cross products with their
carries into the high word, and adds only the low words of the three remaining high-word cross products.
Its partial products remain 16×16 (section 1.5); dropping terms above bit 95 changes no returned bit. The
native product test above checks this truncation. This reduction leaves room for the integer selection
instructions within one VirGL command, without changing the probe's work limit or its literal expectations.

The public renderer harness, pin, environment and invocation are B10's, with CHECKOUT denoting commit `41766187` in this section. `profile-grammar.py` adds one change to the small B10 baseline: replace its UADD opcode with USNE
(capacity 25). Output: `G4 unsigned inequality ACCEPT 0`. L defines USNE at line 1481; its enumeration is
T:497 and its instruction and operand layouts are T:649–661 and 752–803. The complete named output at that commit was:

```text
G4 replicated scalar baseline ACCEPT 0
G4 scalar source shorthand REFUSE 22
G4 integer conditional ACCEPT 0
G4 bounded loop syntax ACCEPT 0
G4 unsigned inequality ACCEPT 0
G4 combined baseline ACCEPT 0
G4 missing immediate type REFUSE 22
```

`G4 combined baseline ACCEPT 0` here refers to that commit’s generated text, capacity
23,165, 44 temporary registers and 251,697 bytes including the NUL. This is grammar acceptance, not a
substitute for the profile cases' device readback.


## B12: NaN-shaped word literals

CHECKOUT in this section denotes commit `a66cc5986b0e70f85098572e4565e3eb630460fc`.
A renderer behavior experiment isolated an expression whose inputs are integer immediates:
`150 XOR 0xffffffff` returned `0x7fc00000`, rather than the integer result `0xffffff69`.
The public API experiment is `tools/gpu-experiments/profile-words.py`, run with B10's unmodified renderer,
pin and environment. Reproduce with
`QEMU_GL_FRAMEWORKS="$FRAMEWORKS" DYLD_LIBRARY_PATH="$FRAMEWORKS" DYLD_FRAMEWORK_PATH="$FRAMEWORKS" taskpolicy -b python3 "$CHECKOUT/tools/gpu-experiments/g3-egl.py" "$CHECKOUT/tools/gpu-experiments/profile-words.py"`.
The harness creates a one-pixel BGRA render target, draws a rectangle, transfers that device pixel through
the public read API, and decodes its raw word. The packet fields retain their P/D/H sources above.

The first candidate is the literal `header + immediates + XOR + pack` in that file: integer immediate
150, UINT_MAX, integer XOR, byte extraction by USHR and AND, U2F and normalization by binary32 1/255.
Its capacity is 68 tokens: header/declarations 9, three immediates 15, XOR 4, eight byte-extraction
instructions 32, U2F 3, MUL 4 and END 1. The second candidate changes the UINT_MAX immediate to 0 and
constructs that operand with UADD of the uniform containing 0 and 65535, SHL by 16, and OR with 65535; its capacity
is 80. The output was:

```text
G4 raw literal XOR 7fc00000
G4 runtime literal XOR ffffff69
G4 repaired routine 15 80000000
G4 repaired routine 21 00000001
```

The last two lines use the committed combined shader, with the exact operands specified in the script:
binary32 truncation of `0xcf000000`, and outline membership at `(INT_MAX,1)` for a circle centred at (0,0)
with radius INT_MAX. These observations localize the literal-expression hazard; attributing it to floating
constant folding is an inference, not implementation-code evidence. They do not change the required word
results or substitute for the guest suite.

The generator now replaces references to the four NaN-shaped literal words (UINT_MAX, `0x7fffffff`,
`0x7fc00000` and `0xffffff81`) with dedicated temporaries, constructing their bits from finite 16-bit
immediates using the second candidate's integer sequence. CONST[4].z is the draw's supplied 0 word;
its seed keeps the construction at runtime. The old immediate components become 0. Four registers
above the model and selection registers hold these words, giving 48 temporaries in the combined shader.
Its capacity and byte length are recorded above. The seven B11 grammar experiments returned their stated
results on this revised candidate, including `G4 combined baseline ACCEPT 0` at capacity 23,233.

## G5 parameterized catalogue

| Value or design | Source |
|---|---|
| DRAW_PIPELINE command 9; ten header words; twenty words per instance; sixteen parameter words; overlap refusal 20 and parameter refusal 21 | O: contract PR #1714, `docs/gui/gpu-abi.md`, G5 parameterized draws |
| Pipeline parameter domains, clipped source footprints, destination snapshots, scratch size and work weights | O: #1714 and `docs/gui/gpu.md` sections 1.3–1.5; clipping uses wide integer geometry, rather than CPU pixels |
| Fragment constants 0–3: the sixteen parameters; 4: integer 0 seed; 5: draw colour; 6: texture dimensions; 7 reserved | O: encoder layout for #1714; shape colours use G1's binary32 UNORM encodings, other programs receive the packed BGRA word |
| Sampler slots 0 source, 1 destination snapshot, 2 crossing data; nearest states bound in those slots | O: encoder bookkeeping; the commands, states and view layouts retain the P/D/H/T rows above |
| Crossing data in a temporary RGBA8 texture: one u32 per texel, a power-of-two row width no larger than the reported device maximum, four bytes per texel, inline writes of at most 1,024 words | O: parameter storage choice; format 67 is H:108, RESOURCE_INLINE_WRITE is P:521–532; data words are geometry, not a coverage image |
| Context creation's sixteen rendering pairs (32 programs); probe_int's two programs remain lazy | O: `docs/gui/gpu.md` section 1.4.1 and #1714; the existing context-create case checks the count and reports creation time against 10 s |

The new vertex programs use the same L-derived position pass-through as G1, each with its own device
object pair. The fragment programs and token capacities are generated by `tools/gpu-profile/math.rs`
with `--pipelines`, through `tools/gpu-profile/pipelines.rs`; their text and capacities are in
`kernel/src/gpu/virgl/pipelines/`. Reproduce with CARGO_BUILD_JOBS=4 exported,
`taskpolicy -b rustc --edition=2021 -O "$CHECKOUT/tools/gpu-profile/math.rs" -o "$GENERATOR"`, then
`taskpolicy -b "$GENERATOR" --pipelines "$OUTPUT"`. CHECKOUT, GENERATOR and OUTPUT are absolute paths.
The generator's output was compared byte for byte with the kernel's included files before the device run.

| Programs | Derivation and published instructions |
|---|---|
| line, circle_fill, circle_outline, rounded_rect, triangle | G4's Breenix-authored integer predicates, now applied to the fragment's pixel, with the colour written for membership and KILL otherwise; L's integer instructions listed in G4, plus KILL at L:661 and ELSE at L:1693. triangle computes the model's area test once and runs its edge test three times in a loop like coverage_spans's, the vertices rotating between passes (807 lines rather than 1,501; on Parallels the three-copy program compiled for 0.43 to 0.75 s); the emitted-ISA test `triangle_pipeline_matches_the_model_predicate` runs the committed program against the model on 46 triangles at 221 pixels each |
| gradient_v | The contract's weighted numerator in two words, restored division and packed channel output; G4's integer model and L instructions |
| copy_scaled, nearest_scaled, box_filter | The contract's integer mappings; 64 restoring division steps, explicit remainder carry and normalized comparison bits; box means accumulate two-word sums and round half up; L's integer instructions, TXF at 858, and the boundary conversion below |
| over, over_straight, coverage, bitmap_text | Integer byte channels, div255 and the contract's source-over, coverage or bit-row rules; TXF and L's integer instructions; bitmap scale uses the bounded restoring divider |
| coverage_spans | Per-row eight-sub-scanline traversal of the snapshotted crossing lists; winding and fill_start follow section 2.1; span subtraction, ordered accumulation, multiplication and truncation use G4's IEEE-754 integer model. The subtraction and the accumulation run one binary32 addition twice, and the two scalings one binary32 multiplication twice, each in a loop whose bound is computed from `CONST[4].x`, which the encoder's catalogue draws and warm-up draw supply as 0 (`kernel/src/gpu/virgl/encode.rs`), so the bound is unknown at compile time and the program carries one copy: on Parallels this program's warm-up step took 0.86 to 1.64 s across 124 contexts against the 2 s compile watchdog, and the loops cut its text from 2,437 to 1,787 lines |
| UNORM boundary conversion | L:111 MUL, 124 ADD, 1040 F2U: floor(v*255+0.5); O: #1714's fetch boundary. Output uses L:1009 U2F and MUL by binary32 1/255, as G4. Native float operations do not compute span coverage. |

<!-- claim-lint:ok: Literal selection is implemented by integer_words in tools/gpu-profile/math.rs and checked by integer_rewrites_use_a_declared_zero_literal in tools/gpu-profile/emitted_tests.rs. -->
The G5 integer lowering selects an actual declared zero immediate for its word-copy UADD and
selection USNE. The model's live literals can place a sign bit or UINT_MAX in the first slot;
that slot is not an implicit zero. The emitted-ISA regression
`integer_rewrites_use_a_declared_zero_literal` checks the zero-radius circle's centre result.

The token accounting retains T's header, processor, declaration/range, immediate, instruction,
register and optional flow-label layouts recorded above. The programs declare samplers and sampler views individually after B14
showed that accepted range text did not produce a working sampler-0 draw. Branches guard the body containing texture reads and loops;
KILL is used for discarded output, not as an assumed early return. L:661–670 defines discard and states
that execution may continue. A warm-up span draw puts its fragment outside the glyph's bounds, so that
body is guarded off while the program is compiled. Shader upload continuation retains G4's one-page
commands, grouped into the encoder's existing bounded submission chunks. The encoder packs commands from successive programs into bounded chunks; neither program nor continuation boundaries require a separate fence. This keeps the P:273–282 offsets
and order while avoiding a synchronous device round trip for each page. Under ruling 33 (#1725) the
catalogue warms asynchronously on the GPU worker, one pipeline per step: a step creates that pipeline's
programs, unless a submission already did, and draws once with them; its chunks complete before the next
step is sent. A submission that draws with a pipeline whose programs no step has created yet creates them
in its own chunks, ahead of the draw. Encoding happens outside the core lock in both cases.

The suite's coverage expectation uses the frozen `breengel-cpu-393` rasterizer in `libs/libgfx-ref`.
Its rasterizer, float helpers, work counter and error definitions are byte-identical to that tag's
`libs/libfont/src/` files. Adapters use the existing LineSegment structure; they add no raster algorithm.
The rendering input is flattened font geometry and sorted crossings; the expected coverage is consumed
only by the suite's comparison and is not uploaded. Under ruling 30 (#1719) the four files are listed in
`scripts/frozen-copies.json` with their source tag and SHA-256. Claim-lint and `git diff --check` skip exactly
those files, and `scripts/check-frozen-copies.py` fails if their bytes change. The suite's expectation
modules include them directly (ruling 32); no crate wraps them.

## B13: G5 shader-text grammar

Experiment source revision: `819e749b52e8fb616b86a9f24c80659e36891db2` (the included catalogue and harness).

The experiment is `tools/gpu-experiments/g5-grammar.py`, through B10's public API harness and the
unmodified renderer at P's pin, with ANGLE's Metal backend. Reproduce with
`QEMU_GL_FRAMEWORKS="$FRAMEWORKS" DYLD_LIBRARY_PATH="$FRAMEWORKS" DYLD_FRAMEWORK_PATH="$FRAMEWORKS" taskpolicy -b python3 "$CHECKOUT/tools/gpu-experiments/g3-egl.py" "$CHECKOUT/tools/gpu-experiments/g5-grammar.py"`.
CHECKOUT is absolute; FRAMEWORKS is the pinned GL bundle's Frameworks directory. Each candidate is sent
to a fresh renderer context with its text length, terminator, fragment stage and token capacity. These
are grammar observations, not pixel correctness or performance evidence.

The small baseline's exact text is in the script: FRAG, COLOR output, CONST[0], TEMP[0], MOV and END,
capacity 64. The conditional baseline replaces its MOV with UIF CONST[0].xxxx, MOV, ENDIF. The sampler
baseline adds one SAMP and one typed 2D SVIEW declaration to the small baseline.

| Candidate or single change from its named baseline | Returned output |
|---|---|
| Small baseline | `G5 grammar baseline ACCEPT 0` |
| Add KILL before END | `G5 unconditional discard ACCEPT 0` |
| Replace MOV with F2U | `G5 float-to-unsigned ACCEPT 0` |
| Conditional baseline | `G5 conditional baseline ACCEPT 0` |
| Add ELSE and KILL before that baseline's ENDIF | `G5 discard alternative ACCEPT 0` |
| Sampler baseline | `G5 sampler baseline ACCEPT 0` |
| Change its two declarations' ranges from [0] to [0..2] | `G5 sampler ranges ACCEPT 0` |
| Generated catalogue candidates 2 through 15, each using its generated capacity | `G5 pipeline 2 ACCEPT 0` through `G5 pipeline 15 ACCEPT 0`, one result for each candidate |

The committed experiment reads the kernel's included text and capacities rather than maintaining a
second copy. An accepted candidate still has to pass its GPU suite readbacks; a response alone does not
establish that its pixels were written.


## B14: sampler declarations at draw time

Experiment source revision: `3211c7a83563ea8be041a0b3cb1b7e84d8bc195e`.

B13 establishes parser acceptance only. The first G5 device boot exposed a shader compilation error
when a range declaration was used with a fetch from sampler 0. The independent reproduction is
`tools/gpu-experiments/g5-samplers.py`, run with B13's invocation and environment, replacing
`g5-grammar.py` with `g5-samplers.py`. It calls the same unmodified public renderer API at P's pin.

The script uploads the one BGRA texel (17,34,51,68), builds a one-pixel target and a six-vertex quad,
and draws a minimal fragment program consisting of COLOR, SAMP/SVIEW declarations, an integer-0
coordinate, TXF of sampler 0 and END, capacity 64. The first candidate declares SAMP[0..2] and
SVIEW[0..2]. The second changes just those ranges to individual declarations for 0, 1 and 2. Both
SUBMIT and transfer calls return 0. The first produces pixel 00000000, with the compiler diagnostic
`'fssamp0' : undeclared identifier`; the second produces the fixture's exact BGRA word:

```text
G5 range submit=0 transfer=0 pixel=00000000
G5 individual submit=0 transfer=0 pixel=44332211
```

The core emits the individual declarations, each using T's declaration and range tokens. The added
four declarations add ten tokens. This is a text-form/device observation, not an implementation-source
explanation for the renderer's behavior and not a waiver of the required pixels.


## B15: box-filter source bounds

Experiment source revision: `c159c73c32c74becd52ae713bf38a869b6455fa5`; the baseline below is a separate input.

The contract's outside-source discard rule applies to the complete box-filter bin. Its last
texel must be inside the texture before the shader enters the fetch loop. The generator
compares the exclusive bin end with the texture dimensions, using L's USLT, OR and USEQ,
and guards the loop as it guards other outside-source fragments. This is integer geometry;
it does not change the bin means or pixel expectations.

`tools/gpu-experiments/g5-box-bounds.py` reproduces the defect and repair through B13's
public API harness, renderer pin, environment and invocation (substitute this script's name).
The baseline is the included box-filter text at `3211c7a8`; the repaired candidate is the
checkout's included text. Both use token capacity 10,000, no other differing packet fields.
A three-by-one BGRA texture contains three copies of (17,34,51,255). A one-pixel destination
is cleared to BGRA (25,39,49,255). The draw maps source origin (1,0), extent (3,1) to that
pixel, so the bin's exclusive x end is 4, past texture width 3. Submission and readback
return 0 for both candidates. The baseline changes the pixel; the guarded program leaves
the clear colour intact:

```text
G5 box baseline submit=0 transfer=0 pixel=ff22170b
G5 box bounded submit=0 transfer=0 pixel=ff312719
```

## G6 Breenix values

| Value | Source |
|---|---|
| A handle is SplitMix64's finalizer over the handle key, the process id and a sequence; values whose top byte is 0x00 or 0xFF are skipped, and the sequence is the core's rather than the process's, so an exec'd image draws handles its earlier image did not hold | O: `docs/gui/gpu.md` section 1.16, no addresses; A: SplitMix64, as above |
| Reclamation runs at the top of `handle_thread_exit` when no other row of the thread group is live, at the top of `exit_process_and_retire` under the same test, and after a successful exec returns from the process manager; the once-a-second sweep reclaims a thread group only when no row of it is live | O: `docs/gui/gpu.md` section 1.16, reclamation |
| `gpu_contexts`, `gpu_resources` and `gpu_resource_bytes` are signed deltas added to CPU 0's slot of their trace counters, at each insertion into and removal from the core's tables and at each charge and release | O: `docs/gui/gpu.md` section 1.16, global GPU counters |
| A process's quota keeps two counts: the bytes charged, which the 192 MiB limit applies to, and the share of them that is `SUBMIT`'s copies; resource backing is released with its record, staging with its call, the stream copy and snapshot when `SUBMIT` returns, the translated chunks when the submission retires, and vertex storage with its buffer; frames are charged whole, vertex storage by its byte size | O: `docs/gui/gpu.md` section 1.16 and `docs/gui/gpu-abi.md` `SUBMIT` and `COUNTERS` (`copy_bytes` at offset 136) |
| `gpu_irq_masked_max_us`: `without_interrupts` and `IrqSafeMutex` read CNTVCT_EL0 after masking and after restoring when the saved DAIF.I (bit 7) was clear and the current thread carries the GPU-path mark; the GPU interrupt handler reads it at entry and before return; ticks become microseconds by CNTFRQ_EL0, raised into CPU 0's slot with `fetch_max`; `DISARM_ALL` stores 0 | O: `docs/gui/gpu.md` section 1.16; A: Arm Architecture Reference Manual, DAIF, CNTVCT_EL0 and CNTFRQ_EL0 |
| Point 2 holds IRQs masked for 2,000,000 ns in `SUBMIT`, after the snapshot and before validation; point 3 writes `BREENGEL-EMIT-DIAGNOSTIC-PAYLOAD` and a newline through `serial_println!` at the start of syscall 416 | O: `docs/gui/gpu-abi.md`, test-control points; BreenGL's contract, section 21 |
| A request nobody waits for that names objects of an earlier reset epoch is queued in that epoch, so the confirmed reset retires it | O: `docs/gui/gpu.md` section 1.4.1, the device reset |
| Coverage tables: per data buffer, the bytes of each distinct table, `4*(8*height+1) + 8*entries`, are summed and compared with the largest `data_bytes` named for that buffer | O: `docs/gui/gpu-abi.md`, G5 parameterized draws, as G6 amends it |
| Snapshot ranges, sorted by buffer and offset and disjoint, and a submission's view of the process's resources, in handle order, are searched by bisection | O: `docs/gui/gpu.md` section 1.16, bounded validation |

## G7 display values

| Value or design | Source |
|---|---|
| MODES 256-byte record, two 16-byte modes, SET_MODE 16-byte input; 1280x800 scale 1 and 2560x1600 scale 2; default index 1 | O: `docs/gui/gpu.md` section 1.7 and contract PR #1738, `gpu-abi.md` G7 display interface |
| Display stack of four userspace owners, root direct take-over, EBUSY 16 for overflow or a cycle, EPERM 1 for a foreign give-back; saved mode and last binding; idempotent current-owner take | O: `docs/gui/gpu.md` section 1.7, its reference to BreenGL section 18, and contract PR #1738 |
| Current-scanout READBACK target 0 for the owner; handle 0 outside ownership and explicit foreign handles remain EINVAL; observation uses that resource’s completed rendering context | O: contract PR #1738; the transfer, attachment and complementary-poison layouts retain the V/U/B sources above |
| Points 25 and 26: omitted flush leaves binding unknown; wrong-target bind chooses the first-created other live scanout target, and completion records that command's actual resource | O: `docs/gui/gpu.md` section 1.8 and contract PR #1738; SET_SCANOUT and RESOURCE_FLUSH retain V section 5.7.6.8's fields |
| GPU console target 2560x1600 BGRA; font atlas of printable ASCII 32 through 126, 16 rows, R8 coverage, 256-byte transfer pitch; source bytes counted as fixture bytes | O: `gpu.md` section 1.7, built-in font tables; Breenix's existing Noto Sans Mono bitmap dependency, `get_raster` at Regular/Size16; atlas arrangement is core bookkeeping |
| Header `Breenix GPU console` at physical (16,16), background 0xFF181512 and foreground 0xFFE8EDF2, drawn offscreen once so the panic screen's target has a completed write; 128 KiB console vertex buffer | O: contract PR #1738 for the header; the buffer size is a kernel-console sizing choice. The GPU console is the panic path only (ruling F25): it is not bound as the kernel's screen |
| Glyphs use the existing bitmap_text coverage-table variant, with destination copies and integer shader blending; the full target is cleared by the existing solid pipeline | O: `gpu.md` sections 1.3 and 1.5; the existing G5 program derivations and wire values above, with no new shader text or grammar |
| FNV-1a offset basis 14695981039346656037 and multiplier 1099511628211 over retained legacy framebuffer memory; direct framebuffer writers retire under the framebuffer lock before the kernel screen's first scanout bind, and the handoff is the first completed bind of that screen | U: [RFC 9923 sections 2 and 5](https://www.rfc-editor.org/rfc/rfc9923.html) for FNV-1a and its 64-bit constants; O: contract PR #1738 for the observation definition; the framebuffer retains its memory address at initialization, and the observation uses volatile reads of that memory |
| Display transition serialization covers takeover, give-back, restore and the kernel screen's presents; owner sequence detects an exit that changes the destination during a restore | O: core concurrency bookkeeping for `gpu.md` section 1.7 |
| Kernel screen (`screen.rs`, ruling F25): one BGRA scanout target of the kernel mode's size (index 1, 2560x1600), pitch 10,240 bytes, backed by frames reserved once; the screen console writes its rows there, and the core uploads them with TRANSFER_TO_HOST_3D (box x 0, rows y..y+h, width 2560, offset y times the pitch), binds the target with SET_SCANOUT when it is not the known binding, and flushes the rows with RESOURCE_FLUSH (x 0, y, width 2560, h) | V section 5.7.6.8 for SET_SCANOUT's and RESOURCE_FLUSH's rectangles and the transfer's box, offset and stride; O: `docs/console/contract.md` section 8 for what the console reads back; the mode is section 1.7's |
| Kernel screen read-back: the backing filled with 0x3C5AA5C3 (the core's first observation poison), then TRANSFER_FROM_HOST_3D of the bound target in bands of at most 8 MiB, invalidated for the CPU before it is read | O: section 1.8's poison and band size, reused; V section 5.7.6.8 for the transfer |
| A framebuffer display owner's pane (`fb_mmap`) is copied into that owner's private target backing at each flush and presented as that owner's frame: SET_SCANOUT and a fenced RESOURCE_FLUSH counted as one of its presents, so the ownership stack keeps it as the owner's last binding | O: `docs/console/contract.md` section 4.2, the framebuffer path; section 1.7's ownership stack |

## B16: G7 scanout and console device observations

Commit `db251d0bbbe64274e34c17c30d635e329c327dd6`, Vigil run `20261017T232302Z-qemu`,
used the section 1.6.1 GL QEMU and its pinned renderer. Build ARM64 userspace, the production kernel
and disk first, with `CARGO_BUILD_JOBS=4` and builds under `taskpolicy -b`; the actual boot invocation was
`BREENIX_QUIET=0 "$CHECKOUT/scripts/boot-interactive.sh" --mode suite --suite gpu --gpu-gl --display --no-build --serial-log "$RUN/serial.log" < /dev/null`.
CHECKOUT and RUN denote absolute checkout and private run directories. The runner retained
`host-renderer.json` beside serial and accepted the ANGLE Metal hardware renderer. The full producer
reported `SUITE gpu DONE passed=158 failed=12 skipped=0 total=170`: present 16/16, three earlier-category
failures, and nine unwritten G8 performance checks. The earlier failures and the handle-0 correction are
tracked in #1739; this run does not establish the earlier categories as green.

| Committed input/check | Producer output excerpt and established fact |
|---|---|
| `mode_1280x800`, `mode_2560x1600`: BGRA full-screen targets, clear 0xFFD15321, one-pixel left/right/bottom edges and a 2x2 centre in 0xFF25B3E7; normal and target-0 observations compare the full canvas and resource identity | `SUITE gpu CASE present/mode-1280x800 PASS`; `SUITE gpu CASE present/mode-2560x1600 PASS`. The device accepts both full-resource scanout/flush sequences and the observed bytes equal those scenes. |
| `flip`: two 1280x800 targets, the patterned scene and a 0xFF25B3E7 clear, presented A/B/A; `mode_change_content` changes 1280/2560/1280 with full observations | `SUITE gpu CASE present/flip PASS`; `SUITE gpu CASE present/mode-change-content PASS`. Each completed binding names the selected resource, with increasing context fences, and the readback canvases match. |
| `takeover_giveback` and `ownership_stack`: 73x17 and 31x9 patterned targets; distinct forked owners; saved mode 0 restored after mode 1; four owners, fifth EBUSY | `SUITE gpu CASE present/takeover-giveback PASS`; `SUITE gpu CASE present/ownership-stack PASS`. Give-back restores the previous binding's bytes and mode, and nested owners restore in order. |
| `console_restored` (as measured at that commit, before ruling F25 made the kernel's screen the screen console's): user frame, give-back, retake before presenting; current-scanout transfer of the 19-character header compared channel-by-channel with the independent built-in font coverage formula | `SUITE gpu CASE present/console-restored PASS`. The observed resource is the private console target and its glyph pixels match; completed bitmap draws and fixture transfers are reported. |
| `firmware_handoff` (as measured at that commit): compare retained firmware memory's hash at handoff and after GPU console restore, and observe the console header again | `SUITE gpu VALUE present/firmware-handoff handoff=1 expect=1..1`; `SUITE gpu CASE present/firmware-handoff PASS`. The legacy memory is unchanged during these console transitions. This measures those transitions, not programs using the legacy framebuffer API. |
| Point 25 omits a flush; point 26 binds the other live 47x13 scanout target, whose colour differs | `SUITE gpu CASE present/fault-skip-flush PASS`; `SUITE gpu CASE present/fault-wrong-scanout PASS`. An omitted flush returns EIO and binding-unknown without a completion; the wrong bind is distinguished by its actual resource, pixels and the requested target's offscreen flag. Both recover on the next ordinary present. |

The handle-0 correction was checked at `85891e63711db85e6fa639b7d63f4df199df9b76`
with the same ARM64 disk and this boot invocation:
`BREENIX_QUIET=0 "$CHECKOUT/scripts/boot-interactive.sh" --mode suite --suite gpu --case isolation/handle-guess --gpu-gl --display --no-build --serial-log "$RUN/serial.log" < /dev/null`.
It produced `SUITE gpu VALUE isolation/handle-guess guesses=615`,
`SUITE gpu CASE isolation/handle-guess PASS ms=2108` and
`SUITE gpu DONE passed=1 failed=0 skipped=0 total=1 only=isolation/handle-guess`.
The host renderer attestation was accepted. This was a focused check of the changed errno;
the two inherited pre-G7 expectations above remain unchanged and need a ruling.

## G8 Breenix values

| Value or design | Source |
|---|---|
| Readback bands of 8 MiB of the target's backing: a 2560x1600 B8G8R8A8 readback is two bands, each transferred under both poisons | O: `docs/gui/gpu.md` section 1.8's sequence, unchanged; the band size is a sizing choice |
| The poison fill stores eight bytes at a time once at a pattern boundary; the first observation is read into the staging frames through `observation::read` and the second compared with them eight bytes at a time; pixels go to the caller straight from the staging frames | O: section 1.8; copy granularity is core bookkeeping |
| The control queue's doorbell is written after the core's lock is released, by the thread whose call queued requests; a waiter takes the device's answers but leaves queued requests to `kgpu`; the interrupt wakes the waiters and `kgpu`, and a returned chunk of a submission still in progress notifies no one further | O: section 1.4.1 and the `performance` bounds of section 1.10; scheduling is core bookkeeping |

## B17: G8 performance measurements

The three runs below used the section 1.6.1 GL QEMU with its pinned renderer, built ARM64 userspace, the
production kernel and the disk first with `CARGO_BUILD_JOBS=4 BREENIX_BUILD_BACKGROUND=1`, and kept
`host-renderer.json` beside the serial; the runner accepted the ANGLE Metal hardware renderer each time.
CHECKOUT and RUN denote absolute checkout and private run directories. Each excerpt is the producer's
`SUITE gpu VALUE` or `CASE` line, without its `expect=` suffix.

| Run | Commit, Vigil run and invocation | Producer output excerpt |
|---|---|---|
| Before G8's changes | `d9c7b5dd5b3199bace16fb91b791f2f7a5caa129` (the checks alone), `20261018T012257Z-qemu`; `"$CHECKOUT/scripts/boot-interactive.sh" --mode suite --suite gpu --gpu-gl --serial-log "$RUN/serial.log" < /dev/null` | `clears-per-second=379`, `submission-us=3595us`, `median-us=67us`, `max-us=1297us`, `slowest-us=2381us`, `readback-us=79999us`, `upload-us=32033us`, `coverage-us=2769us`, `presents-per-second=746`; `wait-us=52061us`, `cpu-us=49933us`, `CASE performance/wait-no-spin FAIL`; `DONE passed=169 failed=1` |
| Waiters woken only through `kgpu` | `95769078b414a89a5eab4a3ca840b98f7b72015d`, `20261018T030905Z-qemu`; as below | `CASE pipelines/coverage-spans-corpus FAIL ms=10000 msg=timed out after 10000 ms` (4709 ms before); `CASE isolation/quota-handles PASS ms=5414` (925 ms before); `CASE performance/first-draw-latency FAIL ms=1209 msg=pipeline 8's first draw in a warmed context took 12134 us`; `DONE passed=168 failed=2` |
| G8 | `63431ee9f3fc4f5e454425d230d4b83ee528eacf`, `20261018T041021Z-qemu`; `"$CHECKOUT/scripts/boot-interactive.sh" --mode suite --suite gpu --gpu-gl --display --serial-log "$RUN/serial.log" < /dev/null` | `clears-per-second=327`, `submission-us=4745us`, `median-us=79us`, `max-us=1352us`, `slowest-us=2744us`, `readback-us=73989us`, `upload-us=39545us`, `coverage-us=2799us`, `presents-per-second=728`; `wait-us=24720us`, `cpu-us=313us`; `CASE pipelines/coverage-spans-corpus PASS ms=4962`; `CASE isolation/quota-handles PASS ms=1142`; `DONE passed=170 failed=0 skipped=0 total=170` |

The facts these establish, for this host and renderer:

- A GPU wait whose loop read the device's status register and rang the queue's doorbell used 49,933 µs of
  CPU time in a 52,061 µs wait; with neither access in the wait path, 313 µs in a 24,720 µs wait. The
  waiter's CPU time followed those MMIO accesses, not the length of the wait.
- Delivering each device answer to a waiter through `kgpu` instead of from the interrupt made
  synchronous-request-heavy cases several times slower (the second row), so the interrupt wakes waiters
  directly.
- A full 2560x1600 readback measured 79,999 µs before and 73,989 µs after, against the 100 ms bound.


## Parallels catch-up values

The historical #1875 per-step warm-up timings cited in the catalogue derivation
used uncommitted timing diagnostics. Those historical timings are unverified under
ruling 12 and are not reproducible from the recorded revision.

| Value or design | Source |
|---|---|
| Variable shifts that can reach 32 take the count's low five bits with an explicit `AND` with 31 first (45 masked shifts across 39 generated programs): the division loop's two shifts by its bit counter (counts 95 to 0) in `tools/gpu-profile/math.rs`, each variable `SHL` and `USHR` the model's emitter writes, and `bitmap_text`'s bit-row shift in `tools/gpu-profile/pipelines.rs`. The eight shifts by `CONST[6].w` in pipeline 8 are unmasked because their count is at most 14 | L: SHL 1283 and USHR 1313 define the shift by the count's low five bits, so the `AND` changes no result where that definition holds; B18 shows a host renderer where it does not |
| Parallels' virtio-gpu interrupts through MSI-X: vector 0 for the control queue and vector 1 for configuration changes, each an edge-triggered SPI from the GICv2m frame; the driver does not read the ISR byte for the queue vector | VirtIO 1.3 sections 4.1.4.3, 4.1.4.5 and 4.1.5.1.2; the Arm GICv2m frame's MSI_SETSPI_NS register at offset 0x40 |
| In the 39 generated programs an instruction with only immediate sources (a word copy `UADD x, k, 0`, or a select between two literals lowered to `XOR`) takes `CONST[4].z` in place of its immediate 0; the draws of these programs supply that word as 0 (`kernel/src/gpu/virgl/encode.rs`: catalogue draws pass `CONST[4]` as `0, 0, 0, 0`, PROBE passes `word, routine, 0, 0`) | B19; the operation and its result are unchanged where `CONST[4].z` is 0 |

The programs were regenerated with the commands in "G4 shader derivation" and "G5 parameterized
catalogue". The model tests exercise arithmetic routines; the emitted-ISA tests exercise
profile routines and catalogue triangle 15, rather than each regenerated catalogue program.
The committed-catalogue comparison checks generated text and token counts for pipelines 2 through 15.
The generator at the previous commit reproduced the committed programs byte for byte before the change.

## B18: Parallels variable shift counts

A Linux guest on the same Parallels configuration ran `tools/gpu-experiments/parallels-shift-count.c`
(commit `f856f059`) through its own GLES driver (Mesa 25.2.8's virgl driver, as a library through EGL
and GLES 3.0): `gcc -O2 -o psc parallels-shift-count.c -lgbm -lEGL -lGLESv2 && ./psc`. The fragment
program shifts 0x80000001 by eight counts read from a uniform array and also by the count ANDed with 31.
The Gallium trace of the same run (`GALLIUM_TRACE`, shader cache disabled) shows the driver passed one
`ushr` and one `ishl` by the uniform count: it folded the `& 31u` variants into them, as its shifts are
defined on the low five bits, so the host received a bare variable count.

| Count | Right shift read back | Left shift read back | TGSI's definition |
|---|---|---|---|
| 0, 1, 31 | `0x80000001`, `0x40000000`, `0x00000001` | `0x80000001`, `0x00000002`, `0x80000000` | equal |
| 32, 33, 63, 64, 95 | `0x00000000` | `0x00000000` | `value >> (count & 31)` and `value << (count & 31)`, for example `0x40000000` and `0x00000002` at 33 |

The program printed `SHIFT COUNT NOT MASKED TO FIVE BITS`. On this host renderer a shift by a register
count of 32 or more gives 0, where TGSI defines the shift by the count's low five bits. The ARM64 QEMU
renderer follows TGSI's definition: the G5 programs, which relied on it, pass there.

## B19: Parallels folding of literal-only instructions

The original diagnostic shader and watchdog patch were not committed; this experiment
is not reproducible from the recorded revision and remains unverified in #1871.
The corrected command below describes the historical invocation shape, not a
reconstruction with the current per-routine checkpoint tool.

On Parallels, `profile/binary32-emulation` and the three `coverage_spans` cases read wrong pixels while
the integer routines passed; the probe program's binary32 routines returned, for example, `0x3f7ffffb`
for 1.0 + 2^-24 (want `0x3f800000`) and 1 for 3 × 0.5 (want 2). Each of these failures was a run of the
gpu suite at `9d8cebd1` with the kernel's diagnostic changes not committed: `probe_int` warmed with the
catalogue and an 8 s watchdog so that the first PROBE's 3.1 s compilation (section "Parallels catch-up
values") did not end the case.

A run with a twelve-instruction diagnostic program in place of `probe_int` (XOR of a constant with
`0x80000000` and `0x7f800000`, moves of `0x80000000`, `0xffffffff` and `0x7fc00001`, UADD and USLT
with `0x80000000`, AND with `0xffffffff`) returned 12 of 12 words exactly, so the host keeps those bit
patterns through registers and constant operands.

`tools/gpu-experiments/probe-checkpoints.py "$PROFILE" 600 1194 "$OUT"`, with PROFILE the probe program
at `9d8cebd1`, gave a diagnostic program of 64 checkpoints in routine 12 (binary32 add). A 64-pixel PROBE
row of routine 12 with operands `0x3f800000, 0x33800000` was read back on Parallels and compared with
the generator's interpreter on the same text:

| Checkpoint | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 |
|---|---|---|---|---|---|---|---|---|---|
| Interpreter | `3f800000` | `c000000` | `ffffffff` | `0` | `800000` | `ffffff99` | `1000000` | `1` | `fffffffc` |
| Parallels | `3f800000` | `c000000` | `ffffffff` | `0` | `7ffffd` | `ffffff99` | `1000000` | `1` | `fffffffd` |

Between checkpoints 3 and 4 the program selects between two literals: `XOR TEMP[44].x, IMM[62].zzzz,
IMM[62].yyyy` (`0x00800000` and 0), `AND` with the condition mask, then `XOR` with 0. Checkpoint 4 read
`0x007ffffd`. Of the 11 generated programs with literal-only instructions, the 2 that failed on Parallels (program 8
and `probe_int`) had a normal binary32 bit pattern among those literals (`0x00800000`, `0x3f800000`); the
other 9 used only 0 and small integers there and passed. The generator now gives no instruction only immediate sources. Attributing
the deviation to the host compiler's constant folding is an inference from these observations, as B12's
was for ARM64 QEMU's renderer.

## B20: Parallels virtio-gpu BAR0 memory

The Breenix-side hash observation below used uncommitted diagnostics and cannot be
reproduced from its recorded revision; it is historical, unverified evidence under ruling 12.

On Parallels the UEFI framebuffer that `present/firmware-handoff` hashes is BAR0 of the virtio-gpu
device itself (`1af4:1050`, 128 MiB at `0x10000000`; the GOP mode used its first 3 MiB). In a one-case run
of that case (`run.sh --parallels --suite gpu --case present/firmware-handoff` at `9d8cebd1` with
diagnostic prints not committed), the hash at the console's handoff was `0x7319e2a2077647ed`, and after
the GPU console's first present the whole region read 0 (hash `0x3fe01b4acfe22325`, also read after
a device reset in another run), with no device reset between.

A Linux guest on the same configuration ran `tools/gpu-experiments/parallels-vram-scanout.py`
(commit `74b9252e`) as root, first with the command `true` (no change of scanout), then with
`scripts/parallels/egl_green_test.c`. Its output:

```text
written: first=kept last=kept first-16=030a11181f262d343b424950575e656c
after 1 s: first=changed last=changed first-16=01010100010101000101010001010100
command exit 0
after the 3D scanout: first=changed last=changed first-16=01010100010101000101010001010100
```

The second run printed the same three lines and `command exit 1` (the program did not take the
display). The pattern written into BAR0 from the guest CPU was replaced within one second without any
guest action: while the virtio-gpu drives the display, the device rewrites its BAR0 memory. A hash of
that memory changes without any guest write, so on Parallels it cannot show whether the kernel wrote
the firmware framebuffer after the handoff.

## Parallels attestation negative control

Run `20261018T114114Z-parallels` at `af4fc2f3`, with
`BREENIX_PARALLELS_3D=off` and `run.sh --parallels --suite gpu`, finished
with 0 passed, 170 failed and 0 skipped. Admission refused `missing 3d-feature`,
and the host judge refused the disabled 3D configuration before measuring its rate.
This checks admission, not the ten-times-idle rate against a negative device run.
Under ruling 51, an idle interval measuring 0 ns GPU time requires only positive performance GPU time;
that positive-time rule has no stronger measured negative-control claim here.

## CU-1749 terminal display design

| Value or design | Source |
|---|---|
| Reserve 18 descriptors at the end of the control queue; retain interrupted normal chains and use the published available index after stopping normal producers | V sections 2.7.5, 2.7.13 and 2.7.14; O: `encode::CHUNK_WORDS` is 16,384 words, hence 16 payload pages plus a header and response descriptor. The reservation is excluded in `queue::Queue::reset`. |
| Private context, target, glyph atlas and preallocated bitmap_text commands; patch only each cell's atlas x coordinate before submission | O: ruling 40 (#1749), the G7 console and the catalogue encoder in `kernel/src/gpu/virgl/encode.rs`. The layout is 80 columns by 32 rows, scale 2, with 40/32 pixel margins and 46-pixel line spacing. Its 2,560 glyph quads occupy 122,880 bytes of the console's 128 KiB vertex buffer. Background 0xFF6E1216 and foreground 0xFFFFEBEB follow the diagnostics screen's colours. |
| Terminal latch, per-CPU producer counts, a 100 ms producer-drain deadline and a shared 2 s device deadline covering pending normal commands, terminal draws and observation; invalidate preparation at reset and retain DMA storage after timeout | O: ruling 40; the drain bound and screen layout are terminal-path policy. The device bound uses the contract's 2 s watchdog interval. A producer abandoned on the panicking CPU is excluded from the drain; its descriptors are disjoint from the reservation. |
| Mask the GPU's level SPI in hard IRQ context; `kgpu` reads the ISR byte with IRQs enabled and unmasks the SPI without changing its route or clearing a pending level. Route the SPI explicitly to CPU 0 and enable it after publishing the worker | V section 4.1.4.5: the ISR read acknowledges INTx; A: Arm DAI0492 version 3.0, “Configuring the Arm CoreLink GIC”, Enable registers (GICD_ICENABLER and GICD_ISENABLER), [Arm GICv3/v4 overview](https://developer.arm.com/-/media/Arm%20Developer%20Community/PDF/Learn%20the%20Architecture/GICv3_v4_overview.pdf?revision=65f91645-cd52-4795-952b-f01095ff5ef8). Deferral to the worker is Breenix policy under the IRQ budget in O section 1.16. |
| Direct IRQ wakes use a try-lock and a budget of `limits::IN_FLIGHT` waiters; completion publication wakes the rest in task context | O: section 1.4.1's four-submission limit supplies the wake budget. This preserves direct wakeups while bounding the IRQ's queue work; it does not change fence completion or wait deadlines. |
| The soft-lockup detector recognizes the terminal GPU latch as an intentional end of scheduler progress | O: `docs/gui/gpu.md` section 1.7.1, created by PR #1964 for #1749; the detector retains its ordinary behavior before that latch. |
| In the capture-panic test build, confirm scanout binding and flush, then fenced TRANSFER_FROM_HOST_3D twice under complementary poisons; retain the first observation in frames for post-mortem inspection | V section 5.7.6; O: `docs/gui/gpu.md` section 1.8 and the existing 3D transfer layout recorded above. The terminal requests currently reserve their fence identifiers during preparation; device acceptance after later normal requests remains unvalidated (review m5). Normal builds omit the observation buffers and the private target's guest backing. |

| Panic preparation retries at most three times per device epoch, resetting the count on epoch change | O: bounded preparation policy for #2033; three is a Breenix retry budget, independent of device wire values or external driver implementations. |

## Object creation follow-up to PR #2111

| Value or design | Source |
|---|---|
| `WARM_QUIET_NS = 2_000_000` ns; defer a catalogue step for send/answer quiet, bounded by the existing `WARM_GRACE_NS = 20_000_000` ns; traffic after a step starts also spaces the next start by that grace | O: Breenix scheduling design for #2103 and the follow-up review of #2111. The 2 ms quiet interval is a Breenix policy choice, not a device requirement or an externally derived constant. The 20 ms grace is the existing bounded warm-up policy of ruling 33 (#1725). |
| Unanswered batched attaches to a quarantined, reset, dropped or removed context finish with EIO and abandon their answers, including attaches not yet sent | O: `docs/gui/gpu.md` section 1.4.1 requires observing quarantine within 250 ms; abandonment retains device-access holds until completion or confirmed reset. |

Source declaration: this change derives from Breenix's own GPU core, its contract,
and PR #2111's review findings. No external implementation source was used.
