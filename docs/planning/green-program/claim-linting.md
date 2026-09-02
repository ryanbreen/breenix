# Claim linting — running scripts/claim-lint.py

**R1, 2026-09-01.** The 2026-09-01 assessment found that 37 of 66 blocking review
findings across twelve arcs (56%) were factually wrong, checkable *sentences* in
EVIDENCE/PROVE docs, PR bodies, gate-script headers, or source comments — not code
defects. Nine of eleven multi-round arcs were held open by a final round containing
only prose findings. The assessment's recommendation was folded into dispatch briefs
as an instruction ("state N of M, name a mutation, cite an artifact") and three more
instances of the same failure shipped the same day anyway. An instruction that isn't
checked isn't a control. `scripts/claim-lint.py` is the mechanical version: it reads
the changed files in a diff, flags the sentence *shapes* that made up those 37
findings, and reports file:line, the offending text, and which rule fired. It cannot
tell you whether a claim is true — only that it makes a strong claim with no evidence
attached to it in the same breath, which is exactly the thing every one of the 37
false claims had in common.

## Running it

```bash
# Lint files changed on this branch vs origin/main (the normal case)
scripts/claim-lint.py

# Diff against a specific ref
scripts/claim-lint.py --base main

# Lint specific files (bypasses git diff)
scripts/claim-lint.py --files docs/planning/green-program/tty/EVIDENCE-2026-08-30.md

# JSON output, for tooling
scripts/claim-lint.py --format json

# Lint every tracked text file in the repo (slow; useful for auditing an
# existing doc rather than a diff)
scripts/claim-lint.py --all
```

Run it **before requesting review**, over your own changed files — the review slot
should not be the first place a claim-discipline violation is caught; that is the
whole point of making it mechanical instead of an instruction.

<!-- claim-lint:ok: the "never" below describes the tool's own file-extension
     allowlist and rule scoping, verifiable by reading scripts/claim-lint.py's
     TEXT_EXTENSIONS set and RULES list directly. -->
Exit code 0 = clean, 1 = un-discharged findings, 2 = usage error. It checks `.md`,
`.rs`, `.sh`, `.py`, `.txt` files: markdown prose (including table cells and
blockquotes) and `//`/`///`/`//!`/`#` comment blocks in source and scripts. It does
not touch code, and it never blocks on anything outside a comment or a doc.

## What it catches

<!-- claim-lint:ok: see scripts/claim_lint_corpus/historical_false_claims.json --
     every entry there names the rule it was built to catch. -->
Five rules, each reverse-engineered from a verbatim quote this campaign's review slot
actually caught — see `scripts/claim_lint_corpus/historical_false_claims.json` for
every specimen a rule was built against, with the arc, the file:line or issue it
shipped in, and which review round caught it:

