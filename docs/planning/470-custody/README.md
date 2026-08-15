# #470 custody campaign — recovered design artifacts

This directory preserves the documents that gated the #470 process-teardown custody
campaign (frame ledger + address-space custody record, PR-1a/1b/1c, PR-2, PR-3, and
the PR-4 re-scope). They previously lived only under `/tmp` scratchpad directories
and were lost when a reboot cleaned temporary storage.

- **`DESIGN-470-v2.md`** — the ratified custody design (v2): the allocator-generation
  frame ledger, the address-space custody record, `retire_bounded`/`abandon` dispositions,
  leaf custody, the oracle suite (O1/O2/O3), the structural ratchets, the PR plan, and the
  full constraint crosswalk (C1–C23, including C21). This is the design that PR-1a
  (#534), PR-1b (#539), PR-1c (#542), and PR-2 (#547) were built and reviewed against,
  and that gates P3 re-ratification and PR-4.
- **`TRAP-LIST-1a.md`** — the implementer's contract / trap list for PR-1a: mechanism
  traps found across review rounds r1–r5, the oracle contract (R1/R7/R9, O2/A–E),
  harness traps, the honesty contract, and the landing checklist.
- **`PR4-RESCOPE.md`** — the current PR-4 decision artifact, re-derived against today's
  main, reflecting the Q4→receipt shift that PR-3 already absorbed.

## Recovery provenance

`DESIGN-470-v2.md` and `TRAP-LIST-1a.md` were authored by workflow agents in an
earlier session and written to `/tmp` scratchpad paths
(`scratchpad/470design/DESIGN-470-v2.md`, `scratchpad/470pr1a/TRAP-LIST-1a.md`).
Both files were lost when a reboot cleaned `/tmp`. They were reconstructed byte-for-byte
from the original `Write` tool-call payloads recorded in this session's agent
transcript JSONLs (no intervening `Edit` calls were found for either file, so each
recovered file is the single, complete, as-written version — not a reassembly of
partial edits). Section headers 1–10 (including §7 the PR plan and §10 the
constraint crosswalk with C21) are present and intact in `DESIGN-470-v2.md`; both
files end on complete sentences with no evidence of mid-content truncation.
