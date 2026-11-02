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

The manifest identifier is recomputed from canonical JSON of
`{"version":1,"rows":...}`, projecting each row without the optional documentation
fields `expect` and `expectCommit`, using sorted keys, compact separators and
unescaped Unicode, as the exporter does. The original row fields remain part of
the identity; unknown fields are refused. PNG dimensions must equal the row's mode. The
canonical pixels are BGRX with X set to FF; their SHA-256 must equal the row's
digest. PNGs must be opaque RGB PNGs inside the candidates directory.
Decisions must match a manifest row's suite, case, checkpoint, mode and digest.
A subset of the candidates may be decided. Rejected or undecided rows do not
modify existing expectations.

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
      "samples": [
        {"status": 1, "crc": "12345678", "crc2": "abcdef12"},
        {"status": 1, "crc": "12345678", "crc2": "abcdef12"},
        {"status": 1, "crc": "12345678", "crc2": "abcdef12"},
        {"status": 1, "crc": "12345678", "crc2": "abcdef12"},
        {"status": 1, "crc": "12345678", "crc2": "abcdef12"}
      ]
    }
  ]
}
```

`methods` contains the latched viewport, scaler and output configuration's
method/value pairs as unsigned 32-bit integers, ordered by unique, aligned
method address. The illustrated values are placeholders, not a hardware
configuration. The producer must supply the values actually used by the head.
`viewport` is `[x, y, width, height]` in the raster. Sources are primary SOR and
secondary RG. The configuration digest is SHA-256 of `output_config` with the
same canonical JSON encoding described above; this is also the digest to pass
to `screen-verdict.py --native --config` for that configuration.

<!-- claim-lint:ok: Input requirements from docs/gui/screen-proof.md and the pinned MIT crc.c and crc907d.c; not a hardware measurement. -->
`samples` contains consecutive written notifier entries: the first is discarded,
then at least K=4 (at most 254) must agree with the serial CRC pair. Sample `status: 1` is the producer's normalized written marker, not the
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
this run, and its CRC pair, output and raster must match the capture evidence.
A missing or inconsistent parent refuses the import. Native rows record the
run and commit; they cannot add a picture expectation. This sidecar contract is
an input for U14/U18; U12 does not claim hardware acceptance or produce captures.
