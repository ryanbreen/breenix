# Screen approvals

This directory is append-only. An approval is exported from Vigil's screen approvals
with `vigil-cli breenix screens export`, or relayed by the coordinator with the
operator's words quoted verbatim, the Eastern Time and gallery identity. Agents do
not author approvals. Keep old approval files when a picture changes.

Each JSON approval contains a `candidates` SHA-256 identifier and an `entries`
array. Each decision names `suite`, `case`, `checkpoint`, `sha256` and `verdict`;
only `approve` authorizes that exact checkpoint and digest. A table entry names
its approval file and the same candidates identifier. The approval may contain
additional export or relay metadata.

<!-- claim-lint:ok: Approval requirements specified in docs/gui/screen-proof.md. -->
Only `scripts/screens/approve.py` may add or change table entries, after checking
the approval against verified candidates and their pictured bytes. Native CRC
rows are derived from a run matching an approved parent digest. See
[the screen proof contract](../../../gui/screen-proof.md) for the complete rule.
