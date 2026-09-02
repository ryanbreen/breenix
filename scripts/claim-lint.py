#!/usr/bin/env python3
"""
claim-lint.py — mechanical detector for the "claim discipline" violations that
made up 56% of blocking review findings in the 2026-08/09 green-program campaign
(37 of 66 blockers, per ~/Downloads/breenix-progress-assessment-2026-09-01.html):
factually wrong, checkable sentences shipped in EVIDENCE/CONFIRM docs, PR bodies,
gate-script headers and source comments — not code defects.

This tool does not know whether a claim is TRUE. It cannot execute a gate or
replay a mutation. What it catches is the SHAPE that every one of those 37
false claims shared: an unquantified absolute ("every", "zero", "airtight",
"guaranteed", "structurally"), a "proven"/"PROVEN" with no named mutation next
to it, an "observed live"/"confirmed live" with no artifact path next to it, or
a "preserved/attached/committed at <path>" citing a path that does not exist in
the tree. Every rule below was reverse-engineered from a verbatim quote in the
review corpus — see scripts/claim_lint_corpus/historical_false_claims.json for
the specimen each rule was built to catch, and docs/planning/green-program/
claim-linting.md for how to run this and how to discharge a hit honestly.

Usage:
    scripts/claim-lint.py                       # lint files changed vs origin/main
    scripts/claim-lint.py --base main
    scripts/claim-lint.py --files a.md b.rs
    scripts/claim-lint.py --all                 # lint every tracked text file (slow)
    scripts/claim-lint.py --format json ...

Discharge: an author who has genuinely checked a strong claim marks it in the
same paragraph with a `claim-lint:ok` annotation naming the artifact, e.g.:

    <!-- claim-lint:ok: 12/12 arms -- review-baseline.log -->
    every arm passed the baseline run.

    // claim-lint:ok: mutation-proven, see fix2-review.md B1
    every close path decrements the refcount.

Exit codes: 0 = clean, 1 = un-discharged findings, 2 = usage/internal error.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from typing import Optional

REPO_ROOT = subprocess.run(
    ["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True
).stdout.strip() or os.getcwd()

TEXT_EXTENSIONS = {".md", ".rs", ".sh", ".py", ".txt"}

DISCHARGE_RE = re.compile(r"claim-lint:\s*ok\b", re.IGNORECASE)

# ---------------------------------------------------------------------------
# Rule vocabulary. claim-lint:ok -- traceable one-for-one against
# scripts/claim_lint_corpus/historical_false_claims.json's `expected_rules`;
# see that file for the verbatim quote each word was added to catch. Do not
# add words on spec; add a specimen to the corpus first (calibrate against
# the real thing) -- see the "each" decision in claim-linting.md for what
# happens when you don't.
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

PROVEN_RE = re.compile(r"\b(proven|proves|proof|PROVEN)\b", re.IGNORECASE)
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
# below it implements; see test_evidence_log_path_clears_a_universal and
# test_source_path_does_not_clear_a_universal in test_claim_lint.py.
# A citation that counts as "evidence attached in this paragraph" for the
# UNIVERSAL / PROVEN / LIVE_CLAIM / ABSOLUTE_GUARANTEE rules: either a proper
# N-of-M count, or a path/filename that looks like a captured log/serial
# rather than a bare source-code pointer (source paths name the code under
# discussion, they do not establish the claim -- see gtty fix2-review
# BLOCKING 1, where the false universal cites two *.rs paths in one sentence).
NM_COUNT_RE = re.compile(r"\b\d+\s*(?:/|of)\s*\d+\b", re.IGNORECASE)
EVIDENCE_PATH_RE = re.compile(
    r"[`\"]([^`\"\s]*(?:serials?/|confirm/|scratchpad/)[^`\"\s]*"
    r"|[^`\"\s]+\.(?:log|txt))[`\"]",
    re.IGNORECASE,
)

# Matches "preserved/attached/committed/saved/written" + "at/to/in" + a
# backtick-quoted path -- a mechanically checkable claim (gtty review.md B4:
# the cited path did not exist on the branch). Captures the path following
# the verb so its existence can be checked on disk.
ARTIFACT_CLAIM_RE = re.compile(
    r"\b(preserved|attached|committed|saved|written)\s+(?:at|to|in)\b"
    r"[^`]{0,40}`([^`]+)`",
    re.IGNORECASE,
)


@dataclass
class Finding:
    file: str
    line: int
    rule: str
    text: str
    detail: str = ""


@dataclass
class Paragraph:
    file: str
    start_line: int
    text: str


# claim-lint:ok: this design choice and its cost are measured, not asserted --
# see the "unit is the paragraph, not the sentence" section of claim-linting.md
# and RealDocumentReportTests in test_claim_lint.py for the actual counts.
# ---------------------------------------------------------------------------
# Extraction: turn a source file into a list of prose paragraphs worth
# checking. Granularity is the paragraph, not the sentence -- a single claim
# in this corpus is almost always one bullet, one blockquote, or one table
# row, and paragraph-level exemption search (does *this* paragraph carry a
# citation anywhere) matches how these docs actually cite evidence: often in
# a different clause of the same bullet than the trigger word.
# ---------------------------------------------------------------------------

FENCE_RE = re.compile(r"^\s*(```|~~~)")
TABLE_ROW_RE = re.compile(r"^\s*\|.*\|\s*$")
TABLE_SEP_RE = re.compile(r"^\s*\|[\s:|-]+\|\s*$")


def extract_markdown_paragraphs(file: str, lines: list) -> list:
    # Blank out fenced code blocks (keep line count so numbers stay aligned).
    scrubbed = list(lines)
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

    def flush():
        if buf:
            text = " ".join(s.strip() for s in buf if s.strip())
            if text:
                paragraphs.append(Paragraph(file, buf_start[0], text))
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
                paragraphs.append(Paragraph(file, lineno, cell_text))
            buf_start[0] = lineno + 1
            continue
        if not stripped:
            flush()
            buf_start[0] = lineno + 1
            continue
        text = stripped
        if text.startswith(">"):
            text = text.lstrip(">").strip()
        if not buf:
            buf_start[0] = lineno
        buf.append(text)
    flush()
    return paragraphs


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

    def flush():
        if buf:
            text = " ".join(s for s in buf if s)
            if text.strip():
                paragraphs.append(Paragraph(file, buf_start[0], text))
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
        if not buf:
            buf_start[0] = lineno
        buf.append(content)
    flush()
    return paragraphs


def extract_paragraphs(file: str, content: str) -> list:
    lines = content.splitlines()
    ext = os.path.splitext(file)[1]
    if ext == ".md" or ext == ".txt":
        return extract_markdown_paragraphs(file, lines)
    if ext in LINE_COMMENT_PREFIXES:
        return extract_comment_paragraphs(file, lines, ext)
    return []


# ---------------------------------------------------------------------------
# Rules
# ---------------------------------------------------------------------------

def has_discharge(text: str) -> bool:
    return bool(DISCHARGE_RE.search(text))


def has_nm_or_evidence_path(text: str) -> bool:
    return bool(NM_COUNT_RE.search(text) or EVIDENCE_PATH_RE.search(text))


def check_universal(p: Paragraph) -> Optional[Finding]:
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
    if has_discharge(p.text):
        return None
    if has_nm_or_evidence_path(p.text):
        return None
    return Finding(
        p.file, p.start_line, "universal-claim", p.text,
        "unquantified absolute ('%s') with no N-of-M count, "
        "evidence-log citation, or claim-lint:ok in this paragraph" % m.group(1),
    )


def check_proven(p: Paragraph) -> Optional[Finding]:
    m = PROVEN_RE.search(p.text)
    if not m:
        return None
    if has_discharge(p.text):
        return None
    if PROVEN_EVIDENCE_RE.search(p.text):
        return None
    if has_nm_or_evidence_path(p.text):
        return None
    return Finding(
        p.file, p.start_line, "unproven-claim", p.text,
        "'%s' with no named mutation/experiment (mutation, revert, "
        "redden, falsify, reproduce, regression, --boots, bisect), "
        "N-of-M count, or evidence-log citation in this paragraph" % m.group(1),
    )


def check_live_claim(p: Paragraph) -> Optional[Finding]:
    m = LIVE_CLAIM_RE.search(p.text)
    if not m:
        return None
    if has_discharge(p.text):
        return None
    if EVIDENCE_PATH_RE.search(p.text):
        return None
    return Finding(
        p.file, p.start_line, "live-no-artifact", p.text,
        "'%s' with no artifact path (serials/, confirm/, "
        "scratchpad/, .log, .txt) in this paragraph" % m.group(0),
    )


def check_absolute_guarantee(p: Paragraph) -> Optional[Finding]:
    m = ABSOLUTE_GUARANTEE_RE.search(p.text)
    if not m:
        return None
    if has_discharge(p.text):
        return None
    if has_nm_or_evidence_path(p.text):
        return None
    return Finding(
        p.file, p.start_line, "absolute-guarantee", p.text,
        "'%s' asserted with no N-of-M count, evidence-log "
        "citation, or claim-lint:ok in this paragraph" % m.group(1),
    )


def check_artifact_path(p: Paragraph, repo_root: str) -> Optional[Finding]:
    m = ARTIFACT_CLAIM_RE.search(p.text)
    if not m:
        return None
    if has_discharge(p.text):
        return None
    path = m.group(2).strip()
    if path.startswith(("http://", "https://")):
        return None
    if os.path.isabs(path):
        candidates = [path]
    else:
        # Evidence docs commonly cite a path relative to their own directory
        # (e.g. `serials/foo.txt` cited from inside .../tty/EVIDENCE-*.md,
        # meaning .../tty/serials/foo.txt) as well as repo-root-relative
        # paths. Accept either resolution.
        doc_dir = os.path.dirname(os.path.join(repo_root, p.file))
        candidates = [os.path.join(doc_dir, path), os.path.join(repo_root, path)]
    if any(os.path.exists(c) for c in candidates):
        return None
    return Finding(
        p.file, p.start_line, "artifact-path-missing", p.text,
        "claims '%s at/to/in `%s`' but that path does not "
        "exist in the tree" % (m.group(1), path),
    )


RULES = [check_universal, check_proven, check_live_claim,
         check_absolute_guarantee]


def lint_paragraph(p: Paragraph, repo_root: str) -> list:
    findings = []
    for rule in RULES:
        f = rule(p)
        if f:
            findings.append(f)
    f = check_artifact_path(p, repo_root)
    if f:
        findings.append(f)
    return findings


def lint_text(file: str, content: str, repo_root: str = None) -> list:
    if repo_root is None:
        repo_root = REPO_ROOT
    findings = []
    for p in extract_paragraphs(file, content):
        findings.extend(lint_paragraph(p, repo_root))
    return findings


def lint_file(path: str, repo_root: str = None) -> list:
    if repo_root is None:
        repo_root = REPO_ROOT
    ext = os.path.splitext(path)[1]
    if ext not in TEXT_EXTENSIONS:
        return []
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as fh:
            content = fh.read()
    except (FileNotFoundError, IsADirectoryError):
        return []
    rel = os.path.relpath(path, repo_root) if os.path.isabs(path) else path
    return lint_text(rel, content, repo_root)


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

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


def git_all_tracked(repo_root: str) -> list:
    out = subprocess.run(
        ["git", "ls-files"], capture_output=True, text=True, cwd=repo_root,
    )
    return [line for line in out.stdout.splitlines() if line.strip()]


def resolve_base(explicit, repo_root: str) -> str:
    if explicit:
        return explicit
    for candidate in ("origin/main", "main"):
        check = subprocess.run(
            ["git", "rev-parse", "--verify", "--quiet", candidate],
            capture_output=True, text=True, cwd=repo_root,
        )
        if check.returncode == 0:
            return candidate
    raise SystemExit(
        "claim-lint: could not find origin/main or main; pass --base <ref>"
    )


def format_text(findings: list) -> str:
    lines = []
    for f in findings:
        snippet = f.text if len(f.text) <= 240 else f.text[:237] + "..."
        lines.append("%s:%d: [%s] %s" % (f.file, f.line, f.rule, snippet))
        lines.append("    -> %s" % f.detail)
    return "\n".join(lines)


def format_json(findings: list) -> str:
    return json.dumps([f.__dict__ for f in findings], indent=2)


def main(argv: list) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--base", help="git ref to diff against (default: origin/main, else main)")
    ap.add_argument("--files", nargs="*", help="explicit file list, bypasses git diff")
    ap.add_argument("--all", action="store_true", help="lint every tracked text file")
    ap.add_argument("--format", choices=["text", "json"], default="text")
    ap.add_argument("--repo-root", default=REPO_ROOT)
    args = ap.parse_args(argv)

    repo_root = args.repo_root

    if args.files:
        targets = args.files
    elif args.all:
        targets = git_all_tracked(repo_root)
    else:
        base = resolve_base(args.base, repo_root)
        targets = git_changed_files(base, repo_root)

    all_findings = []
    for rel in targets:
        path = rel if os.path.isabs(rel) else os.path.join(repo_root, rel)
        all_findings.extend(lint_file(path, repo_root))

    if args.format == "json":
        print(format_json(all_findings))
    else:
        if all_findings:
            print(format_text(all_findings))
            print(
                "\nclaim-lint: %d finding(s) across %d file(s). Discharge a "
                "legitimate universal with a same-paragraph "
                "`claim-lint:ok: <citation>` annotation. See "
                "docs/planning/green-program/claim-linting.md."
                % (len(all_findings), len(targets))
            )
        else:
            print("claim-lint: clean (%d file(s) checked)." % len(targets))

    return 1 if all_findings else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
