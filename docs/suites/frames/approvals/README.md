# Screen approvals

This directory is append-only. An approval is exported from Vigil's screen approvals
with `vigil-cli breenix screens export`, or relayed by the coordinator with the
operator's words quoted verbatim, the Eastern Time and gallery identity. Under the
G29 amendment (2026-11-02), delegated agents may also approve. Keep old approval
files when a picture changes.

<!-- claim-lint:ok: G29 amendment; agent field requirements checked in tests/screens_approve_test.py and tests/screens_table_test.py. -->
The decision file's `by` is `operator` or `agent`. Operator entries are unchanged.
Every entry in an agent file must have `verdict: "approve"`, a nonblank string
`evidence` stating which checks ran and their findings, and a `delegation` string
containing these exact operator words:

> can you have an agent do most of these approvals and validations? most of these seem like they would be super easy for you to verify without my help.

<!-- claim-lint:ok: G29 amendment authorizes agent approval only; tests/screens_approve_test.py refuses agent rejection. -->
The table's `approval` metadata retains `by: "agent"`, `evidence` and `delegation`
matching the retained decision entry. Agents never reject. A mismatch, disagreement
or unresolved claim stays undecided for the operator, marked "needs you" with its
reason in the review workflow. An agent approval requires two independent checks
that agree: the mechanical Expect pixel checks plus Opus vision review for claims
pixels cannot settle, or an Opus sampled audit for mechanically checkable claims.
A mechanical report alone does not authorize approval. Vigil shows agent approvals
as such, and the operator may override them there.

Each JSON approval contains a `candidates` SHA-256 identifier and an `entries`
array. Each decision names `suite`, `case`, `checkpoint`, `sha256` and `verdict`;
only `approve` authorizes that exact checkpoint and digest. A table entry names
its approval file and the same candidates identifier. See [the importer format](../../../../scripts/screens/README.md)
for the strict decision fields; unknown fields are refused by the importer.

<!-- claim-lint:ok: Approval requirements specified in docs/gui/screen-proof.md. -->
Only `scripts/screens/approve.py` may add or change table entries, after checking
the approval against verified candidates and their pictured bytes. Native CRC
rows are derived from a run matching an approved parent digest. See
[the screen proof contract](../../../gui/screen-proof.md) for the complete rule.
