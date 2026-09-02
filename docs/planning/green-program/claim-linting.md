# Claim linting — running scripts/claim-lint.py

<!-- claim-lint:ok: 2 commands produce every number in this doc and each is
     printed beside its number -- `python3 scripts/test_claim_lint.py -v` for
     the catch rates, the per-document counts and the per-round counts, and
     the R2/R3 build notes for the held-out set and the attribution tables. -->
**R1, 2026-09-01. R2, 2026-09-01. R3, 2026-09-01** — every number below was
re-derived after the R3 changes, and the command that produced it is printed
next to it. Where a number got worse, the worse number is the one printed.

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
changed** (`--changed-only`, the default). That matters: measured over eleven
real fix rounds, whole-file linting produced 7–282 findings against 1–36 in
changed hunks, suppressing 0%–97% of the load per round (table below).
`--whole-file` restores the old behaviour when you want to audit a file end to
end.

The base is resolved to `git merge-base <ref> HEAD` and then diffed against the
**working tree**: a stale local `main` does not drag in files the branch did not
touch, and uncommitted edits are still linted before you commit them. New files
that have not been `git add`ed yet are included too, and linted whole — `git
diff` cannot see an untracked file, which is what a not-yet-added
`EVIDENCE-*.md` is (review R2-M2, closed by
`test_untracked_new_file_is_linted_in_diff_mode`). Ignored files are not.

<!-- claim-lint:ok: the file-extension allowlist and the capture-skip predicate
     are `TEXT_EXTENSIONS` and `is_capture_file()` in scripts/claim-lint.py,
     with test_captured_serial_txt_is_skipped,
     test_hand_authored_prose_under_a_serials_dir_is_linted and
     test_a_source_dir_named_serial_is_not_a_capture_tree in
     scripts/test_claim_lint.py. -->
Exit code 0 = clean, 1 = un-discharged findings, 2 = usage error. It checks `.md`,
`.rs`, `.sh`, `.py`, `.txt` files: markdown prose (including table cells and
blockquotes) and `//`/`///`/`//!`/`#` comment blocks in source and scripts. It
does not touch code, and it does not fire outside a comment or a doc.

### What is skipped under `serials/` and `confirm/`, and what is not (R3)

A **machine-emitted capture** is skipped: nobody can discharge a claim written
inside a serial a gate script printed. That is scoped by **file extension**, not
by directory. Under a `serials/` or `confirm/` path component the tool skips
`.txt` and `.log` — **419 tracked files here (403 `.txt`, 16 `.log`)** — and
lints everything else in those trees: **30 tracked files (22 `.md`, 7 `.sh`,
1 `.rs`), carrying 124 findings**. Those are PROVE narratives, RCA write-ups,
per-arc READMEs and mutation apply/revert scripts — prose a human wrote and can
discharge. Commands:

```bash
git ls-files | grep -E '(^|/)(serials?|confirm)/' | grep -Ev '\.txt$|\.log$'   # the 30
python3 scripts/test_claim_lint.py -v                                          # per-round table
```

<!-- claim-lint:ok: both numbers come from replaying that round under each
     predicate; the harness is PerRoundLoadReportTests in
     scripts/test_claim_lint.py and the side-by-side run is in the R3 build
     notes -->
R2 skipped by directory alone, and the cost was not hypothetical (review R2-B1):
the `cbc6873b` round in the per-round table below — the round whose whole subject
was archiving dangling evidence citations — reported **0 findings in its changed
hunks under the R2 predicate and 19 under this one**, because the round's own
PROVE narrative lived under `serials/`. A round could record `claim-lint … ->
exit 0` while the document making its claims was not read at all. The directory
pattern is also plural-only and matched as a whole path component, so
`kernel/src/serial/` — a source directory whose only relationship to the rule
was its name — is not a capture tree.

## The round checklist — the run is an artifact

This repo has no CI and no pre-commit hook, so the run does not happen unless a
person makes it happen. An unchecked instruction is the thing this tool exists to
replace, so the run itself is recorded as evidence, exactly like a gate serial.

