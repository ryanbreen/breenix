# Import screen decisions

`approve.py` imports a candidates manifest from `tools/screen-gallery/export.py`
and the operator's decision file from `vigil-cli breenix screens export`:

```sh
python3 scripts/screens/approve.py /path/to/candidates/manifest.json /path/to/decisions.json
python3 scripts/screens/approve.py --native /path/to/stopped-native-run
```

`--table PATH` selects another table, primarily for host tests. The default is
`docs/suites/frames/screens.json`. Pillow is required to decode candidate PNGs.
This command does not run a guest, contact a target or author decisions.

## Decision file

The U03 decision fields and the coordinator's approval-schema ruling are the
basis of this format. U12 chooses `decided_at`, a timezone-qualified ISO 8601
string, for the decision time left unnamed by that ruling. An approval may omit
`reason`; a rejection requires a nonblank reason. This is the export shape
consumed by this command; unknown fields and duplicate JSON keys are errors.
The example below is a schema illustration, not operator approval evidence.

```json
{
  "candidates": "<64 lowercase hex candidates_id>",
  "by": "operator",
  "entries": [
    {
      "suite": "gpu",
      "case": "present/example",
      "checkpoint": "end",
      "mode": "2560x1600",
      "sha256": "<64 lowercase hex>",
      "verdict": "approve",
      "reason": "",
      "decided_at": "2026-11-02T08:00:00-05:00"
    }
  ]
}
```

Under the G29 amendment (2026-11-02), a decision file may instead use
`"by": "agent"`. Its entries require `"verdict": "approve"`, a nonblank `evidence`
string describing the checks and findings, and `delegation` containing the exact
operator quote in [the approval contract](../../docs/suites/frames/approvals/README.md).
These fields are copied into the table's approval metadata and checked against
the retained decision. The operator format above is unchanged.

## Mechanical Expect report

```sh
python3 scripts/screens/verify_expect.py /path/to/candidates --output /path/to/report.json
```

The argument may also name `manifest.json`. The verifier checks the candidate
identity and PNG digests, then reports a result for each row. It does not write
approvals or change expectations. Exit status is 0 for mechanically passing rows,
1 if a row fails, or 2 for unresolved claims without a failure. The JSON retains
checked claims and measurements, failed claims, unresolved claims, crop bounds,
scale and divisor, the Expect documentation commit, candidate identity, and hashes
of the manifest and verifier source. A failed claim takes precedence over
unresolved prose in the row verdict.

Intent crops use the centred layout in `libs/libbreenix/src/suite_intent.rs`,
checking the 16 px checkerboard, 2 px white border and uniform integer enlargement
blocks. Source dimensions come from the surface described in Expect; pixels
establish compatibility with that layout, not an independent record of the
original source dimensions. End frames and mode-sized pictures are checked as
captured. Decimation leaves unsampled source pixels unresolved.

Checks measure named hexadecimal colours and their shares, unambiguous uniform
fills and exclusive palettes, literal points, half-open rectangles, and rectangle
sizes stated before or after an origin. Explicitly described later overlays
exclude hidden pixels from the earlier rectangle's check. Coordinates are
zero-based from the picture's top left. Text origins, curves, procedural patterns,
spatial descriptions without literal coordinates, temporal claims and other
unsupported prose remain `unparseable` with the original claim and a reason.
Only a complete supported sentence can receive `pass`; the report alone is
insufficient for the G29 independent-review requirement.

BreenGL `bmp`, `png-decode` and `png-encode` comparison panels are checked first,
before generic prose dimensions. The verifier reconstructs the tile inventory
and layout from the captured commit's fixtures and the `images.rs` panel rules.
It checks each actual/reference pair in both the colour and grey alpha planes,
uniform integer enlargement blocks, white outlines, and entirely black displayed
difference pictures. Reference pixels are compared with the case's literal BGRA
values, independent sample arithmetic, or committed `.bgra` fixture files;
the verifier never uses the codec under test to generate a reference. The report
retains source-file hashes, fixture hashes, tile locations, scales, divisors and
comparison counts. Missing source commits and unsupported cases fail closed.
Intent and tile decimation leave omitted pixels unresolved. Labels, glyphs and
remaining visual prose also stay unresolved for independent review, so these
mechanical checks alone never authorize a G29 approval.

The manifest identifier is recomputed from canonical JSON of
`{"version":1,"rows":...}`, projecting each row without the optional documentation
fields `expect` and `expectCommit`, using sorted keys, compact separators and
unescaped Unicode, as the exporter does. The original row fields remain part of
the identity; unknown fields are refused. PNG dimensions must equal the row's mode. The
canonical pixels are BGRX with X set to FF; their SHA-256 must equal the row's
digest. PNGs must be opaque RGB PNGs inside the candidates directory.
Decisions must match a manifest row's suite, case, checkpoint, mode and digest.
A subset of the candidates may be decided. An undecided row leaves existing
expectations unchanged. Rejecting the currently approved digest and mode removes
that expectation and its derived native rows; rejecting a different picture
leaves the existing expectation unchanged.

The exporter reads manifests from its own checkout's HEAD commit, including the
case's optional `expect` sentence. A row with that sentence records its source
commit as `expectCommit`; both fields are omitted when the case has no `expect`.
The row's `commit` continues to identify the captured run. `--manifests` must name
a directory tracked in the exporter's repository. To refresh retained candidates
from their capture disk and serial log, pass `--refresh` with the original boot id
and capture commit. The exporter requires the same candidate identity, evidence
rows and PNG bytes before atomically replacing only `manifest.json` in place.

