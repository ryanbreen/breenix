# Native GPU core producer (8.1)

The explicit `--gt710-category core` probe-mode diagnostic with `gt710_core_probe` uses `gpu::dev::Device::Gk208`
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
- **Masked class:** mask the 3D feature at admission before the copy
  submission. Admission must refuse without advancing the queue,
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
with the console's 330 s bound.

## Host scorer and companion boot (8.1 u8)

The host-run `core` category has `row`, `dispatch` and `producer`; its metal
milestone is `metal-gt710/gpu-core`. The coordinator's 2026-10-28 review ruling
requires one final-head production GPU-suite boot for `producer`, after the
profile-source and wording fixes are committed. Formats arrive in 8.2, so the
170 cases must refuse the unprobed format at 8.1. The production console suite
and explicit core diagnostic stand from the preceding acceptance when the
review fixes change no kernel or startup code. For same-head captures, use:

```text
scripts/metal/gt710-suite --boot CONSOLE_BOOT --core-boot CORE_BOOT --gpu-boot GPU_BOOT --sha FULL_COMMIT
```

The primary must exit 0 with console 88/88 and authenticated first-CAPS startup
evidence. Its retained handoff and final-marker events must be at most 330 seconds
apart. The separate successful probe-mode boot explicitly requests
`--gt710-category core`; its authenticated upload and original capture files
are retained under `core-diagnostic/`. Only that boot builds `gt710_core_probe`
and supplies row/dispatch evidence. Captures must bind their commit file,
verified `build.commit` and `toolsCommit` to their requested full commit, with
clean source, the required profiles and no gaps or fatal events.

For the review ruling's retained console/core pair, rescore the original
capture files with the final scorer, passing their original SHA and the
explicit final GPU SHA to `core_capture`, `record_capture` and
`acceptance_status`. Check the source diff first: this exception is authorized
only because kernel and startup code are unchanged. `acceptance.json` records the
retained SHA separately and binds `producerBoot` and `gpuElapsedMS` to the new
GPU capture; it does not relabel the older captures as final-head boots.

The scorer retains `acceptance.json` beside the console capture. It records each
boot ID, exit status, kernel/build/tools identity, profile and serial/bulk hashes;
the console handoff-to-done duration and startup masked maximum; named core
row/dispatch/producer and masked-capability results; and measured aggregates.
The masked-capability result requires the dispatch check, including the refused
A0B5 request and immediate clean successor. PROBE 17/17 alone is not that check.

No persistent session is used. The acceptance lane deploys branch tools under the
shared lock before the boots and restores main's tools afterward.

`row` checks the complete 512-byte CAPS record: backend 6, the created
927D/A06F/A0B5/A197 classes, native origins, limits, live geometry and reserved
bytes. Format/profile/pipeline support stays absent. `dispatch` checks five
clean draws and the copy; the three controls and their clean successors run
on copies,
using the native answer and method namespaces and matching receipts.

The diagnostic retains its refusal-only CAPS snapshot and tears its channels down.
Production uses the ordinary M7 draw path without the core controls. On the first
CAPS request, the existing native allocator/CE owner initializes FECS/GPCCS,
generates a context, uploads the committed shaders, binds A197 and executes CE2
readbacks on a live channel. It transfers that channel and its pinned bootstrap
storage to `dev::Device::Gk208`, resuming after the startup sequences. Shared
admission then reports the unprobed BGRA render capability. Startup failure
reports DEVICE_FAILED and its cause; it cannot qualify as capability evidence.
The existing console CE checks run before this first-use ownership transfer.

The native test probes refuse with `native GPU core owns FIFO` once first-use
startup claims the engine, including after failed startup. The ownership flag
remains visible while startup temporarily removes its retained
session. CE wait/status/drain, CE recovery, the startup CE/GR probes and board
inventory re-entry check ownership before hardware access. GR refusal precedes
cleanup capture, so it cannot reset or restore the core's live state. Recovery
publishes `GT710_RECOVERY_ERROR native GPU core owns FIFO` and returns its
retained session on that early error. The host check is
`tests/gt710_core_ownership_test.py`; it substitutes the hardware boundary and
does not supply hardware execution evidence.