**The requirement lives in `CLAUDE.md`, not here (R3).** R2 wrote the checklist
into this document, and `git grep claim-lint` outside the tool's own five files
returned no other reference — an instruction inside a file nobody is required to
open, which is the construction this tool's own thesis rejects (review M5).
`CLAUDE.md` is loaded as this repo's standing instructions at the start of a
session, and its "Claim Discipline" section now carries the two lines below
and the rule that a round without them is a finding. This document stays the
reference for what the tool reaches and what it does not.

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
| `artifact-path-missing` | a preserved/attached/committed/saved/written/**archived**-at claim naming a backticked path | the cited path must resolve to a **file** on disk (checked relative to the citing file's own directory, then the repo root) |

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
- **A cited artifact that does not exist cannot exempt anything (R2), and a
  cited *directory* is not an artifact (R3).** Before R2, an exempting path was
  accepted on shape alone while the `artifact-path-missing` rule right next to
  it checked the filesystem. Corpus F2 is precisely a cited serial path that did
  not exist; phrased as "see `<that path>`" it used to *silence* the rule
  instead of tripping it. Both now resolve through the same check. R2 then left
  a weaker version of the same hole: `os.path.exists` is true for a folder, so
  "Every serial and gate verdict referenced here is in `serials/`" cleared its
  whole paragraph by naming the folder beside the doc (review R2-M1, live at
  `tty/EVIDENCE-2026-08-30.md:5`, which R3 now flags). A resolution has to name
  a file. Cost, measured: a true claim citing a log that was never committed is
  now flagged (corpus T1 is exactly that, and moved from "clean" to "needs an
  annotation").
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

**A bare `claim-lint:ok` does not silence a finding (R2), and neither does one
whose text cites no openable thing (R3).** The annotation has to be followed by
text containing at least one of: an N-of-M count, an issue or PR number, a review
**file** (`fix2-review.md`), a named test function (`test_…`), or a path that
resolves on disk. Before R2 the doc demanded a citation and the code accepted
`claim-lint:ok: lol`. R2 then accepted the bare English word "review", so
`claim-lint:ok: see the review` cleared a claim without naming anything a reader
could open (review r2-m2). Tightening it made this very paragraph fire: in its R2
wording it contained the literal marker and the phrase "a review reference", and
was therefore discharging itself — the same self-silencing shape R2 found in its
own rules table.

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
are recoverable (75%); 9 of those 9 catches fire on a rule whose trigger word is
inside the false sentence itself.** Command:
`python3 scripts/test_claim_lint.py -v` →
`HistoricalCorpusInContextTests`, which recovers each sentence's file with
`git show <shipped_commit>:<shipped_path>`, lints the whole file, and counts a
specimen caught only when a finding's own paragraph contains the offending
sentence. Before R2 this was 8 of 12 (67%); R3 did not move it.

The second half of that sentence is R3's answer to a review question about
whether the catches are incidental — whether the tool flags the paragraph
because of some *other* sentence in it. Measured per specimen (the `on-claim` /
`incidental` column in the same test output): on 9 of 9 caught specimens at
least one reported rule fires on vocabulary inside the false sentence. Four of them
(F8, F9, F10, F13) also draw a second, incidental finding on a neighbouring
sentence of the same paragraph, and on those the *first* finding listed quotes
the neighbour's trigger — which is what makes the catches look incidental if you
read only one finding per paragraph. The author sees all of them, at the same
`file:line`.

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

A **second** held-out set of 11 specimens, from six further review arcs
(`sweep2`, `gtracing`, `gttyx86`, `p540`, `coreproof/rung2`, `r4`), was built and
run independently by the R2 review slot against the R2 tool: **6 of 11 (55%)**,
identically in the shipping default and whole-file modes. It sits where the
first held-out figure predicts and it is the least flattering of the three
numbers on this page. It is likewise not reproducible from this repo. Folding
both held-out sets into `historical_false_claims.json` behind a `heldout: true`
flag would make all three figures re-derivable by
`python3 scripts/test_claim_lint.py`; R3 did not do it, because the specimen
bytes and the review files that justify each one live outside this repo and
copying only the sentences would reproduce the isolated-quote layer's mistake.

**The miss class is ordinary, not exotic, and it has two comparable halves.**
The R1 record named three exotica (an `each`, a fabricated N-of-M, a
negated-change claim), which reads as "the rest is caught". It is not. Misses
split into **vocabulary** misses (the false sentence uses no flagged word, so no
rule can reach it) and **exemption** misses (a trigger word *is*
present, and a same-paragraph N-of-M, resolving path or mutation keyword
silences it). Which half dominates depends on the document:

| Miss set | n | Vocabulary | Exemption |
|---|---|---|---|
| R1 held-out set (13 specimens, R2 build notes) | 6 | 5 | 1 |
| R2 review's own held-out set (11 specimens, R2 review §"THE PASS/FAIL TEST") | 5 | 1 | 4 |
| In-corpus, in context (F4, F16, F18) | 3 | 1 | 2 |
| Pooled | 14 | 7 | 7 |

R2 published the first row's split as a property of the tool ("that is the
**majority** of what it misses"); the R2 review measured the second row and it
came out the other way (review R2-M5). Neither set is reproducible from this
repo — both were assembled in review scratchpads — so the pooled row is quoted,
not re-derived, and the third row is the only one `python3
scripts/test_claim_lint.py -v` prints. The lever the numbers actually point at
is the exemption half, which is the rule R2 and R3 both declined to build; see
"Deferred: pairing one citation to one claim" below. A clean run means "no bare
absolutes shipped in this diff"; it does not mean the prose is right, and a
reviewer should not treat it as a substitute for reading the claims.

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
- **`artifact-path-missing` only reads "verb, then path" (R3).** Adding the verb
  `archived` closes the shape where that verb introduces a backticked path, and
  is regression-tested by
  `test_archived_at_a_missing_path_is_flagged`. Measured against the specimens
  that motivated it, it does **not** reach either of sweep2 B1's two dangling
  in-repo citations, because in both the verb follows the path rather than
  introducing it: ``per-suite logs: `serials/suite-*.log` (archived durably
  in-repo alongside this doc)`` (`nic-bus/CONFIRM-2026-08-31.md:211`
  @`cbc6873b`) and "only the files actually cited by EVIDENCE/CONFIRM's own text
  were archived" (`nic-bus/serials/fix2-prove.md:23` @`cbc6873b`, a file R3 also
  brings back into scope). On the first of those the tool *does* compute that
  the citation dangles — `dangling_evidence_paths()` returns it — and then
  stays silent, because an unrelated "2 of 10" earlier in the same bullet
  exempts the universal. Promoting a dangling citation to a finding of its own,
  independent of what else exempts the paragraph, is the obvious repair and is
  measured under "Deferred" below.
- **The per-round replay resolves cited paths against the *current* tree.**
  `PerRoundLoadReportTests` lints each round's file bytes as of that commit but
  checks path citations against the checkout it runs in, because resolving them
  as of the commit would need a worktree per round. A path archived after the
  round therefore resolves when it did not at the time, and vice versa. The
  effect is small and both directions occur; it is why the R2 review's live-CLI
  numbers for `cbc6873b` (3 findings in changed hunks) and this harness's
  replay (0 under the same predicate) differ.

### Deferred: pairing one citation to one claim

Three related changes were identified, measured, and **not** shipped in R1, R2 or
R3. They are one design problem, they are the biggest measured miss class, and
each needs a calibration round of its own rather than a bolt-on. Filed so the
decision is reopened on evidence, not forgotten:

1. **One citation exempts every claim in the paragraph.** The exemption search
   asks "does *this paragraph* carry an N-of-M, a resolving path, or a mutation
   keyword anywhere", so a true count in one clause clears a false absolute in
   another. Specimens it would reach, each a review finding a human raised:
   corpus **F4** (`tty/EVIDENCE-2026-08-30.md` §7 @`cd77e41c` — false "x86 beast
   battery: PROVEN, PASS" beside a true `1/1 boot tests passed`), corpus **F16**
   (`coreproof/rung2/PROVE-2026-08-30.md` @`28263b11` — "direct, demonstrated
   proof" in a bullet that also says the mutation was reverted), held-out
   **I8** (`sockets/EVIDENCE-2026-08-29.md:254` @`cfde4768` — a true `20/20` and
   a false "zero oracle failures in sixteen beast x86 boots" **in the same
   sentence**), and held-out **K8**/**K9** from the R2 review's set (a 1,683-char
   bullet exempted by a true `13/13`; a tautological non-vacuity gate exempted
   by `50/50`, `1/1` and a resolving path). R2's list-item paragraph break
   reaches none of them: no list boundary exists between the citation and the
   claim.