The imported decision file is retained byte for byte at
`approvals/approval-<SHA-256 of decision-file bytes>.json` beside the table.
Existing evidence is retained. The table is replaced atomically after input
validation. Approving a changed picture or mode discards its old native rows;
reapproving the same picture and mode preserves them.

## Native run evidence

U12 defines the following local artifact input for the later native capture
producer. A serial CRC alone is insufficient: it does not expose stability or
the output configuration. A directory passed to `--native` must contain:

- `run.json`: the metal runner's record, with `id`, a 40-hex `commit`,
  `platform: "metal"`, `arch: "x86_64"`, `mode: "suite"` and
  `evidenceEligible: true`. Other runner metadata is allowed.
- `serial.txt`: the run's CASE and SCREEN records in the existing scorer grammar.
- `screen-crc.json`: the version 1 object below. Its `run` and `commit` must
  match `run.json`. Fields not shown in this sidecar schema are errors.

```json
{
  "version": 1,
  "run": "<run id>",
  "commit": "<40 lowercase hex>",
  "checkpoints": [
    {
      "suite": "gpu",
      "case": "present/example",
      "checkpoint": "end",
      "backend": 6,
      "output_config": {
        "mode": "2560x1600",
        "raster": "1280x1024",
        "viewport": [0, 112, 1280, 800],
        "out": "sor0",
        "sources": ["sor0", "rg0"],
        "methods": [[1028, 0], [1232, 0]]
      },
      "binding_before": {"resource": 7, "present_fence": 9},
      "binding_after": {"resource": 7, "present_fence": 9},
      "elapsed_us": 85000,
      "disarmed": true,
      "display_error": 0,
      "status": 1,
      "secondary_absent": "gk208-rg-no-words-u13",
      "samples": [
        {"status": 1, "crc": "12345678", "crc2": null},
        {"status": 1, "crc": "12345678", "crc2": null},
        {"status": 1, "crc": "12345678", "crc2": null},
        {"status": 1, "crc": "12345678", "crc2": null},
        {"status": 1, "crc": "12345678", "crc2": null}
      ]
    }
  ]
}
```

<!-- claim-lint:ok: U13 sidecar contract, exercised by tests/screen_sor_crc_test.py. -->
`methods` contains the latched viewport, scaler and output configuration's
method/value pairs as unsigned 32-bit integers, ordered by unique, aligned
method address. The illustrated values are placeholders, not a hardware
configuration. The producer must supply the values actually used by the head.
`viewport` is `[x, y, width, height]` in the raster. Sources are primary SOR and
secondary RG selectors as programmed; the RG result is explicitly absent under
coordinator ruling U13 (E22 rulings, 2026-11-07). Captures
20261107T082545Z, 20261107T084839Z, 20261107T090458Z and 20261107T092218Z
recorded SOR words and no RG words. `secondary_absent` must equal
`gk208-rg-no-words-u13`, backend must be 6 and each sample must have `crc2: null`;
serial must say `crc2=none`. An absent secondary on backend 4 or 5 is refused.
Numeric zero, garbage, a missing reason and a mismatched secondary are refused.
Legacy dual-source records omit `secondary_absent` and require nonzero CRCs
in both fields. `SCRCRC01` v2 carries reason tag 1 at byte 116, followed by the
names at byte 120; each raw secondary word must be 0 as a reserved absence
word. A v1 numeric zero never becomes an absence.
The configuration digest is SHA-256 of `output_config` with the
same canonical JSON encoding described above; this is also the digest to pass
to `screen-verdict.py --native --config` for that configuration. The metal tools
pass `backend=6` for picture-only scoring; the CLI equivalent is `--backend 6`.
That admits the U13 record grammar and still checks approved pictures;
`--native --config` additionally matches the approved output CRC row.
An explicit run backend must agree with the caller.

<!-- claim-lint:ok: Input requirements from docs/gui/screen-proof.md and the pinned MIT crc.c and crc907d.c; not a hardware measurement. -->
`samples` contains consecutive written notifier entries: the first is discarded,
then at least K=4 (at most 254) must agree with the serial SOR CRC. Sample `status: 1` is the producer's normalized written marker, not the
undocumented hardware entry-status word. A primary CRC of `00000000` is unwritten,
following the pinned MIT `crc.c` reader. Context `status` must have bit 0 set and no bits in mask 0x3e.
These status meanings follow the pinned MIT
[`crc907d.c`](https://github.com/torvalds/linux/blob/e8f897f4afef0031fe618a8e94127a0934896aba/drivers/gpu/drm/nouveau/dispnv50/crc907d.c)
notifier layout and context-status decoding, recorded in THIRD_PARTY_NOTICES.
This importer implements validation from those facts; it contains
no copied driver code.

<!-- claim-lint:ok: U12 input validation requirements, covered by tests/screens_approve_test.py. -->
The binding must remain pinned with nonzero resource and present fence, capture
must finish within 250000 microseconds, disarm must complete and display error
must be zero. The named SCREEN must match an approved table digest and mode in
this run, and its SOR CRC, explicit secondary state, output and raster must match the capture evidence.
A missing or inconsistent parent refuses the import. Native rows record the
run and commit; they cannot add a picture expectation. This sidecar contract is
an input for U14/U18; U12 does not claim hardware acceptance or produce captures.

Text claims use `the text 'Wi é A', white (#FFFFFF), starting near x=8, y=40,
font size 16`. The verifier checks for the exact ink colour in a clipped box:
a quarter-em margin, up to 1.5 em per codepoint horizontally and 1.5 em vertically
from the anchor. It checks a named `on navy (#0A141E) background` against more than
half the judged picture's pixels. The string, font and glyph shape remain unresolved for independent vision review.
Decimation leaves missing ink and source colour shares unresolved. A source colour
omitted by antialiasing must not be asserted as an exact visible pixel.
