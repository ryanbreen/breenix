# Native GPU core producer (8.1)

The x86 console-suite route with `gt710_core_probe` uses `gpu::dev::Device::Gk208`
for the five 7.4 A197 draws and the A0B5 rectangle copy into the inactive
scanout. The existing input uploads and complementary readbacks retain their
7.4 mechanisms. The core producer runs inside the same graphics probe, before
7.5 context switching. It adds evidence for the later `gt710` core category;
it does not claim the complete 170-case GPU suite is admitted or has passed.

`DrawSession` temporarily transfers its active session, including the peer,
into the adapter. It resumes the native sequence after the preceding diagnostic
submission. On return, the session goes back to the original owner before
propagating success or failure. A failed publication or completion leaves its
mappings and backing under the existing FIFO drain, GR reset and retention
rules. This probe does not use the installation API for a fresh unbound channel,
restart a half-saved image, open another channel pair, or retain a session after
the boot probe. Scratch ownership remains with the enclosing probe cleanup.

## Wire format

The existing bounded network upload (`/proc/gt710-pixels`, retained as
`gt710-pixels.rle`) carries one contiguous block of ASCII lines. A line starts
with `GT710_CORE `, followed by one JSON object and one LF byte. Schema is 1.
Identifiers in `case`, `control` and `outcome` are fixed producer strings.
Integers are unsigned decimal numbers; booleans use JSON spelling.

The first line has `type="caps"`, `bytes=512`, and `hex`: 1,024 lowercase
hexadecimal digits encoding the exact little-endian CAPS v1 ABI bytes. There
is one CAPS line per producer. It names backend 6 and the created
927D/A06F/A0B5/A197 classes. The snapshot follows successful native shader
binding and CE upload/readback, and checks the live native scanout. Unprobed
format/profile capabilities remain absent; `MISSING_CAPABILITY` is expected at
8.1 and must not be rewritten to `ADMITTED` for this diagnostic.

Subsequent objects have `type="completion"` and these fields:

| Field | Meaning |
| --- | --- |
| `case` | `triangle` (five draws), `copy`, or `after-withheld`, `after-stale`, `after-mask` |
| `control` | `clean`, `withheld-release`, `stale-epoch`, or `masked-class` |
| `context`, `epoch` | Native instance address and current reset epoch |
| `sequence` | Native queue sequence; 0 for a request refused before publication |
| `caller` | Caller fence, `(1 << 32) \| planned_sequence`, distinct from the native sequence |
| `resource`, `generation` | Destination GPU VA and diagnostic resource generation (1) |
| `kind` | ABI native A197 draw release (`0xa1971b0c`) or A0B5 transfer release (`0xa0b50300`) |
| `answer` | Unmodified adapter Done answer; `0x60000000` is native success, 0 means no Done answer |
| `observed_context`, `observed_epoch`, `observed_sequence` | Actual adapter sample of binding and engine release |
| `consumed` | Native sequence whose stream consumption the adapter confirmed, or 0 |
| `healthy` | Whether the sampled binding and engine/FIFO checks passed |
| `elapsed_us` | Device-side interval, under the existing 2,000,000 µs request wait bound |
| `outcome` | `done`, `rejected`, `refused`, or `error` |

An unsubmitted request has unsampled observation fields set to 0 and
`healthy=false`. A control that unexpectedly returns Done records its actual
answer with `outcome="error"`; it must fail acceptance. A failed/timed-out
request retains the latest sample and is also an error.

Serial carries only this block's anchor:

```text
GT710_CORE_BULK <bytes> <sha256>
```

`bytes` and the lowercase SHA-256 cover the concatenated original lines,
including their trailing LFs. The anchor is not included in its own hash.
The encompassing `GT710_BULK` still authenticates the complete upload. The
joiner authenticates the core block independently and inserts it at its anchor,
keeping the existing draw and context blocks at their original boundaries.
Core JSON on serial, missing blocks, duplicate/interleaved blocks, mismatched
lengths and digest changes fail transport validation. Existing 7.4 and 7.5
record encoders and their pixel oracles retain their schemas.

## Controls and clean requests

The first core rectangle copy runs the three controls, each followed immediately
by an ordinary copy through the same adapter. The requested final copy then
runs normally. Draws and clean copies require the adapter's matching native
context, epoch, sequence, stream consumption, health and Done fence.

