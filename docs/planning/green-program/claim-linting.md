# Claim linting — running scripts/claim-lint.py

<!-- claim-lint:ok: 2 commands produce every number in this doc and each is
     printed beside its number -- `python3 scripts/test_claim_lint.py -v` for
     the catch rates, the per-document counts and the per-round counts, and
     scratchpad/claimlint/r2/measure.py for the held-out set and the two
     attribution tables. -->
**R1, 2026-09-01. R2, 2026-09-01** — every number below was re-derived after the
R2 changes, and the command that produced it is printed next to it.

The 2026-09-01 assessment found that 37 of 66 blocking review findings across
twelve arcs (56%) were factually wrong, checkable *sentences* in EVIDENCE/PROVE
docs, PR bodies, gate-script headers, or source comments — not code defects. Nine
of eleven multi-round arcs were held open by a final round containing only prose
findings. The assessment's recommendation was folded into dispatch briefs as an
instruction ("state N of M, name a mutation, cite an artifact") and three more
instances of the same failure shipped the same day anyway. An instruction that
isn't checked isn't a control. `scripts/claim-lint.py` is the mechanical version:
it reads the lines a diff changed, flags the sentence *shapes* that made up those
37 findings, and reports file:line, the offending text, and which rule fired. It
cannot tell you whether a claim is true — only that it makes a strong claim with
no evidence attached to it in the same breath.

## Running it

```bash
# The normal case: only findings in hunks this branch changed, vs origin/main
scripts/claim-lint.py

# Diff against a specific ref (resolved to `git merge-base <ref> HEAD`)
scripts/claim-lint.py --base main

# Same diff, but report findings anywhere in a changed file
scripts/claim-lint.py --whole-file

# Lint specific files whole — REQUIRED for surfaces outside the repo
scripts/claim-lint.py --files /tmp/pr-body.md scratchpad/<arc>/EVIDENCE.md

# JSON output, for tooling
scripts/claim-lint.py --format json

# Lint every tracked text file (slow; for auditing an existing doc)
scripts/claim-lint.py --all
```

Diff mode reports **only findings whose paragraph overlaps a line this branch
changed** (`--changed-only`, the default). That matters: linting whole changed
files produced 84–282 findings per real fix round against 8–18 in changed hunks
— 79% to 97% of them on prose the round did not touch (table below).
`--whole-file` restores the old behaviour when you want to audit a file end to
end.

The base is resolved to `git merge-base <ref> HEAD` and then diffed against the
**working tree**: a stale local `main` does not drag in files the branch did not
touch, and uncommitted edits are still linted before you commit them.

<!-- claim-lint:ok: the file-extension allowlist and the skipped-directory rule
     are `TEXT_EXTENSIONS` and `CAPTURE_DIR_RE` in scripts/claim-lint.py, with
     test_capture_directories_are_skipped in scripts/test_claim_lint.py. -->
Exit code 0 = clean, 1 = un-discharged findings, 2 = usage error. It checks `.md`,
`.rs`, `.sh`, `.py`, `.txt` files: markdown prose (including table cells and
blockquotes) and `//`/`///`/`//!`/`#` comment blocks in source and scripts. It
skips captured-artifact trees (`serials/`, `confirm/` — 449 tracked files here):
nobody can discharge a claim written inside a serial a gate script emitted. It
does not touch code, and it does not fire outside a comment or a doc.

## The round checklist — the run is an artifact

This repo has no CI and no pre-commit hook, so the run does not happen unless a
person makes it happen. An unchecked instruction is the thing this tool exists to
replace, so the run itself is recorded as evidence, exactly like a gate serial:

1. Before requesting review, run `scripts/claim-lint.py` over the diff.
2. Run `scripts/claim-lint.py --files <pr-body.md> <scratchpad evidence/confirm
   docs>` as a **separate step**. Diff mode only sees tracked repo files; PR
   bodies, GitHub issue comments and scratchpad evidence are exactly where the
   assessment found much of the false prose (see "surfaces it cannot see").
3. Paste both invocations and their exit statuses into the round's notes:

   ```
   claim-lint: scripts/claim-lint.py                       -> exit 0
   claim-lint: scripts/claim-lint.py --files /tmp/pr-body.md -> exit 0
   ```

