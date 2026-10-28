# GT 710 graphics context acceptance (M7 PR 7.5)

This is the producer/scorer wire contract for `metal-gt710/gr-contexts`.
The `switch` and `controls` cases extend the existing draw gate; they do
not replace its shader-stage controls or presentation evidence. Hardware
acceptance requires the subsequent completed console-suite boot, its usual
producer stages and report, and handoff-to-done at or below 330 seconds.
This specification and its synthetic host captures are not boot evidence.

## Transport and complete images

Append the context image block to `gt710-pixels.rle`, transferred through the
existing network upload. Serial carries `GT710_CTX_BULK <bytes> <sha256>`
for this block and the existing overall `GT710_BULK` length/digest. These
anchors hash the newline-terminated ASCII bytes, not the expanded pixels.
The block must appear in the same order as its anchor among the other
artifact blocks. The scorer rejects context image rows on serial, absent
or repeated anchors, interleaving, mismatched lengths and mismatched digests.

Each row is:

```text
GT710_CTX_IMAGE <case> <poison> <y> <count:word,...>
```

`poison` is decimal 85 then 170. `y` is a decimal row index from 0 through
63. Runs use the present-image encoding: lowercase hexadecimal positive
counts and eight-digit words, expanded little-endian into BGRA bytes.
A row expands to 64 pixels; an image has 64 rows and 16,384 bytes, with
pitch 256 and no padding. Image order is the execution order below, each
with poison 85 then 170. The two complete CE transfers must agree, and
the SHA-256 over the decoded bytes must match their `readbacks` entries.
The producer poisons the transfer destination again before each transfer
and waits for the draw release before starting CE.

The eight-image block has a worst-case size of 753,760 ASCII bytes when
each pixel requires its own run. Adding the 7.4 budget of 1,326,749 bytes
gives 2,080,509 bytes, leaving 16,643 bytes below the unchanged 2 MiB
limit. JSON metadata stays on serial. This is a wire-size calculation,
exercised by `tests/gt710_contexts_report_test.py`, not a measured upload.
<!-- claim-lint:ok: normative bounds and synthetic worst-case calculation in tests/gt710_contexts_report_test.py. -->

## Serial metadata

`GT710_CONTEXTS` is one JSON object with:

- `schema: 1`, `class_id: 41367`, `error: null`;
- `current_register: 4233984` (`0x409b00`);
- `trap_registers: [4194560,4194568,4194576,4194584]`;
- `channels`, `frames`, and `readbacks` as specified below.

`channels` has two entries in `a`, `b` order, each containing `id`,
`instance`, `image`, `target`, `release`, `epoch`, `vertex_sha256` and
`fragment_sha256`. The four addresses are distinct allocation bases,
positive, 4 KiB aligned and below the 40-bit DMA limit, also distinct
across channels. `image` identifies private saved graphics state; `target`
identifies the private 64×64 allocation; `release` identifies the semaphore
storage. Retain these allocations through the final transfers and confirmed
drain. The epoch is a positive 64-bit native channel epoch.

Both channels use the committed `diagnostic-triangle-vertex.bin`. Channel
A uses `diagnostic-triangle-fragment.bin`; B uses `gradient-fragment.bin`.
The hashes must match `kernel/assets/kepler/manifest.json`. G5 applies:
the producer embeds the committed 7.1 assets and does not rebuild them on
the x86 build host. No proprietary shaders or firmware are admitted.

`frames` has these eight entries, in this order:

| `case` | `channel` | `variant` | Required result |
|---|---|---|---|
| `a-v1` | `a` | `v1` | Original triangle coverage, background and literal samples |
| `b-gradient` | `b` | `gradient` | Exact full integer gradient |
| `a-v2` | `a` | `v2` | Changed positions and colours, V2 coverage and literals |
| `b-restored` | `b` | `gradient` | Restore B and repeat its gradient without re-emitting its shader/target state |
| `suppress` | `a` | `v2` | Clear A, suppress its draw, reject the resulting background |
| `after-suppress` | `a` | `v2` | Immediate clean V2 successor |
| `corrupt` | `b` | `gradient` | Draw B, CE-upload the background word to the epoch-selected pixel before read-back, reject it |
| `after-corrupt` | `b` | `gradient` | Immediate clean full-gradient successor |
<!-- claim-lint:ok: normative eight-frame sequence in tests/gt710_contexts_report_test.py and graphics_contexts_gate. -->

