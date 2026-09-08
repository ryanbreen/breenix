# PR 2b evidence provenance

Kernel and ratchet revision for the final structure and boot gates:
`cab966e76c46d9a6eae343ba8bb5bf564d1ee24b`.

01–04 are pre-commit development checks based on `58af1a7f`, annotated in
those transcripts. The first emitter used a second serial macro; the logging
ledger rejected it. The emitter correction precedes the code commit.
05–18 record the code commit and the subsequent evidence-only work.

`strict-revision.txt`, `service-revision.txt`, `production-revision.txt` and
`beast-revision.txt` map raw serials/facts to their commands and source revision.
Raw serials were copied byte-for-byte; the local `.gitattributes` disables
Git text normalization for captured TXT files. `mac-preflight-logs.tar.gz` preserves
per-suite logs from the initial and corrected structure runs, strict, service
and production preflights. `beast-preflight-logs.tar.gz` preserves both Beast
attempts, including the 300-second timeout and the QMP fixture failure in the
900-second retry. The first Beast attempt was stopped before its built-in
600-second retry finished. The second exited 1 before kernel build or boot.

The forced oracle transcript is host execution of extracted scheduler methods;
it is not a boot serial. The baseline source is loaded from `58af1a7f`.
No x86 boot serial exists for this round because neither attempt reached QEMU.
Issue 963 tracks the un-attributed QMP fixture red. The round document is
`../../3F-PR2B-2026-09-07.md`.

`SHA256SUMS` covers the evidence files except itself.