The native sequence also counts the surrounding diagnostic CE work, which
is checked by the existing draw/readback cases. Successive draw rows differ
by seven: two poisoned readbacks, four 4 KiB background-upload chunks, and
the next draw. The first copy is three after the final draw: its two
readbacks and the copy. The control/copy block then advances one per
published request; the masked request is refused before publication and
its clean successor reuses the planned sequence. The scorer requires these exact
deltas, not arbitrary increasing gaps.

- **Withheld release:** execute the CE transfer and epoch releases, omit the
  requested sequence release, and append a separate A0B5 engine release to
  the fourth word of the 16-byte receipt slot. The ordinary completion must
  stay empty. Only the separate drain release, consumed stream, matching
  context/epoch and unchanged sequence word (0) permit negative retirement.
- **Stale epoch:** execute the same transfer with the previous epoch in its
  engine release. The sequence must match and the epoch must differ by one.
  The separate terminal release and stream consumption are required here too.
- **Masked class:** remove A0B5 from the adapter's effective class profile
  before submission. Admission must refuse without advancing the queue,
  producing Done, or publishing a command. The next unmasked copy must complete.

The terminal control word is not a request completion and cannot produce Done.
Both release controls use the real A0B5 release mechanism from MIT
`classes/dma-copy/cla0b5.h`; FIFO SET_REFERENCE alone does not retire them.
The next clean requests require fresh engine receipts, without a synthetic
success or reset. Source copyrights and full MIT grants remain in `copy.rs`
and `gr/channel.rs`; `THIRD_PARTY_NOTICES` lists those sources.

The encoder/anchor host check is `tests/gt710_core_producer_test.py`.
`tests/gt710_core_adapter_test.py` executes the production producer, adapter,
class profile and control encoder against controlled engine words; it also
checks upload joining and rejects changed bytes and a changed core digest.
These host checks do not supply device execution evidence. The later acceptance
unit boots and scores the native core category and the existing 7.4/7.5 records,
with the console's 330 s bound and a separately timed GPU-suite boot.

## Host scorer and companion boot (8.1 u8)

The host-run `core` category has `row`, `dispatch` and `producer`; its metal
milestone is `metal-gt710/gpu-core`. Run the scorer after the two acceptance
boots, with matching kernel and tool commits:

```text
scripts/metal/gt710-suite --boot CONSOLE_BOOT --gpu-boot GPU_BOOT --sha FULL_COMMIT
```

The primary is a completed successful console-suite boot. Its retained
handoff and final-marker events must be at most 330 seconds apart. The
companion is a distinct completed `--suite gpu` boot. Its original run,
commit, cleanliness, exit, event and serial files are copied under the
primary capture's `core-producer/` directory, then rescored from those files.
No persistent session is used. The acceptance lane deploys the branch tools
under the shared lock before either boot and restores main's tools afterward.
This unit does not boot or deploy.

`row` checks the complete 512-byte CAPS record: backend 6, the created
927D/A06F/A0B5/A197 classes, native origins, limits, live geometry and reserved
bytes. Format/profile/pipeline support stays absent. `dispatch` checks five
draws, the copy, three rejected controls and their immediate clean successors,
using the native answer and method namespaces and matching receipts.

The shared core retains only the diagnostic's refusal-only CAPS snapshot;
its channels follow the existing teardown. A GPU-only `gt710_core_probe` boot requests
the same bounded native probes so that its syscall CAPS names the same
unprobed capability. Admission refuses the suite's cases before rendering.
The runner acquires no GPU view or display ownership for that unprobed native
profile and leaves the owned native console in place.

`producer` requires START, the shared manifest's 170 CASE identities in order,
DONE with 170 failures and no skips, `SUITE gpu ELAPSED ms=N`, and READY.
Each case's sole failure must name the measured unprobed admission capability.
Panic, stalls, other failure reasons, incomplete capture, changed commits,
console gaps, missing or inconsistent memory records and elapsed time above
900 seconds fail. Exit status 1 is expected for these admission refusals;
it is accepted only with that complete evidence. This host milestone is
integration evidence, not a passing GPU suite. Later milestones retain the
unchanged 170-pass GPU gate, pixels and bounds.

The metal launcher enables `gt710_core_probe` for console, GPU and GT 710 suite
boots. This compiles the producer, controls and refusal snapshot while keeping
production init and the requested suite target; it does not enable `testing`.