The initial A/B/A/B sequence exercises FECS save/restore with distinct
private images and programs. Change A's vertices for V2 after returning to
A. Keep B's state resident in its saved context; reinitializing that state
before `b-restored` would defeat the restore probe. Targets begin cleared
to the oracle's zero background; B is also cleared before `b-restored`
and `corrupt` so those draws cannot pass using stale pixels. Clean images must come from A197 execution.
CE uploads initialize the background and vertex/program data, and the corrupt control uploads one
background word after the draw release; CE also performs fenced read-back.
<!-- claim-lint:ok: normative producer requirements for the eight-frame sequence in tests/gt710_contexts_report_test.py; not hardware claims. -->

Each frame contains `case`, `channel`, `variant`, `current_before`,
`current_after`, `trap_before`, `trap_after`, `peer_before`, `peer_after`,
`accepted`, and `receipt`. The current fields are raw reads of `0x409b00`
after activation immediately before submission and after the release,
before CE can select another channel. Both must equal
`0x80000000 | (instance >> 12)` for that frame's channel. Both trap arrays
must contain four zeros in `trap_registers` order; a recovered timeout
must still populate `error` and fail the gate under G9.

The binding interpretation comes from the checked MIT nouveau v6.8
`macros.fuc:125`, `hub.fuc:234-267,470-483` and
`gr/gf100.c:1613-1626`: FECS selects/saves the channel with bit 31 valid,
and the instance address is decoded from bits 27:0 shifted by 12. The
rnndb `CURRENT_CTX` declaration is labelled GM107 and newer; it is not
used as the authority for this Kepler field. Notices are retained in
`scripts/metal/gt710-suite` and listed in `THIRD_PARTY_NOTICES`.

`peer_before` and `peer_after` each contain two lowercase SHA-256 strings,
in poison 85/170 order, from complete CE transfers of the other channel's
target immediately before/after this draw. Their decoded content is retained
by reference to that peer's latest complete target image in this block;
before its first draw it is the specified all-zero image. Both pairs must
match that known content, not merely each other. Do not derive these
digests from cached expected pixels: read the peer target again under
both poisons. This avoids duplicating identical image bodies in the upload.

`receipt` contains `target` equal to this channel's `release`, `epoch`
equal to this channel's epoch, a nonzero 32-bit `nonce`, matching raw
`observed_epoch` and `observed`, and `consumed: true`, `accepted: true`.
No epoch/nonce pair may repeat among the eight frames. Even a suppressed
draw must execute its ordered release. Frame `accepted` is the pixel
verifier's result, independently checked by the host: false for the two
controls and true for their immediate clean successors and other frames.

`readbacks` contains 16 entries in image/poison order, each exactly
`{"case": "...", "poison": 85, "bytes": 16384, "sha256": "..."}`
(then poison 170). These hash decoded bytes, unlike the bulk anchors.

## Pixel decision

V1 uses `provenance/gt710-triangle.json`; V2 uses the independently derived
`v2` in `provenance/gt710-contexts.json`. Require their complete coverage
mask hashes, exact `[0,0,0,0]` background and opaque covered pixels,
and exact literal BGRA sample bytes. These dyadic geometry/sample checks
do not prescribe the other covered RGB bytes or permit a tolerance.
The existing 7.4 stage-suppression and constant-colour controls remain
required by `draw/controls`.
<!-- claim-lint:ok: normative pixel checks in scripts/metal/gt710-suite and oracle mutations in tests/gt710_contexts_report_test.py. -->

For B, independently construct the full image with BGRA
`[4*x, 4*y, 2*(x+y), 255]` at each integer coordinate and compare all
16,384 bytes, as well as the provenance SHA-256. A single changed byte
fails. `suppress` must leave exactly the cleared image; `corrupt` must
equal the preceding restored gradient with exactly one complete BGRA pixel
replaced by `[0,0,0,0]`. Its linear index is channel B's epoch modulo 4096:
`x = index % 64`, `y = index // 64`, byte offset `index * 4`. The producer
performs this four-byte upload through B's CE session after B's A197 draw
release and before the two read-backs. The host derives the location from
the channel epoch, rather than trusting an offset supplied by the producer.
Both must fail their normal pixel oracle, and both immediate clean successors
must pass. A producer's rejection flag alone is insufficient.
<!-- claim-lint:ok: normative control decisions exercised by tests/gt710_contexts_report_test.py. -->

The host test supplies rationally generated synthetic V1/V2 images and
the integer gradient, then mutates binding, peer state, control results,
successors, epochs/nonces, pixels, poison coverage and digests. It also
checks authenticated network framing and the aggregate upload budget.
These tests certify the scorer, not GPU execution.
