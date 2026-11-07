# SCRCRC01 version 1

This is the backend-6 driver's binary checkpoint record for the native sidecar
specified in `scripts/screens/README.md`, Native run evidence. It contains the
per-checkpoint fields. The boot exporter supplies the enclosing `run`, `commit`
and sidecar `version` from the retained boot. Transport and exporter wiring are
subsequent E22 units.

The driver entry is `gk208::display_crc::observe`. Its caller must hold the shared
display observation transition across the pixel observation and this call, using
the same resource, present fence and current mode. That transition excludes
another presentation or mode change. The native owner retains the resource and
checks the head's active and assembly surface state during collection. The
interface is suite-boot-only; authorization belongs to the screen-evidence device.
ARM's observation boundary remains latched-binding readback and host-renderer
attestation (`screen-proof.md`), with no equivalent virtual display CRC.

## Encoding

Integers are little endian, with no struct padding or alignment between fields.
The fixed header is 116 bytes. The total record is less than 4096 bytes. Source
identifiers are the captured SOR and RG indices, rather than values derived from
CRC samples or a host summary.

| Offset | Type | Field |
|---|---|---|
| 0 | 8 bytes | `SCRCRC01` |
| 8 | u32 | Record version, 1 |
| 12 | u32 | Total byte length |
| 16 | u32 | Backend, 6 |
| 20 | u16 | Suite byte length |
| 22 | u16 | Case byte length |
| 24 | u16 | Checkpoint byte length |
| 26 | u16 | Method/value pair count |
| 28 | u16 | Sample count, including the discarded first entry |
| 30 | u16 | Disarmed, 1 |
| 32 | u64, u64 | Binding before: resource, present fence |
| 48 | u64, u64 | Binding after: resource, present fence |
| 64 | u32 | Elapsed microseconds |
| 68 | u32 | Display error |
| 72 | u32 | Context status |
| 76 | u32, u32 | Mode width, height |
| 84 | u32, u32 | Raster width, height |
| 92 | four u32 | Viewport x, y, width, height |
| 108 | u32 | SOR index: `out=sorN` and primary `sources[0]=sorN` |
| 112 | u32 | RG index: secondary `sources[1]=rgN` |
| 116 | variable | Suite, case, checkpoint UTF-8 bytes, in that order, without NUL |
| following | pairs of u32 | Method address, latched value |
| following | triples of u32 | Normalized written status, primary CRC, secondary CRC |

Method addresses are aligned, sorted and unique. Values come from the active
bank, with a matching assembly bank required before arm and after disarm. The
inventory covers output routing, raster timing, surface geometry/storage,
viewport, scaler, color/LUT and dithering settings; it excludes the resource's
address and the CRC notifier controls. Mode and raster dimensions become `WxH`
strings in the sidecar. CRC words become eight-digit lowercase hexadecimal.
These conversions do not compute CRCs or replace method values.

## Collection and refusal

The CRC notifier is the existing guarded 4 KiB `dma` display allocation. Handle
4 refers to a separate physical VRAM descriptor installed while the display
instance is inactive. Ordinary present, retained acknowledgement and terminal
notification use the existing `notifier` allocation, with no overlapping slots.

The production collector clears the CRC page, arms context DMA then primary SOR
and secondary RG through a notifier-scoped core UPDATE, and collects five
consecutive nonzero output pairs. The first is retained but discarded from
stability; the following four must agree. After disarm (disabled output selectors, context
DMA 0, fresh UPDATE), it waits for context completion and reads the completed
notifier tail. A changed tail refuses the observation too. The 255-entry layout
allows at most 254 entries after the discarded first.

The collector refuses an overflow, changed binding or output configuration,
display error, partial secondary output, unstable pair or observation exceeding
250000 microseconds. Entry status is normalized to 1 only for written output;
the undocumented hardware entry-status and compositor word are not sidecar
fields. The context status is retained directly, with bit 0 required and mask
0x3e refused. No partial record is returned as a successful observation.

Cleanup attempts disarm after preflight, clear, arm and collection errors on a
valid route. An invalid route is rejected before I/O. A failed arm or disarm
also invokes the existing synchronous present path for the pinned surface; a
failed recovery remains an error. Core waits and recovery retain finite bounds.
A failed observation may exceed its capture budget while attempting cleanup;
it cannot yield an eligible record.

Sources and MIT grants are listed in `THIRD_PARTY_NOTICES`. The source permits
programming the secondary RG field; whether this card populates it remains a
hardware acceptance question. A missing secondary refuses this producer rather
than manufacturing an RG value.