2. **The mutation exemption is paragraph-wide** (`PROVEN_EVIDENCE_RE`). Any
   `revert`/`redden`/`mutation` anywhere in a bullet clears the `proven` rule for
   the whole bullet.
   This is the *measured* cause of F16's in-context miss, and it is why
   `tty/EVIDENCE-2026-08-30.md:5` still passes its `prove` after R3 flags its
   universal — the word "mutations" earlier in the sentence exempts it.
3. **A dangling citation is computed and then discarded** unless some other rule
   fires. Promoting it to a finding of its own was built as a variant and
   measured: it reaches the sweep2 B1 specimen R3 could not
   (`nic-bus/CONFIRM-2026-08-31.md:211` @`cbc6873b`) and raises the in-context
   corpus catch from 9 of 12 to 11 of 12 — but F16 and F18 flip on a dangling
   `.txt` cited elsewhere in their paragraph, not on their own words, and on the
   two verified-true docs it adds 13 findings over 8 paragraphs (75 → 83 flagged
   of 212, 87 → 100 findings) of which **11 of 13 are citation shorthand, not
   false claims**: brace expansions (`serials/…-boot{1,2,3}-20260830.txt`),
   ellipses (`…/scratchpad/assessment/`), and eight table rows naming a serial
   by bare filename when it sits one directory down. Shipping it would buy a
   headline with noise nobody would keep running.

