# SCRCRC01 version 1

This is the backend-6 driver's binary checkpoint record for the native sidecar
specified in `scripts/screens/README.md`, Native run evidence. It contains the
per-checkpoint fields. The boot exporter supplies the enclosing `run`, `commit`
and sidecar `version` from the retained boot. The shared codec is
`gpu_abi::screen_crc`; the exporter belongs to a subsequent E22 unit.

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
and secondary RG through a notifier-scoped core UPDATE. Five written primary
entries bound the collection interval. After disarm (disabled output selectors,
context DMA 0, fresh UPDATE), it waits for context completion and re-reads the
complete notifier, as NVIDIA Display-CRC/display-crc.txt specifies. Only that
completed snapshot establishes the nonzero output pairs: the first is retained
but discarded from stability; the following four must agree. Written tail
entries must agree too. A secondary word missing after DONE still refuses the
observation; provisional primary reads are not approval evidence. The 255-entry layout
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

## Guest transport

<!-- claim-lint:ok: Request layout in libs/gpu-abi/src/screen.rs; codec and boundary tests in tests/fixtures/gt710_display_crc.rs and tests/screen_crc_endpoint_test.py. -->
The proof-only CRC variant uses the separate `SCRCRCRB` tag on
`/dev/screen-evidence`. The pixel-only 32-byte `SCREENRB` form is unchanged.
The CRC request has a 48-byte header followed by suite, case and checkpoint
UTF-8 tokens without NULs. Header fields, little endian:

| Offset | Field |
|---|---|
| 0 | Eight-byte `SCRCRCRB` tag |
| 8 | u64 pixel destination |
| 16 | u64 pixel buffer capacity |
| 24 | u64 ordinary 1024-byte observation destination |
| 32 | u64 CRC record destination, capacity 4096 bytes |
| 40 | Three u16 identity byte lengths: suite, case, checkpoint |
| 46 | u16 reserved, 0 |

<!-- claim-lint:ok: Normative request contract implemented by CrcRequest and observe_screen_crc; tests/screen_crc_endpoint_test.py checks the access boundary. -->
The record's own byte length delimits its output. Names are bounded to 255 bytes
each; suite is gpu or breengel, case is category/name, and checkpoint is a
lowercase alphanumeric hyphenated token. Partial, trailing and malformed requests
are refused. The endpoint requires root, a suite boot, proof mode and backend 6.
Both readback and CRC run before releasing the display transition and resource
read pin. CRC failure uses the observation destination's SCREENER status;
ordinary observation record fields are unchanged.

<!-- claim-lint:ok: Guest handoff and corruption tests in tests/suite_screen_test.rs; pinned native capture is in kernel/src/gpu/mod.rs read_back_impl. -->
The child writes the raw record to a per-case queue with a byte length and
SHA-256. The parent validates the digest, suite, case, binding stability and
collection status before its diagnostic PUT. Intent checkpoints use the same
request and queue from the parent. Native proof observations bypass the cached
pixel readback so the CRC and digest come from the same pinned observation.

Labels are `crc-<suite>-<16 lowercase hex digits>`; the suffix is the first eight
bytes of SHA-256 over `case`, one NUL byte, then `checkpoint`. The separator
cannot occur in either identity token. Each record is one W3 diagnostic request.
The serial anchor is:

```text
SCREEN_CRC_UPLOAD <suite> <case> <checkpoint> label=<label> bytes=<n> sha256=<64 hex> result=ok|failed:<reason>
```

The CRC SCREEN line uses the first stable sample (entry 1), raster and SOR index
from that record. The proc summary describes the last completed checkpoint,
including the first discarded entry in `entries`; before any completed capture
it returns errno 61. Outside suite boots it returns ENOSYS, and virtual backends
return ENODEV. The summary read clones the completed text without starting collection. ARM's boundary
remains latched readback and host-renderer attestation.

## Perturbation records

The native runner selects its first successful `end` CRC capture with screen_proof=on. Selection happens while the case's display owner is still alive,
including a helper's pre-drop observation. The host requires the sequence's
baseline to equal the selected ordinary `end` SCREEN and its approved native
row; the guest does not carry the approval table.

The four extra requests use the binary checkpoint identities `perturb-one`,
`perturb-two`, `perturb-three` and `perturb-four`. They use the same case and
label derivation as ordinary CRCs, yielding four distinct diagnostic labels.
The ordinary SCREEN inventory does not gain these private checkpoints.
The serial form is:

```text
SUITE <suite> SCREENPERTURB <case> end phase=<1-4> sha256=<digest> mode=<WxH> binding=<WxH> crc=<hex8> crc2=<hex8> raster=<WxH> out=sor<N> resource=<R> present_fence=<F> fired=<count> disarmed=<0|1>
```

Here `disarmed` describes point 35, whereas SCRCRC01's `disarmed` field
continues to describe the CRC collector. Phases 1 and 4 have point 35 disarmed;
phases 2 and 3 have it armed/fired. Phase 2 observes after arming and witnesses
one firing. Phase 3 re-observes the same binding. Phase 4 follows DISARM_ALL and
checks both CRCs and the saved output method list against phase 1.

The case clock's shared atomic claim survives fork and returns to the parent
before the next case. Point 35's boot-lifetime fired counter also prevents a
second successful sequence across suite-sequence exec. An unsuccessful
sequence emits `error=sequence-failed`, is not retried in that runner and fails
screen acceptance independently of CASE.

The phase lines use the hashed case text handoff; raw records use the existing
length/SHA-256 CRC queue. The parent validates their framing and identity before
emission/upload. The host exporter verifies phase receipts and bytes, puts
ordinary checkpoints in `screen-crc.json` and phases in `screen-perturb.json`.
This keeps perturbation observations out of the native approval import.
The receiver adds four diagnostic slots to the checkpoint inventory cap; the
2 MiB request and 64 MiB boot limits retain their values.

The U4 R240 acceptance entry point is:

```sh
python3 scripts/screen-proof-acceptance.py --profile r240 --commit <full-sha> \
  --console <console-export> --gpu <gpu-proof-export> \
  --breengel <breengel-proof-export> --output <report.json>
```

It requires distinct eligible boots at one kernel/build/tools commit, confirmed
power-off, full suite verdicts, memory PASS and full clocks. GPU and BreenGL must
agree with their host records on screen_proof=on, match the checkpoint inventory and
approved digests, and carry native rows for the uploaded output configurations.
The strict native comparison calls screen-verdict's native scorer per
checkpoint with the digest of that checkpoint's verified configuration; logical
mode changes can produce different configuration digests within a boot.
Missing, extra, reordered or repeated phases, changed memory pixels, an
unchanged phase-3 SOR CRC or an unrestored phase-4 CRC/configuration fail.
The default `arm` profile retains its previous companion requirements.