First-CAPS startup runs with CPU interrupts enabled; only short register
read/modify/write sequences mask them. Concurrent first callers sleep through
the ownership transfer and result publication. The startup restores its caller's
entry IRQ state on success, failure and interrupted waiting.

Production startup retains one `GT710_CORE_STARTUP` JSON line in the existing
`/proc/gt710-pixels` bulk buffer, with `schema=1`, boolean `ready`, and unsigned
`irq_masked_max_us`. The latter snapshots the GPU gauge's boot-global maximum
through the startup window; the record reads the gauge without changing it.
The x86 masking helpers time instrumented intervals that begin with interrupts
enabled, including IRQ-safe mutex acquisitions. This startup-window measurement
does not measure interrupts-off time across GPU syscalls, which enter with
interrupts already disabled. That measurement belongs to the 24.9 performance
follow-up [#2567](https://github.com/ryanbreen/breenix/issues/2567). Serial carries
only `GT710_CORE_STARTUP_BULK <bytes> <sha256>`, covering the exact line and LF.
The acceptance capture must upload the buffer after first CAPS, through the
existing network uploader. The joiner authenticates this block and rejects
failed startup or a startup-window maximum over the 500 µs startup bound. A host check of
the encoder or interrupt state supplies no measured hardware duration.

The console entry point runs its CE wait/recovery checks before first CAPS,
then uploads the retained buffer before starting the cases. The GPU entry
point also calls CAPS before uploading and starting its cases. Disk staging
provides the lease-bound route for named console/GPU suites and lists containing
them. A sequence's later suite finds no pending rows after the earlier upload
unless a producer has added evidence. The GPU companion retains its bulk file;
scoring authenticates its startup row as well as the console's.

Acceptance checks the completed shared console manifest, including 88/88 passed
cases, 0/88 skips and the ordinary memory rule, plus the authenticated ready
startup row and diagnostic backend-6 CAPS snapshot. Startup failure, a
startup-window maximum above 500 µs, dirty or mismatched builds/tools, a failed
console or diagnostic exit, gaps or missing captures fail acceptance.

`producer` requires its own named production GPU capture: START, the shared manifest's 170 CASE identities
in order, DONE with 170 failures and no skips, elapsed time and READY. Each
case's sole failure must name CAPS status 2 and `missing b8g8r8a8-render`;
NO_DEVICE and other admission failures fail. The memory and 900-second rules
apply to that GPU boot, whose exit 1 is expected for 170 honest refusals.
The milestone 8 rendering gate remains 170 passing cases with its shared pixels
and existing bounds.

<!-- claim-lint:ok: the flag names are checked by tests/suite_sequence_test.py. -->
Use a named `--suite gpu` for this producer boot. The x86-metal `--suite all` aggregate
explicitly excludes `gpu`, with an admission reason from `suite-sequence.py`, until native CAPS
reports `ADMITTED` at milestone 8.7; registering the 8.1 producer does not admit it.

<!-- claim-lint:ok: tests/gt710_profile_test.py exercises suite combinations and the resolved all selection. -->
The launcher selects production for ordinary console, GPU and GT 710 selections,
including lists and `--suite all`. `--gt710-category core` is accepted only for
an automated probe-mode image boot with no suite or fault injection. Its
feature also requests the native hardware probes on that probe-mode boot. Omitting `--mode` selects this same probe route,
whose existing guest producer uploads the evidence before exiting.
The driver records the verified build's `commit`, `profile` and `features` in
`run.json` under `build`; the full manifest remains in `delivery.json`.
Scorers require production `build.profile` for console timing and GPU evidence,
and `gpu-core` with `gt710_core_probe` for the separate diagnostic. A missing
profile fails scoring.
