#!/usr/bin/env python3
"""
claim-lint.py -- mechanical detector for the "claim discipline" violations that
made up 56% of blocking review findings in the 2026-08/09 green-program campaign
(37 of 66 blockers, per ~/Downloads/breenix-progress-assessment-2026-09-01.html):
factually wrong, checkable sentences shipped in EVIDENCE/CONFIRM docs, PR bodies,
gate-script headers and source comments -- not code defects.

This tool does not know whether a claim is TRUE. It cannot execute a gate or
replay a mutation. What it catches is a SHAPE: an unquantified absolute
("every", "zero", "airtight", "guaranteed", "structurally"), a "proven" with no
named mutation next to it, an "observed live" with no artifact path next to it,
or a "preserved/attached/committed at <path>" citing a path that does not exist
in the tree. Every rule below was reverse-engineered from a verbatim quote in
the review corpus -- see scripts/claim_lint_corpus/historical_false_claims.json
for the specimen each rule was built to catch, and docs/planning/green-program/
claim-linting.md for how to run this, what it measurably does not reach, and how
to discharge a hit honestly.

Usage:
    scripts/claim-lint.py                       # changed HUNKS vs origin/main
    scripts/claim-lint.py --base main
    scripts/claim-lint.py --whole-file          # diff mode, but whole files
    scripts/claim-lint.py --files a.md b.rs     # explicit files, always whole
    scripts/claim-lint.py --all                 # every tracked text file (slow)
    scripts/claim-lint.py --commit-msg <file>   # a commit message, as prose
    scripts/claim-lint.py --format json ...

`--commit-msg <file>` is its own mode (see lint_commit_msg_text() below): it
lints the named file as prose with every rule, and it additionally scans
auto-close-keyword against the UNFENCED text, so the phrase cannot hide from
this rule inside a ``` example the way it can inside a real doc's fenced
code. `scripts/lint-commit-msg.sh <file>` wraps this for `git commit -F`
workflows and for a `.git/hooks/commit-msg` hook; see
docs/planning/green-program/claim-linting.md's `--commit-msg` section.

Diff mode reports only findings whose paragraph overlaps a line this branch
actually changed (`--changed-only`, the default; `--whole-file` restores the
old behaviour). The base is resolved to `git merge-base <ref> HEAD`, so a stale
local `main` does not drag in files the branch never touched. New files that
are not yet `git add`ed are included too, whole -- a brand-new EVIDENCE doc is
exactly the shape `git diff` cannot see.

Discharge: an author who has genuinely checked a strong claim marks it in the
same paragraph with a `claim-lint:ok:` annotation that NAMES the artifact --
an N-of-M count, a path that resolves on disk, an issue number, or a review
file. A bare `claim-lint:ok` with nothing after it does not silence anything:

    <!-- claim-lint:ok: 12/12 arms -- review-baseline.log -->
    every arm passed the baseline run.

    // claim-lint:ok: mutation-proven, see scripts/test_claim_lint.py
    // every close path decrements the refcount.

One rule is the exception: `auto-close-keyword` flags a close/fix/resolve
keyword sitting directly in front of "#N", which GitHub reads and acts on
(auto-closing the referenced issue) independently of anything a human writes
next to it. A `claim-lint:ok` annotation cannot discharge that finding --
see check_auto_close_keyword()'s docstring below and
docs/planning/green-program/claim-linting.md.

Exit codes: 0 = clean, 1 = un-discharged findings, 2 = usage/internal error.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass, field
from typing import Optional

REPO_ROOT = subprocess.run(
    ["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True
).stdout.strip() or os.getcwd()

TEXT_EXTENSIONS = {".md", ".rs", ".sh", ".py", ".txt"}

# Captured artifacts are evidence, not prose: nobody can "discharge" a claim
# written inside a serial log a gate script emitted. The skip is scoped by
# EXTENSION, not by directory (review R2-B1): `serials/` and `confirm/` trees
# in this repo also hold hand-authored PROVE narratives, RCA write-ups,
# per-arc READMEs and mutation scripts -- prose a human wrote and can
# discharge. Skipping by directory alone removed 30 such files from the tool --
# 124 findings, measured after this change; 123 under the R2 code -- including
# the file a blocking review finding of this campaign was raised against.
# The directory pattern is also plural-only and matched as
# a whole path COMPONENT, so `kernel/src/serial/` -- a source directory whose
# only relationship to the rule was its name -- is not a capture tree.
CAPTURE_DIR_RE = re.compile(r"(^|/)(serials|confirm)/", re.IGNORECASE)
CAPTURE_EXTENSIONS = {".txt", ".log"}


def is_capture_file(rel: str) -> bool:
    """True for machine-emitted captures only: a `.txt`/`.log` file that lives
    under a `serials/` or `confirm/` path component. A `.md`, `.sh`, `.rs` or
    `.py` file under the same tree is hand-authored prose and IS linted."""
    if os.path.splitext(rel)[1].lower() not in CAPTURE_EXTENSIONS:
        return False
    return bool(CAPTURE_DIR_RE.search(rel))

# A `claim-lint:ok` marker plus whatever the author wrote after it, to the end
# of the paragraph. The marker on its own does not silence a finding: the text
# that follows it has to carry a citation (see discharge_citation()).
DISCHARGE_RE = re.compile(r"claim-lint:\s*ok\b[:\s-]*", re.IGNORECASE)

# ---------------------------------------------------------------------------
# Rule vocabulary. claim-lint:ok: traceable one-for-one against
# scripts/claim_lint_corpus/historical_false_claims.json's `expected_rules`;
# see that file for the verbatim quote each word was added to catch. Do not
# add words on spec; add a specimen to the corpus first (calibrate against
# the real thing) -- see the "each" decision in claim-linting.md for what
# happens when you don't, and the R2 vocabulary table for the words that were
# measured and dropped.
# ---------------------------------------------------------------------------

UNIVERSAL_WORDS = [
    "every", "all", "none", "zero", "always", "never", "nothing", "nowhere",
    "wholly", "entirely", "completely", "fully", "exclusively", "invariably",
]
# Idioms that use one of the words above without making an empirical universal
# claim. Matched as whole lowercase phrases against the paragraph text.
UNIVERSAL_IDIOM_EXEMPT = [
    "at all", "after all", "all the way", "once and for all",
    "all of a sudden", "not at all", "first of all", "all right",
]
UNIVERSAL_RE = re.compile(
    r"\b(" + "|".join(UNIVERSAL_WORDS) + r")\b(?!-\w)", re.IGNORECASE
)
PROVEN_RE = re.compile(
    r"\b(proven|proves|prove|proved|proving|proof|demonstrated)\b", re.IGNORECASE
)
PROVEN_EVIDENCE_RE = re.compile(
    r"\b(mutat\w*|revert\w*|redden\w*|falsif\w*|counter-?example\w*|"
    r"reproduc\w*|regression|--boots|bisect\w*)\b",
    re.IGNORECASE,
)

LIVE_CLAIM_RE = re.compile(r"\b(observed|confirmed)\s+live\b", re.IGNORECASE)

ABSOLUTE_GUARANTEE_RE = re.compile(
    r"\b(airtight|guarantee[ds]?|structurally)\b", re.IGNORECASE
)

# claim-lint:ok: this comment describes the exemption logic the code right
# below it implements; see test_evidence_log_path_clears_a_universal,
# test_source_path_does_not_clear_a_universal and
# test_nonexistent_evidence_path_does_not_clear_a_universal in
# scripts/test_claim_lint.py.
# A citation that counts as "evidence attached in this paragraph" for the
# UNIVERSAL / PROVEN / LIVE_CLAIM / ABSOLUTE_GUARANTEE rules: either a proper
# N-of-M count, or a path that (a) looks like a captured log/serial rather
# than a bare source-code pointer -- source paths name the code under
# discussion, they do not establish the claim, see gtty fix2-review
# BLOCKING 1 -- and (b) RESOLVES ON DISK. A cited artifact that does not
# exist is the corpus's own F2 specimen; it cannot exempt anything.
NM_COUNT_RE = re.compile(r"\b\d+\s*(?:/|of)\s*\d+\b", re.IGNORECASE)
EVIDENCE_PATH_RE = re.compile(
    r"[`\"]([^`\"\s]*(?:serials?/|confirm/|scratchpad/)[^`\"\s]*"
    r"|[^`\"\s]+\.(?:log|txt))[`\"]",
    re.IGNORECASE,
)

# Citations that make a `claim-lint:ok:` annotation a citation rather than a
# mute button: an N-of-M count, an issue or PR number, a review FILE, a named
# test function, or a path -- path tokens are additionally required to resolve
# on disk (see discharge_citation()). What the accepted forms have in common
# is that a reader can open the thing they name.
#
# R3 (review r2-m2): the bare English word "review" used to discharge, so
# `claim-lint:ok: see the review` silenced a claim without naming anything a
# reader could open. A review reference now has to be a filename.
DISCHARGE_ISSUE_RE = re.compile(r"#\d{2,}\b")
DISCHARGE_REVIEW_RE = re.compile(r"\b[\w.-]*review[\w.-]*\.md\b", re.IGNORECASE)
DISCHARGE_TEST_RE = re.compile(r"\btest_\w{4,}\b")
DISCHARGE_PATH_RE = re.compile(
    r"\b([\w./-]+\.(?:log|txt|md|json|rs|sh|py|toml|S))\b"
)

# Matches "preserved/attached/committed/saved/written/archived" + "at/to/in" +
# a backtick-quoted path -- a mechanically checkable claim (gtty review.md B4:
# the cited path did not exist on the branch). Captures the path following
# the verb so its existence can be checked on disk. `archived` was added in R3
# because it is the verb both of sweep2 B1's dangling in-repo citations use;
# see the R3 note in claim-linting.md for what that did and did not reach.
ARTIFACT_CLAIM_RE = re.compile(
    r"\b(preserved|attached|committed|saved|written|archived)\s+(?:at|to|in)\b"
    r"[^`]{0,40}`([^`]+)`",
    re.IGNORECASE,
)

# A different kind of rule from the five above: those look for a CLAIM shape
# that might be false. This one looks for a GitHub *mechanism* trigger -- a
# close/fix/resolve keyword bound to "#N" is not a claim to be right or wrong
# about, it is text GitHub itself parses and acts on. GitHub auto-closes the
# referenced issue the moment a PR body or a commit message on the default
# branch carries one of these keywords immediately before "#N" (see GitHub's
# "linking a pull request to an issue" docs, which also documents the
# cross-repo "OWNER/REPO#N" form and a full issue/PR URL as equivalent
# triggers; "GH-N" is the legacy autolink form from GitHub's original
# closing-keywords announcement -- each of the three is matched below
# alongside a bare "#N"). The convention from here on is a plain "#N", not a
# close/fix/resolve keyword directly in front of it, so an explicit close
# stays a deliberate act rather than a side effect of prose. #737 auto-closed
# at the exact moment PR #799 (2026-09-05) merged, ahead of the round's own,
# later, explicit close -- see docs/planning/green-program/claim-linting.md
# for what is and is not recoverable about which mechanism actually fired.
AUTO_CLOSE_KEYWORD_RE = re.compile(
    r"\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\s*:?\s+"
    r"(?:[\w.-]+/[\w.-]+#\d+|"
    r"https?://github\.com/[\w.-]+/[\w.-]+/(?:issues|pull)/\d+|"
    r"GH-\d+|#\d+)\b",
    re.IGNORECASE,
)


@dataclass
class Finding:
    file: str
    line: int
    rule: str
    text: str
    detail: str = ""
    end_line: int = 0

    def __post_init__(self):
        if not self.end_line:
            self.end_line = self.line


@dataclass
class Paragraph:
    file: str
    start_line: int
    text: str
    end_line: int = 0

    def __post_init__(self):
        if not self.end_line:
            self.end_line = self.start_line


# claim-lint:ok: this design choice and its cost are measured, not asserted --
# see the "unit is the paragraph" section of docs/planning/green-program/
# claim-linting.md and RealDocumentReportTests in scripts/test_claim_lint.py
# for the actual counts, re-derived in R2.
# ---------------------------------------------------------------------------
# Extraction: turn a source file into a list of prose paragraphs worth
# checking. Granularity is the paragraph, not the sentence -- a single claim
# in this corpus is almost always one bullet, one blockquote, or one table
# row, and paragraph-level exemption search (does *this* paragraph carry a
# citation anywhere) matches how these docs actually cite evidence: often in
# a different clause of the same bullet than the trigger word.
#
# R2 change: a list item or an ATX heading STARTS a new paragraph even with no
# blank line before it. Markdown-wise a hanging-indent bullet list is one
# block; claim-wise it is one claim per bullet, and merging them let a count
# in bullet 3 exempt an absolute in bullet 1 (corpus F1, review B1/M3).
# ---------------------------------------------------------------------------

FENCE_RE = re.compile(r"^\s*(```|~~~)")
TABLE_ROW_RE = re.compile(r"^\s*\|.*\|\s*$")
TABLE_SEP_RE = re.compile(r"^\s*\|[\s:|-]+\|\s*$")
LIST_ITEM_RE = re.compile(r"^\s*(?:[-*+]\s+|\(?\d+[.)]\s+)")
HEADING_RE = re.compile(r"^\s{0,3}#{1,6}\s")
HTML_COMMENT_START_RE = re.compile(r"^\s*<!--")
# A block containing only HTML comments is an ANNOTATION, and an annotation
# belongs to what follows it, not to what precedes it. Without this, a
# `claim-lint:ok` written above a bullet silently discharges the PREVIOUS
# bullet (caught while dogfooding R2 on this tool's own doc).
COMMENT_ONLY_RE = re.compile(r"^(?:<!--.*?-->\s*)+$", re.DOTALL)


def extract_markdown_paragraphs(file: str, lines: list,
                                blank_fences: bool = True) -> list:
    # Blank out fenced code blocks (keep line count so numbers stay aligned).
    # `blank_fences=False` is used by lint_commit_msg_text()'s auto-close-
    # keyword pass ONLY: a commit message does not get markdown-rendered for
    # GitHub's issue-closing parser, so a ``` fence does not shield an
    # auto-close phrase from it the way it shields a real doc's code sample
    # from the claim-quality rules.
    scrubbed = list(lines)
    if blank_fences:
        in_fence = False
        for i, line in enumerate(lines):
            if FENCE_RE.match(line):
                in_fence = not in_fence
                scrubbed[i] = ""
                continue
            if in_fence:
                scrubbed[i] = ""

    paragraphs = []
    buf = []
    buf_start = [0]
    buf_end = [0]

    def flush():
        if buf:
            text = " ".join(s.strip() for s in buf if s.strip())
            if text:
                paragraphs.append(
                    Paragraph(file, buf_start[0], text, buf_end[0])
                )
        buf.clear()

    for i, raw in enumerate(scrubbed):
        lineno = i + 1
        stripped = raw.strip()
        if TABLE_SEP_RE.match(raw):
            continue
        if TABLE_ROW_RE.match(raw):
            # Each table row is its own paragraph (precise line reporting).
            flush()
            cell_text = stripped.strip("|")
            if cell_text.strip():
                paragraphs.append(Paragraph(file, lineno, cell_text, lineno))
            buf_start[0] = lineno + 1
            continue
        if not stripped:
            flush()
            buf_start[0] = lineno + 1
            continue
        if (LIST_ITEM_RE.match(raw) or HEADING_RE.match(raw)
                or HTML_COMMENT_START_RE.match(raw)):
            # A new bullet / heading / annotation block is a new paragraph,
            # blank line or not.
            flush()
            buf_start[0] = lineno
        text = stripped
        if text.startswith(">"):
            text = text.lstrip(">").strip()
        if not buf:
            buf_start[0] = lineno
        buf.append(text)
        buf_end[0] = lineno
    flush()
    return attach_annotations_forward(paragraphs)


def attach_annotations_forward(paragraphs: list) -> list:
    """Fold a comment-only block into the paragraph that follows it."""
    out = []
    pending = []
    for p in paragraphs:
        if COMMENT_ONLY_RE.match(p.text.strip()):
            pending.append(p)
            continue
        if pending:
            p = Paragraph(
                p.file, pending[0].start_line,
                " ".join(q.text for q in pending) + " " + p.text,
                p.end_line,
            )
            pending = []
        out.append(p)
    out.extend(pending)
    return out


LINE_COMMENT_PREFIXES = {
    ".rs": ("///", "//!", "//"),
    ".sh": ("#",),
    ".py": ("#",),
}


def extract_comment_paragraphs(file: str, lines: list, ext: str) -> list:
    prefixes = LINE_COMMENT_PREFIXES.get(ext, ())
    paragraphs = []
    buf = []
    buf_start = [0]
    buf_end = [0]

    def flush():
        if buf:
            text = " ".join(s for s in buf if s)
            if text.strip():
                paragraphs.append(
                    Paragraph(file, buf_start[0], text, buf_end[0])
                )
        buf.clear()

    for i, raw in enumerate(lines):
        lineno = i + 1
        stripped = raw.strip()
        if lineno == 1 and stripped.startswith("#!"):
            flush()
            buf_start[0] = lineno + 1
            continue
        matched = None
        for p in prefixes:
            if stripped.startswith(p):
                matched = p
                break
        if matched is None:
            flush()
            buf_start[0] = lineno + 1
            continue
        content = stripped[len(matched):].strip()
        if not content:
            flush()
            buf_start[0] = lineno + 1
            continue
        if LIST_ITEM_RE.match(content):
            flush()
            buf_start[0] = lineno
        if not buf:
            buf_start[0] = lineno
        buf.append(content)
        buf_end[0] = lineno
    flush()
    return paragraphs


def extract_paragraphs(file: str, content: str, ext_override: str = None) -> list:
    lines = content.splitlines()
    ext = ext_override if ext_override is not None else os.path.splitext(file)[1]
    if ext == ".md" or ext == ".txt":
        return extract_markdown_paragraphs(file, lines)
    if ext in LINE_COMMENT_PREFIXES:
        return extract_comment_paragraphs(file, lines, ext)
    return []


# ---------------------------------------------------------------------------
# Path resolution -- shared by the exemption path and the artifact-path rule
# ---------------------------------------------------------------------------

def path_resolves(path: str, citing_file: str, repo_root: str) -> bool:
    """Does a cited path name a FILE that actually exists?

    Evidence docs cite a path either relative to their own directory
    (`serials/foo.txt` inside .../tty/EVIDENCE-*.md) or relative to the repo
    root. Both resolutions are accepted; a URL is not a filesystem citation
    and never resolves here.

    A directory does not resolve (review R2-M1). "Every serial referenced here
    is in `serials/`" used to clear a whole paragraph by naming the folder
    beside the doc -- a folder establishes nothing about the claim, which is
    the same principle that refuses a bare source path.
    """
    path = path.strip().rstrip(".,;:)")
    if not path or path.startswith(("http://", "https://")):
        return False
    if os.path.isabs(path):
        candidates = [path]
    else:
        doc_dir = os.path.dirname(os.path.join(repo_root, citing_file))
        candidates = [os.path.join(doc_dir, path), os.path.join(repo_root, path)]
    return any(os.path.isfile(c) for c in candidates)


def resolving_evidence_paths(p: Paragraph, repo_root: str) -> list:
    """Cited artifact paths in this paragraph that exist on disk."""
    return [m.group(1) for m in EVIDENCE_PATH_RE.finditer(p.text)
            if path_resolves(m.group(1), p.file, repo_root)]


def dangling_evidence_paths(p: Paragraph, repo_root: str) -> list:
    return [m.group(1) for m in EVIDENCE_PATH_RE.finditer(p.text)
            if not path_resolves(m.group(1), p.file, repo_root)]


# ---------------------------------------------------------------------------
# Rules
# ---------------------------------------------------------------------------

def discharge_citation(p: Paragraph, repo_root: str) -> Optional[str]:
    """The citation a `claim-lint:ok:` annotation carries, or None.

    A bare marker silences nothing (review M2). What follows the marker, to
    the end of the paragraph, has to contain at least one of: an N-of-M count,
    an issue or PR number, a review FILE (`*review*.md`), a named test
    function (`test_*`), or a path that resolves on disk. The English word
    "review" on its own is not a citation (review r2-m2).
    """
    m = DISCHARGE_RE.search(p.text)
    if not m:
        return None
    tail = p.text[m.end():].strip()
    if not tail:
        return None
    if NM_COUNT_RE.search(tail):
        return tail
    if DISCHARGE_ISSUE_RE.search(tail):
        return tail
    if DISCHARGE_REVIEW_RE.search(tail):
        return tail
    if DISCHARGE_TEST_RE.search(tail):
        return tail
    for pm in DISCHARGE_PATH_RE.finditer(tail):
        if path_resolves(pm.group(1), p.file, repo_root):
            return tail
    return None


def has_discharge(p: Paragraph, repo_root: str) -> bool:
    return discharge_citation(p, repo_root) is not None


def has_nm_or_evidence_path(p: Paragraph, repo_root: str) -> bool:
    return bool(NM_COUNT_RE.search(p.text)
                or resolving_evidence_paths(p, repo_root))


def _dangling_note(p: Paragraph, repo_root: str) -> str:
    dangling = dangling_evidence_paths(p, repo_root)
    if not dangling:
        return ""
    return (" (cited path %s does not resolve, so it does not count as "
            "evidence)" % ", ".join("`%s`" % d for d in dangling[:2]))


def check_universal(p: Paragraph, repo_root: str) -> Optional[Finding]:
    m = UNIVERSAL_RE.search(p.text)
    if not m:
        return None
    lowered = p.text.lower()
    if any(idiom in lowered for idiom in UNIVERSAL_IDIOM_EXEMPT):
        # Only exempt if the idiom accounts for the match; re-check by
        # stripping idioms out and re-searching.
        stripped = lowered
        for idiom in UNIVERSAL_IDIOM_EXEMPT:
            stripped = stripped.replace(idiom, "")
        if not UNIVERSAL_RE.search(stripped):
            return None
    trigger = m.group(1)
    if has_discharge(p, repo_root):
        return None
    if has_nm_or_evidence_path(p, repo_root):
        return None
    return Finding(
        p.file, p.start_line, "universal-claim", p.text,
        "unquantified absolute ('%s') with no N-of-M count, resolving "
        "evidence-log citation, or claim-lint:ok in this paragraph%s"
        % (trigger, _dangling_note(p, repo_root)),
        p.end_line,
    )


PATH_EXTENSION_RE = re.compile(
    r"\.(?:md|txt|log|json|rs|py|sh|toml|S)$", re.IGNORECASE
)


def _is_path_or_filename_token(text: str, match: "re.Match") -> bool:
    """True if `match` sits inside a filesystem path or filename, where `-`
    and `/` are regex word boundaries but not English ones (review M2:
    `789-SLICE2-PROVE-2026-09-04.md` and `serials/slice1b/prove/` both
    matched `\bprove\b` even though neither is the English word "prove").
    Expands to the surrounding run of non-whitespace characters and checks
    whether THAT token (not the whole paragraph) is a path -- contains a
    `/` -- or a bare filename -- ends in a recognized extension."""
    start, end = match.span()
    tok_start = start
    while tok_start > 0 and not text[tok_start - 1].isspace():
        tok_start -= 1
    tok_end = end
    while tok_end < len(text) and not text[tok_end].isspace():
        tok_end += 1
    token = text[tok_start:tok_end].strip("`\"'(),;:")
    if "/" in token:
        return True
    return bool(PATH_EXTENSION_RE.search(token))


def check_proven(p: Paragraph, repo_root: str) -> Optional[Finding]:
    m = None
    for cand in PROVEN_RE.finditer(p.text):
        if not _is_path_or_filename_token(p.text, cand):
            m = cand
            break
    if not m:
        return None
    if has_discharge(p, repo_root):
        return None
    if PROVEN_EVIDENCE_RE.search(p.text):
        return None
    if has_nm_or_evidence_path(p, repo_root):
        return None
    return Finding(
        p.file, p.start_line, "unproven-claim", p.text,
        "'%s' with no named mutation/experiment (mutation, revert, "
        "redden, falsify, reproduce, regression, --boots, bisect), "
        "N-of-M count, or resolving evidence-log citation in this paragraph%s"
        % (m.group(1), _dangling_note(p, repo_root)),
        p.end_line,
    )


def check_live_claim(p: Paragraph, repo_root: str) -> Optional[Finding]:
    m = LIVE_CLAIM_RE.search(p.text)
    if not m:
        return None
    if has_discharge(p, repo_root):
        return None
    if resolving_evidence_paths(p, repo_root):
        return None
    return Finding(
        p.file, p.start_line, "live-no-artifact", p.text,
        "'%s' with no resolving artifact path (serials/, confirm/, "
        "scratchpad/, .log, .txt) in this paragraph%s"
        % (m.group(0), _dangling_note(p, repo_root)),
        p.end_line,
    )


def check_absolute_guarantee(p: Paragraph, repo_root: str) -> Optional[Finding]:
    m = ABSOLUTE_GUARANTEE_RE.search(p.text)
    if not m:
        return None
    if has_discharge(p, repo_root):
        return None
    if has_nm_or_evidence_path(p, repo_root):
        return None
    return Finding(
        p.file, p.start_line, "absolute-guarantee", p.text,
        "'%s' asserted with no N-of-M count, resolving evidence-log "
        "citation, or claim-lint:ok in this paragraph%s"
        % (m.group(1), _dangling_note(p, repo_root)),
        p.end_line,
    )


def check_artifact_path(p: Paragraph, repo_root: str) -> Optional[Finding]:
    m = ARTIFACT_CLAIM_RE.search(p.text)
    if not m:
        return None
    if has_discharge(p, repo_root):
        return None
    path = m.group(2).strip()
    if path.startswith(("http://", "https://")):
        return None
    if path_resolves(path, p.file, repo_root):
        return None
    return Finding(
        p.file, p.start_line, "artifact-path-missing", p.text,
        "claims '%s at/to/in `%s`' but that path does not name a file "
        "in the tree (a directory is not an artifact)" % (m.group(1), path),
        p.end_line,
    )


def check_auto_close_keyword(p: Paragraph, repo_root: str) -> Optional[Finding]:
    """A close/fix/resolve keyword sitting directly in front of "#N".

    Deliberately does NOT call has_discharge() or check for an N-of-M count
    or an evidence path -- there is nothing here for those to exempt. Every
    other rule in this file asks "is this strong claim backed by evidence in
    the same paragraph"; a `claim-lint:ok` answers that question. This rule
    asks "will GitHub auto-close an issue when this text merges", and the
    answer does not change because a human annotated the paragraph -- GitHub
    reads the merged bytes, not the annotation, so a claim-lint:ok next to
    this phrase would silence the tool while the auto-close still fires. The
    only real fix is to remove the keyword or drop the colon/space binding it
    to "#N" (see AUTO_CLOSE_KEYWORD_RE's comment for the incident this
    codifies). See test_not_dischargeable_by_claim_lint_ok in
    scripts/test_claim_lint.py.
    """
    m = AUTO_CLOSE_KEYWORD_RE.search(p.text)
    if not m:
        return None
    return Finding(
        p.file, p.start_line, "auto-close-keyword", p.text,
        "'%s' immediately in front of an issue reference triggers GitHub's "
        "auto-close-on-merge; the convention from here on is a plain '#N', "
        "not a close/fix/resolve keyword directly in front of it. Not "
        "dischargeable by claim-lint:ok -- rewrite the reference, don't "
        "annotate it. A negated claim ('does not resolve #N') is not "
        "exempt either: GitHub's own matcher does not parse negation, so "
        "the bytes are exactly as dangerous -- break the adjacency instead, "
        "e.g. 'resolve issue #N' or lead with the number ('#N is not "
        "resolved by this change'); see "
        "docs/planning/green-program/claim-linting.md." % m.group(0),
        p.end_line,
    )


RULES = [check_universal, check_proven, check_live_claim,
         check_absolute_guarantee, check_artifact_path,
         check_auto_close_keyword]


def lint_paragraph(p: Paragraph, repo_root: str) -> list:
    findings = []
    for rule in RULES:
        f = rule(p, repo_root)
        if f:
            findings.append(f)
    return findings


def lint_text(file: str, content: str, repo_root: str = None,
              ext_override: str = None) -> list:
    if repo_root is None:
        repo_root = REPO_ROOT
    findings = []
    for p in extract_paragraphs(file, content, ext_override):
        findings.extend(lint_paragraph(p, repo_root))
    return findings


def lint_file(path: str, repo_root: str = None,
              assume_text: bool = False) -> Optional[list]:
    """Lint one file. Returns a list of findings (possibly empty), or `None`
    if the file was SKIPPED because its extension is not in
    `TEXT_EXTENSIONS` and `assume_text` is False.

    `None` is a distinct outcome from `[]` on purpose (review F3): before
    this, a skipped file and a clean file both produced `[]`, and main()'s
    summary line counted a skip as "checked" -- an explicit `--files
    COMMIT_EDITMSG` run (no recognized extension) reported "clean (1 file(s)
    checked)" and exit 0 having never opened the file. `--files` is the
    round checklist's own required step for linting a PR body or a
    scratchpad doc (see docs/planning/green-program/claim-linting.md), and
    those surfaces are not guaranteed to carry a `.md`/`.txt` extension --
    `git commit`'s own `COMMIT_EDITMSG` has none. `assume_text=True` (main()
    sets it whenever the caller named files explicitly via `--files`) treats
    an unrecognized extension as prose rather than skipping it; `--all` and
    diff-mode targets (auto-discovered, and able to include images, locks,
    and other non-prose files) keep the strict allowlist, and main() now
    reports a skip there instead of folding it into "checked".
    """
    if repo_root is None:
        repo_root = REPO_ROOT
    ext = os.path.splitext(path)[1]
    ext_override = None
    if ext not in TEXT_EXTENSIONS:
        if not assume_text:
            return None
        ext_override = ".txt"
    rel = os.path.relpath(path, repo_root) if os.path.isabs(path) else path
    if is_capture_file(rel):
        return []
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as fh:
            content = fh.read()
    except (FileNotFoundError, IsADirectoryError):
        return []
    return lint_text(rel, content, repo_root, ext_override)


def _git_comment_char(repo_root: str) -> str:
    """The character `git commit`'s own cleanup treats as a comment marker
    (`core.commentChar`; unset, empty, or `auto` all fall back to git's
    actual default, `#`) -- see _strip_git_cleanup_noise()."""
    out = subprocess.run(
        ["git", "config", "--get", "core.commentChar"],
        capture_output=True, text=True, cwd=repo_root,
    )
    char = out.stdout.strip()
    if out.returncode != 0 or not char or char == "auto":
        return "#"
    return char[0]


def _strip_git_cleanup_noise(lines: list, repo_root: str) -> list:
    """Blank out (never delete -- callers index by line number) the parts of
    a COMMIT_EDITMSG that git's own cleanup removes before the text becomes
    the actual commit message, so linting does not fail a commit over bytes
    git itself discards (review M1): the scissors cut line a `git commit -v`
    template inserts (`<comment-char> ---...--- >8 ---...---`) and everything
    from it to the end of the file -- the diffstat/diff `-v` appends -- plus
    any comment-prefixed line (`# On branch ...`, git's status hints) above
    it. A `commit-msg` hook runs on COMMIT_EDITMSG BEFORE this cleanup
    (githooks(5)), so without stripping it here too, a real `git commit -v`
    on this very branch tripped `universal-claim` on the appended diff's OWN
    added prose -- reproduced at scratchpad/cm/serials/h2_verbose.txt (526
    lines), `--commit-msg` exit 1 on the diff body; the same file with the
    `-v` region removed is clean."""
    char = _git_comment_char(repo_root)
    scissors_re = re.compile(r"^\s*" + re.escape(char) + r"\s*-+\s*>8\s*-+\s*$")
    comment_re = re.compile(r"^\s*" + re.escape(char))
    out = list(lines)
    past_scissors = False
    for i, line in enumerate(lines):
        if past_scissors:
            out[i] = ""
        elif scissors_re.match(line):
            past_scissors = True
            out[i] = ""
        elif comment_re.match(line):
            out[i] = ""
    return out


def lint_commit_msg_text(content: str, repo_root: str = None,
                          file: str = "<commit-msg>") -> list:
    """Lint a git COMMIT MESSAGE, as prose, with every rule.

    This exists because of a specific incident: commit `e6dd14a6` quoted
    this tool's own auto-close-keyword vocabulary directly in front of three
    real, open issue numbers inside its own commit message, describing a
    rewording made in a different file -- and GitHub's merge-time parser
    auto-closed all three the moment the commit landed on `main`. Neither of
    the round checklist's two mandated runs would have caught it: diff mode
    lints the tree the commit changes, not the message describing the
    change, and a `--files` PR-body run does not read a commit message
    either. See docs/planning/green-program/claim-linting.md's
    `--commit-msg` section (R21) for the incident in full -- its text is not
    reproduced here, on purpose: reproducing it would place the same
    keyword-adjacent-to-a-real-issue-number shape in this file too.

    Git's own comment lines and scissors/diff region are stripped first
    (`_strip_git_cleanup_noise()`, review M1) -- a `commit-msg` hook sees
    that text raw, before git's cleanup ever removes it.

    A SINGLE pass, fences left unblanked, over every one of the six rules
    (review m3, correcting R21's original two-pass split). A fenced ```
    code block is blanked for the diff/`--files` modes because a fence
    there usually IS code a real, markdown-rendered doc should not read as
    prose -- but a commit message is never markdown-rendered, by GitHub's
    issue-closing parser or by anything else that displays one (`git log`
    shows the backticks literally), so that rationale does not hold for
    ANY of the six rules here, not only auto-close-keyword: a universal or
    unproven claim hidden inside a ``` block in a commit message used to
    read as clean (see test_universal_claim_inside_a_fence_still_fires_in_
    commit_msg_mode in scripts/test_claim_lint.py).
    """
    if repo_root is None:
        repo_root = REPO_ROOT
    lines = _strip_git_cleanup_noise(content.splitlines(), repo_root)
    findings = []
    for p in extract_markdown_paragraphs(file, lines, blank_fences=False):
        findings.extend(lint_paragraph(p, repo_root))
    findings.sort(key=lambda f: f.line)
    return findings


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

HUNK_RE = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@")


def git_changed_files(base: str, repo_root: str) -> list:
    out = subprocess.run(
        ["git", "diff", "--name-only", "--diff-filter=ACMR", base, "--"],
        capture_output=True, text=True, cwd=repo_root,
    )
    if out.returncode != 0:
        raise SystemExit(
            "claim-lint: `git diff --name-only %s` failed: %s"
            % (base, out.stderr.strip())
        )
    return [line for line in out.stdout.splitlines() if line.strip()]


def git_untracked_files(repo_root: str) -> list:
    """New, not-yet-`git add`ed, non-ignored files.

    `git diff` cannot see them, so before R3 a brand-new EVIDENCE doc -- the
    shape an arc in this campaign starts with -- was invisible in the mode the
    checklist prescribes (review R2-M2). A whole new file counts as entirely
    changed.
    """
    out = subprocess.run(
        ["git", "ls-files", "--others", "--exclude-standard"],
        capture_output=True, text=True, cwd=repo_root,
    )
    return [line for line in out.stdout.splitlines() if line.strip()]


def changed_line_ranges(base: str, path: str, repo_root: str,
                        head: str = None) -> list:
    """Post-image line ranges changed in `path`, from git diff -U0.

    With no `head`, the comparison is base vs the WORKING TREE (the normal
    pre-review case, so uncommitted edits are covered). With `head`, it is
    base vs that commit -- used to replay a historical round.
    """
    cmd = ["git", "diff", "-U0", base]
    if head:
        cmd.append(head)
    cmd += ["--", path]
    out = subprocess.run(cmd, capture_output=True, text=True, cwd=repo_root)
    ranges = []
    for line in out.stdout.splitlines():
        m = HUNK_RE.match(line)
        if not m:
            continue
        start = int(m.group(1))
        count = 1 if m.group(2) is None else int(m.group(2))
        if count > 0:
            ranges.append((start, start + count - 1))
    return ranges


def overlaps(finding: Finding, ranges: list) -> bool:
    for a, b in ranges:
        if a <= finding.end_line and b >= finding.line:
            return True
    return False


def git_all_tracked(repo_root: str) -> list:
    out = subprocess.run(
        ["git", "ls-files"], capture_output=True, text=True, cwd=repo_root,
    )
    return [line for line in out.stdout.splitlines() if line.strip()]


def resolve_base(explicit, repo_root: str) -> str:
    """Resolve a base ref to the branch point (`base...HEAD` semantics).

    Diffing a stale local `main` against the working tree reports files the
    branch never touched (review m4); the merge-base is the branch point, so
    only this branch's own changes are reported -- while still diffing against
    the WORKING TREE, so uncommitted edits are linted (the pre-review case).
    """
    ref = explicit
    if not ref:
        for candidate in ("origin/main", "main"):
            check = subprocess.run(
                ["git", "rev-parse", "--verify", "--quiet", candidate],
                capture_output=True, text=True, cwd=repo_root,
            )
            if check.returncode == 0:
                ref = candidate
                break
    if not ref:
        raise SystemExit(
            "claim-lint: could not find origin/main or main; pass --base <ref>"
        )
    mb = subprocess.run(
        ["git", "merge-base", ref, "HEAD"],
        capture_output=True, text=True, cwd=repo_root,
    )
    if mb.returncode == 0 and mb.stdout.strip():
        return mb.stdout.strip()
    return ref


def format_text(findings: list) -> str:
    lines = []
    for f in findings:
        snippet = f.text if len(f.text) <= 240 else f.text[:237] + "..."
        lines.append("%s:%d: [%s] %s" % (f.file, f.line, f.rule, snippet))
        lines.append("    -> %s" % f.detail)
    return "\n".join(lines)


def format_json(findings: list) -> str:
    return json.dumps([f.__dict__ for f in findings], indent=2)


def run_commit_msg_mode(path: str, repo_root: str, fmt: str) -> int:
    """The `--commit-msg` CLI mode: read `path`, lint it with
    lint_commit_msg_text(), print, return the exit code. A separate code
    path from the diff/`--files`/`--all` modes below -- there is no target
    discovery, no changed-hunk intersection, and no extension allowlist, on
    the same rationale `lint_file()`'s `assume_text` uses: a commit message
    is exactly the kind of extensionless surface `TEXT_EXTENSIONS` would
    otherwise skip.
    """
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as fh:
            content = fh.read()
    except (FileNotFoundError, IsADirectoryError, PermissionError) as e:
        print("claim-lint: cannot read commit-message file %s: %s" % (path, e))
        return 2
    rel = os.path.relpath(path, repo_root) if os.path.isabs(path) else path
    findings = lint_commit_msg_text(content, repo_root, file=rel)
    if fmt == "json":
        print(format_json(findings))
    elif findings:
        print(format_text(findings))
        print(
            "\nclaim-lint: %d finding(s) in commit message %s. Discharge a "
            "legitimate claim with a same-paragraph "
            "`claim-lint:ok: <citation>` annotation naming an N-of-M count, "
            "a resolving path, an issue, or a review. See "
            "docs/planning/green-program/claim-linting.md."
            % (len(findings), rel)
        )
        if any(f.rule == "auto-close-keyword" for f in findings):
            print(
                "claim-lint: auto-close-keyword finding(s) above cannot be "
                "discharged by a claim-lint:ok annotation -- the phrase "
                "acts on GitHub at merge/commit time regardless of any "
                "annotation. Rewrite the reference as a plain '#N' with no "
                "close/fix/resolve keyword directly in front of it."
            )
    else:
        print("claim-lint: clean commit message (%s)." % rel)
    return 1 if findings else 0


def main(argv: list) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--base", help="git ref to diff against (default: origin/main, else main)")
    ap.add_argument("--files", nargs="*", help="explicit file list, bypasses git diff")
    ap.add_argument("--all", action="store_true", help="lint every tracked text file")
    ap.add_argument(
        "--commit-msg", metavar="FILE",
        help="lint FILE as a git commit message: every rule, as prose, "
             "regardless of extension, with auto-close-keyword additionally "
             "checked unfenced. A separate mode -- ignores --base/--files/"
             "--all/--whole-file. See lint_commit_msg_text().",
    )
    ap.add_argument(
        "--whole-file", action="store_true",
        help="in diff mode, report findings anywhere in a changed file "
             "instead of only in changed hunks",
    )
    ap.add_argument("--format", choices=["text", "json"], default="text")
    ap.add_argument("--repo-root", default=REPO_ROOT)
    args = ap.parse_args(argv)

    repo_root = args.repo_root

    if args.commit_msg:
        return run_commit_msg_mode(args.commit_msg, repo_root, args.format)

    base = None
    untracked = set()

    if args.files:
        targets = args.files
    elif args.all:
        targets = git_all_tracked(repo_root)
    else:
        base = resolve_base(args.base, repo_root)
        targets = git_changed_files(base, repo_root)
        seen = set(targets)
        for rel in git_untracked_files(repo_root):
            if rel not in seen:
                targets.append(rel)
                untracked.add(rel)

    changed_only = base is not None and not args.whole_file
    # `--files` names an explicit surface the checklist requires linting no
    # matter its extension -- a PR body or `COMMIT_EDITMSG` frequently
    # carries no extension (review F3). Everything else (whole-repo and
    # diff-mode targets) auto-discovers files and keeps the strict allowlist
    # instead, since those can include images, lockfiles and other
    # non-prose files a markdown/comment parser would misread.
    assume_text = bool(args.files)

    all_findings = []
    suppressed = 0
    skipped = []
    for rel in targets:
        path = rel if os.path.isabs(rel) else os.path.join(repo_root, rel)
        found = lint_file(path, repo_root, assume_text=assume_text)
        if found is None:
            skipped.append(rel)
            continue
        # A new file has no diff to intersect with; it is linted whole.
        if changed_only and found and rel not in untracked:
            ranges = changed_line_ranges(base, rel, repo_root)
            kept = [f for f in found if overlaps(f, ranges)]
            suppressed += len(found) - len(kept)
            found = kept
        all_findings.extend(found)

    checked = len(targets) - len(skipped)

    if args.format == "json":
        print(format_json(all_findings))
    else:
        scope = ("changed hunks vs %s" % base[:12]) if changed_only else (
            "whole files" if base is None else "whole changed files vs %s" % base[:12])
        if all_findings:
            print(format_text(all_findings))
            print(
                "\nclaim-lint: %d finding(s) across %d file(s) [%s]. Discharge a "
                "legitimate claim with a same-paragraph "
                "`claim-lint:ok: <citation>` annotation naming an N-of-M count, "
                "a resolving path, an issue, or a review. See "
                "docs/planning/green-program/claim-linting.md."
                % (len(all_findings), checked, scope)
            )
            if any(f.rule == "auto-close-keyword" for f in all_findings):
                print(
                    "claim-lint: auto-close-keyword finding(s) above cannot be "
                    "discharged by a claim-lint:ok annotation -- the phrase "
                    "acts on GitHub at merge/commit time regardless of any "
                    "annotation. Rewrite the reference as a plain '#N' with "
                    "no close/fix/resolve keyword directly in front of it."
                )
        else:
            print("claim-lint: clean (%d file(s) checked, %s)."
                  % (checked, scope))
        if changed_only and suppressed:
            print("claim-lint: %d pre-existing finding(s) outside this "
                  "branch's changed hunks not reported (--whole-file shows them)."
                  % suppressed)
        if skipped:
            print(
                "claim-lint: %d target(s) SKIPPED, not checked (extension "
                "not in %s): %s"
                % (len(skipped), sorted(TEXT_EXTENSIONS), ", ".join(skipped))
            )

    return 1 if all_findings else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