What all three need is a rule that binds a citation to the claim it supports —
sentence-scoped exemption at minimum, and probably "this paragraph makes more
absolutes than it makes citations". That is a different tool with a different
noise profile. It needs its own corpus round: the calibration question is what
counts as a claim–citation pair in a table row, a bullet with a trailing
"per `…`", and a sentence carrying two counts.

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
(`WORKLOAD-ENVELOPES.md`, `tty/EVIDENCE-2026-08-30.md`), the tool flags **75 of
212 paragraphs (35%)** — 87 findings — and **75 of the 100 paragraphs that use a
trigger word at all (75%)**. Command: `python3 scripts/test_claim_lint.py -v`,
`RealDocumentReportTests`.

That is **worse than R1's published 29% (48 of 167)**, and it is worse on purpose:
R2 splits merged bullet lists into separate paragraphs (167 → 212 paragraphs, and
a bullet can no longer borrow its neighbour's citation) and stops accepting cited
artifacts that do not exist; R3 stops accepting a cited *directory*. Attributed,
by running each change alone:

| Variant | Paragraphs | Flagged | % | Findings |
|---|---|---|---|---|
| R1 as committed (`62d37564`) | 167 | 48 | 29% | 59 |
| + resolving-path exemption only (M1) | 167 | 52 | 31% | 63 |
| + list-item paragraph break only (M3) | 212 | 70 | 33% | 80 |
| R2 as shipped (both, + `prove/proved/proving/demonstrated`) <!-- claim-lint:ok: row of a measurement table, not a proof claim; produced by RealDocumentReportTests in scripts/test_claim_lint.py --> | 212 | 74 | 35% | 86 |
| R3 as shipped (+ a cited directory no longer resolves) <!-- claim-lint:ok: row of a measurement table, not a proof claim; produced by RealDocumentReportTests in scripts/test_claim_lint.py --> | 212 | 75 | 35% | 87 |

The one paragraph R3 adds is `tty/EVIDENCE-2026-08-30.md:5`, the review's own
R2-M1 specimen. Its `prove` is *not* newly flagged: the word "mutations" in the
same sentence still exempts that rule, which is deferred item 2 above.

**Per round (the number that decides whether anyone keeps running it).** Eleven
real fix rounds from this campaign, replayed at their own commits — the three R2
chose plus the eight the R2 review measured independently. Command:
`python3 scripts/test_claim_lint.py -v`, `PerRoundLoadReportTests`.

| Round | Text files | `--whole-file` | shipping default (changed hunks) |
|---|---|---|---|
| `aa5f0fd8` (#721 fix round) | 7 | 282 | **8** |
| `a6679e7c` (sweep-3 fix round) | 8 | 91 | **25** |
| `9a77c3dc` (#748 fix round) | 3 | 86 | **13** |
| `6ba3bcc4` (R4 doc fix round) | 2 | 66 | **36** |
| `2a2328aa` (coreproof rung-2 prove) <!-- claim-lint:ok: row of a measurement table; the label names the round's document, it does not claim a proof -- numbers from PerRoundLoadReportTests in scripts/test_claim_lint.py --> | 15 | 29 | **29** |
| `73c58fda` (#540 x86 prod gate) | 2 | 141 | **24** |
| `16d6ff5b` (x86 TTY oracle gate) | 2 | 64 | **12** |
| `cbc6873b` (nic-bus doc-truth) | 10 | 58 | **19** |
| `1f098d11` (tracing x86 evidence) | 1 | 7 | **2** |
| `06a1c1a6` (TTY x86 fix-round) | 1 | 22 | **2** |
| `5777bb7b` (#717 trap guard) | 1 | 31 | **1** |

The R1 record claimed the intended load was "a handful, not 60-100" while the
tool linted whole files; measured, whole-file mode hands an author who touched
three lines of `context_switch.rs` a wall of findings on prose they did not
write, and the predictable response is bulk annotation or abandonment — after
which a clean run carries no information.

**R2 then published "the real per-round load is 8–18 findings" off its own three
rounds, and that does not survive eight more (review R2-M4).** Measured across
**n = 11** rounds: the shipping default is **1–36 findings** (five above 18)
and whole-file is **7–282**. The claim the record can carry is the comparison,
not the range: hunk scoping suppressed **0%–97%** of the whole-file load per
round, and reduced it on 10 of the 11 (`2a2328aa` is the exception — 14 of its
15 files are newly added, and a new file's findings sit in changed hunks by
construction). Two rounds moved between R2 and R3 because their prose lived
under `serials/`: `a6679e7c` (18 → 25) and `cbc6873b` (0 → 19).

Most of the flagged paragraphs in the per-document run *are* well-evidenced —
via a `script:line (current tree)` pointer, or a citation one paragraph away,
neither of which this tool accepts, on purpose: loosening either re-opens the
miss class the source-path decision exists to prevent. Treat a flagged run on a
large existing doc as a backlog of one-line annotations, not as 75 new defects.

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