4. The review slot checks for those lines the way it checks gate serials. A
   round with no claim-lint line is a round where this control did not run, and
   that is itself a finding.

## What it catches

<!-- claim-lint:ok: see scripts/claim_lint_corpus/historical_false_claims.json --
     every entry there names the rule it was built to catch, and 12 of the 18
     name the commit and path where the sentence shipped. -->
Five rules, each reverse-engineered from a verbatim quote this campaign's review
slot actually caught — see `scripts/claim_lint_corpus/historical_false_claims.json`
for every specimen a rule was built against, with the arc, the file:line or issue
it shipped in, the review round that caught it, and (for the 12 whose bytes are
recoverable) the commit the sentence shipped at:

| Rule | Fires on | Needs, in the same paragraph |
|---|---|---|
| `universal-claim` | every, all, none, zero, always, never, nothing, nowhere, wholly, entirely, completely, fully, exclusively, invariably | an N-of-M count (`12/12`, `3 of 4`), a captured-artifact citation that **resolves on disk**, or a `claim-lint:ok` carrying a citation |
| `unproven-claim` | proven, proves, prove, proved, proving, proof, demonstrated | a named mutation/experiment (mutation, revert, redden, falsify, reproduce, regression, `--boots`, bisect), an N-of-M count, a resolving artifact citation, or a cited `claim-lint:ok` |
| `live-no-artifact` | "observed live", "confirmed live" | a resolving artifact citation, or a cited `claim-lint:ok` annotation <!-- claim-lint:ok: this row lists the rule's own trigger vocabulary rather than making a claim; the vocabulary is LIVE_CLAIM_RE in scripts/claim-lint.py --> |
| `absolute-guarantee` | airtight, guarantee(d/s), structurally | an N-of-M count, a resolving artifact citation, or a cited `claim-lint:ok` annotation <!-- claim-lint:ok: this row lists the rule's own trigger vocabulary rather than making a claim; the vocabulary is ABSOLUTE_GUARANTEE_RE in scripts/claim-lint.py --> |
| `artifact-path-missing` | a preserved/attached/committed/saved/written-at claim naming a backticked path | the cited path must resolve on disk (checked relative to the citing file's own directory, then the repo root) |

Three design choices, each deliberate:

<!-- claim-lint:ok: specimen F3 in scripts/claim_lint_corpus/historical_false_claims.json,
     regression-tested by test_source_path_does_not_clear_a_universal in
     scripts/test_claim_lint.py -->
- **A bare source-code path (a `.rs` file) does not count as evidence.** The gtty
  arc's worst false claim ("every close path ... calls `pair.slave_close()`")
  cited two Rust files as if naming the code under discussion settled a claim
  about *every* caller across the tree — it didn't; a tenth caller elsewhere was
  the bug. Only a captured log/serial counts, or an explicit `claim-lint:ok` you
  write after checking.
<!-- claim-lint:ok: test_nonexistent_evidence_path_does_not_clear_a_universal and
     test_nonexistent_evidence_path_does_not_clear_a_live_claim in
     scripts/test_claim_lint.py; specimen F2 in the corpus file -->
- **A cited artifact that does not exist cannot exempt anything (R2).** Before
  R2, an exempting path was accepted on shape alone while the
  `artifact-path-missing` rule right next to it checked the filesystem. Corpus F2
  is precisely a cited serial path that did not exist; phrased as "see `<that
  path>`" it used to *silence* the rule instead of tripping it. Both now resolve
  through the same check. Cost, measured: a true claim citing a log that was
  never committed is now flagged (corpus T1 is exactly that, and moved from
  "clean" to "needs an annotation").
- **The unit is the paragraph, not the sentence — but a bullet is a paragraph
  (R2).** A claim in this corpus is usually one bullet, one blockquote, or
  one table row, and the evidence for it often sits in a different clause of the
  same bullet. Merging a whole hanging-indent bullet list into one paragraph,
  though, let a count in bullet 3 exempt an absolute in bullet 1 — that is
  literally why corpus F1 was reported caught but missed in the document it
  shipped in. Since R2 a list item (`*`, `-`, `+`, `1.`) or an ATX heading starts
  a new paragraph with or without a blank line.
  <!-- claim-lint:ok: 2 of 12 in-context specimens (F4, F16) miss this way, per
       HistoricalCorpusInContextTests in scripts/test_claim_lint.py -->
  **One citation still inoculates a whole bullet**, and 2 of the 12 in-context
  corpus specimens (F4, F16) are measured misses for exactly that reason.

## Discharging a hit honestly

If the claim is true and you can name what makes it true, cite it in the same
paragraph. An N-of-M count or a resolving `serials/`/`confirm/`/`.log`/`.txt`
path is enough on its own. When you have no artifact of that shape to cite (an
architectural property, a code-review conclusion, a "closed by construction"
claim), add an explicit annotation naming what you checked:

```markdown
<!-- claim-lint:ok: 13/13 arms, review-baseline.log -->
Every arm passed the baseline run.
```

```rust
// claim-lint:ok: see scripts/test_claim_lint.py
// every close path decrements the slave refcount on release.
```

<!-- claim-lint:ok: the forward-attachment rule is attach_annotations_forward()
     in scripts/claim-lint.py, asserted by
     test_annotation_does_not_discharge_the_paragraph_above_it in
     scripts/test_claim_lint.py -->
**Put the annotation immediately above the claim it discharges (R2).** An
annotation block attaches forward, to the paragraph that follows it, never
backwards — otherwise a `claim-lint:ok` written for one bullet silently clears
the bullet above it. In `//`/`#` comment blocks, order does not matter: the whole
comment block is one paragraph.

**A bare `claim-lint:ok` silences nothing (R2).** The annotation must be followed
by text containing at least one of: an N-of-M count, an issue number, a review
reference, or a path that resolves on disk. Before R2 the doc demanded a citation
and the code accepted `claim-lint:ok: lol`; with the per-round volume this branch
also fixes, that was the path of least resistance.

**Discharging a hit honestly means writing the citation you actually have, not
the citation that would make the flag go away.** If you cannot name a mutation, a
boot count, or a log for a strong claim, that is the linter doing its job —
narrow the sentence to what you measured ("x86 was built and booted once; the PTY
paths were not exercised" beats "x86: PROVEN, PASS"), don't annotate around it.

Note the annotation is a **weaker** bar than the exemption, not a stronger one:
the exemption refuses a source path as evidence, while `claim-lint:ok: see
scripts/claim-lint.py` passes. That is intentional — the annotation records that
a human checked something and says what — but do not read a discharged paragraph
as a better-evidenced one than an exempted paragraph.

## What it does not catch — read this before trusting a clean run

This is a text-shape detector. It cannot execute a gate, replay a mutation, or
read a log.

**Catch rate, in the mode it runs in: 9 of the 12 specimens whose shipped bytes
are recoverable (75%).** Command:
`python3 scripts/test_claim_lint.py -v` →
`HistoricalCorpusInContextTests`, which recovers each sentence's file with
`git show <shipped_commit>:<shipped_path>`, lints the whole file, and counts a
specimen caught only when a finding's own paragraph contains the offending
sentence. Before R2 this was 8 of 12 (67%).

**Secondary, and higher because it is measured in a mode the tool never runs in:
15 of 18 (83%) as isolated one-sentence files** (`HistoricalCorpusIsolatedQuoteTests`).
A lone sentence has no paragraph around it to exempt it. That 83% was the R1
record's headline; it is reported here second, and labelled, because it overstates
what the tool does to a real document. Corpus F1 is the demonstration: caught as a
quote, missed in the §9 bullet list it shipped in.

**On a held-out set — 13 false claims from review files this tool was not
calibrated on (`p721`, `sweep3`, `p673`, `p713`, `g568`, `coreproof/rung3`,
`p728fix`), recovered at their shipping commits and linted whole — it catches
7 of 13 (54%).** Pooled with the in-context corpus that is **16 of 25 (64%)**.
That set was assembled by the R2 review slot, lives in its scratchpad
(`scratchpad/claimlint/tree2`, not tracked here) and was re-measured against the
R2 tool with `scratchpad/claimlint/r2/measure.py`; the per-specimen table is in
the R2 build notes. It is reported because it is held out, and flagged here
because it is not reproducible from this repo alone.

**The miss class is ordinary, not exotic.** The R1 record named three exotica
(an `each`, a fabricated N-of-M, a negated-change claim), which reads as "the rest
is caught". Measured on the held-out set, **5 of 6 misses contain no trigger
word at all**: a wrong rate ("wrong on roughly half of boots" — observed once in
nine), a wrong "its only caller", a wrong "byte-for-byte", a wrong "mirroring
`sys_fork_with_parent_context`'s own ordering" citation, and a bare causal
assertion ("it depends on the UEFI memory map ... not on anything in the kernel
binary"). A shape linter cannot reach a false sentence that uses none of its
flagged vocabulary, and that is the **majority** of what it misses. A clean run
means "no bare absolutes shipped in this diff"; it does not mean the prose is
right, and a reviewer should not treat it as a substitute for reading the claims.

The remaining structural misses, each measured:

- **One citation inoculates every claim inside the same bullet.** Corpus F4 (a
  false "x86 beast battery: **PROVEN, PASS**" in a bullet whose next sentence
  carries a true `1/1 boot tests passed`) and F16 (a false "demonstrated proof"
  in a bullet that also says "the mutation was reverted") both miss this way, as
  does held-out I8 (a true `20/20` and a false "zero oracle failures in sixteen
  beast x86 boots" **in the same sentence**). R2's list-item break does not reach
  any of them — none has a list boundary between the citation and the claim.
  Fixing it needs a rule that pairs one citation to one claim — a different
  design with its own noise profile. R2 considered it and did not build it.
- **A count in correct N-of-M form that was never actually produced** (fabricated
  test execution reported as `8/8`, `5/5` — numbers equal to the `#[test]` counts
  in the files, not to any run) is invisible to a shape check.
- **A negated-change claim** ("no fix-round change touched its inputs", when each
  of its inputs had moved). Adding a `no <noun>` trigger does catch that one
  specimen; it was measured and rejected — see the vocabulary table.
- **`each`** is a real universal quantifier, deliberately out of the trigger list:
  adding it raised ~20 new raw sites in a single 611-line evidence doc for one
  additional historical catch.
- **Python docstrings are not scanned.** The comment extractor reads `#` lines,
  not triple-quoted strings, so a claim in a module or function docstring is
  invisible — including several in `claim-lint.py`'s own docstring.

### Surfaces it cannot see

`claim-lint.py` in diff mode reads tracked repo files. **6 of 18 of its own
calibration specimens (33%) shipped somewhere it does not look**: F2 in a GitHub
issue closing comment; F5/F6/F7/F17 in `scratchpad/gbus/*.md`, which is not in
this repo; F14 in bytes that were reworded before landing (`git log --all -S` on
four fragments of it returns only this branch's own corpus file). The assessment
that motivates this tool names **PR bodies** as one of the four surfaces the false
claims shipped on, and a PR body is not a tracked file either. That is why step 2
of the round checklist is an explicit `--files` run over the PR body and the
scratchpad evidence docs; without it, a third of the target class is out of reach
by construction.

### False positives, measured both ways

<!-- claim-lint:ok: the per-document figures come from
     scripts/test_claim_lint.py's RealDocumentReportTests and the per-round
     figures from PerRoundLoadReportTests, both re-run every time that suite
     runs -- not a one-off manual count. -->
**Per document (the number a doc author sees on an existing file).** Run against
two real, already-reviewer-verified-correct evidence docs in this tree
(`WORKLOAD-ENVELOPES.md`, `tty/EVIDENCE-2026-08-30.md`), the tool flags **74 of
212 paragraphs (35%)** — 86 findings — and **74 of the 100 paragraphs that use a
trigger word at all (74%)**. Command: `python3 scripts/test_claim_lint.py -v`,
`RealDocumentReportTests`.

That is **worse than R1's published 29% (48 of 167)**, and it is worse on purpose:
R2 splits merged bullet lists into separate paragraphs (167 → 212 paragraphs, and
a bullet can no longer borrow its neighbour's citation) and stops accepting cited
artifacts that do not exist. Attributed, by running each change alone:

| Variant | Paragraphs | Flagged | % | Findings |
|---|---|---|---|---|
| R1 as committed (`62d37564`) | 167 | 48 | 29% | 59 |
| + resolving-path exemption only (M1) | 167 | 52 | 31% | 63 |
| + list-item paragraph break only (M3) | 212 | 70 | 33% | 80 |
| R2 as shipped (both, + `prove/proved/proving/demonstrated`) <!-- claim-lint:ok: row of a measurement table, not a proof claim; produced by RealDocumentReportTests in scripts/test_claim_lint.py --> | 212 | 74 | 35% | 86 |

**Per round (the number that decides whether anyone keeps running it).** Three
real fix rounds from this campaign, replayed at their own commits. Command:
`python3 scripts/test_claim_lint.py -v`, `PerRoundLoadReportTests`.

| Round | Text files | `--whole-file` | shipping default (changed hunks) |
|---|---|---|---|
| `aa5f0fd8` (#721 fix round) | 7 | 282 | **8** |
| `a6679e7c` (sweep-3 fix round) | 6 | 84 | **18** |
| `9a77c3dc` (#748 fix round) | 3 | 86 | **13** |

The R1 record claimed the intended load was "a handful, not 60-100" while the
tool linted whole files; measured, whole-file mode hands an author who touched
three lines of `context_switch.rs` a wall of findings on prose they did not
write, and the predictable response is bulk annotation or abandonment — after
which a clean run carries no information. With hunk scoping the real per-round load is
8–18 findings, which is the load the checklist above assumes.

Most of the flagged paragraphs in the per-document run *are* well-evidenced —
via a `script:line (current tree)` pointer, or a citation one paragraph away,
neither of which this tool accepts, on purpose: loosening either re-opens the
miss class the source-path decision exists to prevent. Treat a flagged run on a
large existing doc as a backlog of one-line annotations, not as 74 new defects.

## Calibrating further

<!-- claim-lint:ok: every rule maps to an entry in
     scripts/claim_lint_corpus/historical_false_claims.json, and the R2
     vocabulary decisions below were each measured with scripts/claim-lint.py
     variants over the two real docs. -->
Every rule above exists because of a specific, quoted false claim. If you find a
new recurring false-claim shape this tool misses, add it to
`scripts/claim_lint_corpus/historical_false_claims.json` with the verbatim quote,
the file:line/issue it shipped in, the review that caught it and — if its bytes
are recoverable — the commit, so the in-context layer can test it. Then build
detection for that shape.

Don't add trigger words on spec. R2 measured five candidate additions against the
two real docs and the two catch sets, and kept one:

| Candidate | Added catches | Added flagged paragraphs (of 212) | Verdict |
|---|---|---|---|
| `prove`, `proved`, `proving`, `demonstrated` <!-- claim-lint:ok: row of a measurement table, not a proof claim; numbers from the variant runs recorded in scratchpad/claimlint/r2 and reproducible with scripts/test_claim_lint.py --> | 0 | 0 (+2 findings) | **kept** — conjugations of an already-calibrated rule word, not a new claim shape, and free |
| `verified`, `shown` | 0 | 0 (+5 findings) | dropped — no catch, and "verified" is this campaign's most common honest word ("reviewer-verified") |
| `not a single`, `100%` | 0 | 0 (+0 findings) | dropped — no specimen in the corpus uses either; a rule with no calibrating quote is a rule on spec |
| `impossible`, `cannot` | 0 | +7 | dropped — pure cost |
| `no <noun>` | +1 (corpus F18) | +9 | dropped — of the 10 new hits sampled, 9 are not claims at all ("write `hello` with no newline", "a PTY read with no data parks the thread", "no opinion on whether"). It catches F18 on the words "no fix-round", not on the negation, so it would not generalise to the negated-change class it appears to close. Keeping it would have restored the headline to 10/12 (83%) — the same number R1 published — which is a reason to be more suspicious of it, not less |