| Rule | Fires on | Needs, in the same paragraph |
|---|---|---|
| `universal-claim` | every, all, none, zero, always, never, nothing, nowhere, wholly, entirely, completely, fully, exclusively, invariably | an N-of-M count (`12/12`, `3 of 4`), a `serials/`/`confirm/`/`scratchpad/`/`.log`/`.txt` citation, or `claim-lint:ok` |
| `unproven-claim` | proven, proves, proof, PROVEN | a named mutation/experiment (mutation, revert, redden, falsify, reproduce, regression, `--boots`, bisect), an N-of-M count, an evidence-log citation, or `claim-lint:ok` |
| `live-no-artifact` | "observed live", "confirmed live" | a `serials/`/`confirm/`/`scratchpad/`/`.log`/`.txt` citation, or `claim-lint:ok` |
| `absolute-guarantee` | airtight, guarantee(d/s), structurally | an N-of-M count, an evidence-log citation, or `claim-lint:ok` |
| `artifact-path-missing` | "preserved/attached/committed/saved/written at/to/in" + a backtick-quoted path | the cited path must resolve on disk (checked relative to the citing file's own directory, then the repo root) |

Two design choices, both deliberate:

- **A bare source-code path (`kernel/src/foo.rs`) does not count as evidence.**
  The gtty arc's worst false claim ("every close path ... calls `pair.slave_close()`")
  cited two `.rs` files as if naming the code under discussion proved a claim about
  *every* caller across the tree — it didn't; a tenth caller elsewhere was the bug.
  Only a captured log/serial counts, or an explicit `claim-lint:ok` you write after
  checking.
- **The unit is the paragraph, not the sentence.** A single claim in this corpus is
  almost always one bullet, one blockquote, or one table row, and the evidence for it
  is often in a different clause of the same bullet than the trigger word. This also
  means a long paragraph mixing a well-cited claim with an uncited one only needs one
  citation anywhere to pass both — keep evidence paragraphs to one claim each.

## Discharging a hit honestly

If the claim is true and you can name what makes it true, cite it in the same
paragraph. An N-of-M count or a `serials/`/`confirm/`/`.log`/`.txt` path is enough on
its own for `universal-claim` / `absolute-guarantee` / `live-no-artifact`. When there
is nothing that shape to cite (an architectural property, a code-review conclusion,
a "this is closed by construction" claim), add an explicit annotation naming the
artifact:

```markdown
<!-- claim-lint:ok: 13/13 arms, review-baseline.log -->
Every arm passed the baseline run.
```

```rust
// claim-lint:ok: mutation-proven, see fix2-review.md BLOCKING 1
// every close path decrements the slave refcount on release.
```

**Discharging a hit honestly means writing the citation you actually have, not the
citation that would make the flag go away.** If you cannot name a mutation, a boot
count, or a log for a strong claim, that is the linter doing its job — narrow the
sentence to what you measured ("x86 was built and booted once; the PTY paths were
not exercised" beats "x86: PROVEN, PASS"), don't annotate around it.

## What it does not catch — read this before trusting a clean run

This is a text-shape detector, not a verifier. Run against the campaign's own
historical false claims (`scripts/test_claim_lint.py`, `HistoricalCorpusTests`), it
catches **15 of 18** (83%) — see that file's corpus for the three misses and why:

- A count in **correct N-of-M form that was never actually produced** (fabricated
  test execution reported as `8/8`, `5/5` — the numbers matched the `#[test]` counts
  in the file, not an actual run) is invisible to a shape check; catching it needs
  parsing the cited log and cross-checking the count, not reading the sentence.
- A **negated-change claim** ("no fix-round change touched its inputs", when every
  one of its inputs had moved) uses no word in this tool's vocabulary and would need
  a dedicated check cross-referenced against the actual diff.
- **`each`** is a real universal quantifier and was deliberately left out of the
  trigger list: adding it raised the false-positive count by ~20 raw sites in a single
  611-line evidence doc (`WORKLOAD-ENVELOPES.md`) for one additional historical catch.

<!-- claim-lint:ok: the 48/167 and 48/68 figures are computed directly from
     scripts/test_claim_lint.py's RealDocumentReportTests, re-run every time
     that suite runs -- not a one-off manual count. -->
**The honest false-positive rate is not low in absolute terms.** Run against two real,
already-reviewer-verified-correct evidence docs in this tree
(`WORKLOAD-ENVELOPES.md`, `tty/EVIDENCE-2026-08-30.md` — 167 paragraphs combined),
the tool flags 48 of them (29% of all paragraphs; 71% of the paragraphs that use one
of the trigger words at all) as needing a citation they don't carry in-paragraph, even
though the underlying claims are true. Most of those paragraphs *are* well-evidenced —
just via a `script:line (current tree)` pointer or a citation one paragraph away,
neither of which this tool accepts (on purpose: loosening either would re-open the
exact miss class above). Treat a clean run as "no bare absolutes shipped," not as "this
document is now proven." Treat a flagged run on a large existing doc as a backlog of
one-line annotations to add, not as 48 new defects.

## Calibrating further

<!-- claim-lint:ok: verifiable by reading the RULES list and its matching
     entries in scripts/claim_lint_corpus/historical_false_claims.json directly. -->
Every rule above exists because of a specific, quoted false claim. If you find a new
recurring false-claim shape this tool misses, add it to
`scripts/claim_lint_corpus/historical_false_claims.json` with the verbatim quote,
the file:line/issue it shipped in, and the review that caught it — then build
detection for that shape. Don't add trigger words on spec; the `each` decision above
is the record of what happens when you do (more noise than signal at this tool's
current exemption precision) and the reasoning to redo if the exemption logic gets
smarter later.
