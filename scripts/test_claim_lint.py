#!/usr/bin/env python3
"""
Tests for scripts/claim-lint.py.

Four layers:

1. Unit tests on the rule mechanics (discharge annotation, N-of-M exemption,
   idiom exemption, artifact-path existence check, list-item paragraph
   breaking, changed-hunk intersection) -- small, hand-built fixtures with an
   unambiguous right answer.

2a. IN-CONTEXT historical corpus (the headline). For every specimen in
   scripts/claim_lint_corpus/historical_false_claims.json whose shipped bytes
   are recoverable (`surface: repo`), the FILE AS IT SHIPPED is recovered with
   `git show <shipped_commit>:<shipped_path>` and linted whole, exactly as the
   tool runs in real use; the specimen counts as CAUGHT only if some finding's
   own paragraph contains the offending sentence. This is the number that
   belongs in a record, because it is the mode the tool actually runs in.

2b. ISOLATED-QUOTE historical corpus (secondary, and labelled as such). The
   same specimens fed to the linter as standalone one-sentence files. This
   over-reports -- a lone sentence carries no paragraph around it to exempt it
   -- but it is the only layer available for the six specimens that shipped on
   a surface this repo does not contain (a GitHub issue comment, four
   scratchpad files, one sentence reworded before landing).

3. verified_true_claims.json + two real, currently-in-repo evidence docs
   (WORKLOAD-ENVELOPES.md, tty/EVIDENCE-2026-08-30.md) measure the false-
   positive rate against prose the review process actually accepted as true.

4. Per-round load: three real fix rounds from this campaign, reported both
   whole-file and changed-hunks-only, so the number an author actually faces
   is re-derived every run instead of being asserted once in a doc.

Layers 3 and 4 are reports, not brittle pass/fail gates -- the documents and
commits they read are outside this tool's control.

Run: python3 scripts/test_claim_lint.py [-v]
"""
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CLAIM_LINT_PATH = os.path.join(REPO_ROOT, "scripts", "claim-lint.py")
CORPUS_DIR = os.path.join(REPO_ROOT, "scripts", "claim_lint_corpus")


def _load_claim_lint():
    spec = importlib.util.spec_from_file_location("claim_lint", CLAIM_LINT_PATH)
    mod = importlib.util.module_from_spec(spec)
    sys.modules["claim_lint"] = mod  # dataclass introspection needs this registered
    spec.loader.exec_module(mod)
    return mod


cl = _load_claim_lint()


def wrap_quote(quote: str, filetype: str) -> str:
    """Wrap a bare quote the way it would actually appear in that file type."""
    if filetype == ".md":
        return quote + "\n"
    if filetype == ".rs":
        return "\n".join("/// " + line for line in quote.splitlines()) + "\n"
    if filetype == ".sh":
        return "#!/usr/bin/env bash\n" + "\n".join(
            "# " + line for line in quote.splitlines()
        ) + "\n"
    raise ValueError("unhandled filetype %r" % filetype)


def norm(s: str) -> str:
    return " ".join(s.split()).lower()


# Which rule's trigger vocabulary fired, so a CAUGHT specimen can be split
# into "flagged on the false sentence's own words" vs "flagged because a
# NEIGHBOURING sentence of the same paragraph carried a trigger" (review
# R2-M3). Both put the author's eye on the paragraph; only the first is the
# tool reading the claim.
RULE_TRIGGER_RE = {
    "universal-claim": cl.UNIVERSAL_RE,
    "unproven-claim": cl.PROVEN_RE,
    "live-no-artifact": cl.LIVE_CLAIM_RE,
    "absolute-guarantee": cl.ABSOLUTE_GUARANTEE_RE,
    "artifact-path-missing": cl.ARTIFACT_CLAIM_RE,
}


def trigger_is_in(rule: str, sentence: str) -> bool:
    rx = RULE_TRIGGER_RE.get(rule)
    return bool(rx and rx.search(sentence))


def git_show(commit: str, path: str):
    """Bytes of `path` as of `commit`, or None if this checkout can't reach it."""
    out = subprocess.run(
        ["git", "show", "%s:%s" % (commit, path)],
        capture_output=True, text=True, cwd=REPO_ROOT,
    )
    if out.returncode != 0:
        return None
    return out.stdout


class MechanicsTests(unittest.TestCase):
    """Layer 1: the rule mechanics in isolation."""

    def test_bare_universal_is_flagged(self):
        findings = cl.lint_text("x.md", "Every close path decrements the refcount.\n")
        self.assertTrue(any(f.rule == "universal-claim" for f in findings))

    def test_discharge_annotation_with_citation_clears_a_universal(self):
        text = (
            "<!-- claim-lint:ok: mutation-proven, see fix2-review.md B1 -->\n"
            "Every close path decrements the refcount.\n"
        )
        findings = cl.lint_text("x.md", text)
        self.assertEqual(findings, [])

    def test_annotation_does_not_discharge_the_paragraph_above_it(self):
        # An annotation belongs to the claim it introduces. Attaching
        # backwards let a `claim-lint:ok` written for bullet 2 silence
        # bullet 1 (caught dogfooding R2 on claim-linting.md itself).
        text = (
            "- Every close path decrements the refcount.\n"
            "<!-- claim-lint:ok: 12/12 arms, run.log -->\n"
            "- The other thing is fine.\n"
        )
        findings = cl.lint_text("x.md", text)
        self.assertTrue(
            any(f.rule == "universal-claim" and "close path" in f.text
                for f in findings),
            "the bullet above an annotation must not be discharged by it",
        )

    def test_bare_discharge_annotation_does_not_clear(self):
        # Review M2: `claim-lint:ok` with no text after it is a mute button,
        # not a citation. It must not silence a finding.
        text = "<!-- claim-lint:ok -->\nEvery close path decrements the refcount.\n"
        findings = cl.lint_text("x.md", text)
        self.assertTrue(any(f.rule == "universal-claim" for f in findings))

    def test_discharge_annotation_with_junk_does_not_clear(self):
        text = "<!-- claim-lint:ok: lol -->\nEvery close path decrements the refcount.\n"
        findings = cl.lint_text("x.md", text)
        self.assertTrue(any(f.rule == "universal-claim" for f in findings))

    def test_discharge_annotation_with_issue_number_clears(self):
        text = "<!-- claim-lint:ok: closed by #728 -->\nEvery close path decrements the refcount.\n"
        self.assertEqual(cl.lint_text("x.md", text), [])

    def test_discharge_annotation_with_resolving_path_clears(self):
        text = ("<!-- claim-lint:ok: see scripts/claim-lint.py -->\n"
                "Every close path decrements the refcount.\n")
        self.assertEqual(cl.lint_text("x.md", text, REPO_ROOT), [])

    def test_discharge_annotation_with_dangling_path_does_not_clear(self):
        text = ("<!-- claim-lint:ok: see scripts/no-such-file-xyz.py -->\n"
                "Every close path decrements the refcount.\n")
        findings = cl.lint_text("x.md", text, REPO_ROOT)
        self.assertTrue(any(f.rule == "universal-claim" for f in findings))

    def test_nm_count_clears_a_universal(self):
        text = "Every one of the 12/12 arms passed the baseline run.\n"
        findings = cl.lint_text("x.md", text)
        self.assertEqual(findings, [])

    def test_evidence_log_path_clears_a_universal(self):
        with tempfile.TemporaryDirectory() as tmp:
            os.makedirs(os.path.join(tmp, "confirm"))
            with open(os.path.join(tmp, "confirm", "aarch64-run1-serial.txt"), "w") as fh:
                fh.write("serial\n")
            text = ("All boots reached the clean verdict, per "
                    "`confirm/aarch64-run1-serial.txt`.\n")
            self.assertEqual(cl.lint_text("EVIDENCE.md", text, tmp), [])

    def test_nonexistent_evidence_path_does_not_clear_a_universal(self):
        # Review M1: an artifact path used as an EXEMPTION went unchecked
        # for existence, while the artifact-path rule checked exactly that.
        # Corpus F2 is a cited serial path that did not exist.
        text = "Every close path decrements, see `nonexistent-file-xyz.log`.\n"
        findings = cl.lint_text("x.md", text, REPO_ROOT)
        self.assertTrue(any(f.rule == "universal-claim" for f in findings))

    def test_nonexistent_evidence_path_does_not_clear_a_live_claim(self):
        text = "Observed live in `made-up-xyz.log`.\n"
        findings = cl.lint_text("x.md", text, REPO_ROOT)
        self.assertTrue(any(f.rule == "live-no-artifact" for f in findings))

    def test_source_path_does_not_clear_a_universal(self):
        # claim-lint:ok: the premise is enforced by the assertion below, not by
        # this comment; the specimen is corpus F3 in
        # scripts/claim_lint_corpus/historical_false_claims.json.
        # The gtty BLOCKING-1 shape: citing the *.rs file the claim is about
        # does not establish the claim across every caller.
        text = (
            "while every close path (`kernel/src/ipc/fd.rs`, "
            "`kernel/src/task/process_task.rs`) calls `pair.slave_close()` "
            "unconditionally for any released slave\n"
        )
        findings = cl.lint_text("x.md", text)
        self.assertTrue(any(f.rule == "universal-claim" for f in findings))

    def test_list_items_do_not_share_an_exemption(self):
        # Review B1/M3: a bullet list with no blank lines used to merge into
        # one paragraph, so a count in bullet 3 exempted an absolute in
        # bullet 1. This is the mechanism behind the corpus F1 miss in situ.
        text = (
            "* **Zero open issues against the TTY layer: satisfied.**\n"
            "* The gate ran 12 arms, 3/3 boots, all clean.\n"
        )
        findings = cl.lint_text("x.md", text)
        self.assertTrue(
            any(f.rule == "universal-claim" and "Zero open issues" in f.text
                for f in findings),
            "the bullet carrying the absolute must not be exempted by the "
            "count in a different bullet",
        )

    def test_idiom_at_all_is_not_flagged(self):
        text = "This surface was not driven at all on the production profile.\n"
        findings = cl.lint_text("x.md", text)
        self.assertEqual(findings, [])

    def test_zero_feature_compound_is_not_flagged(self):
        text = "Kernel build: the shipped zero-feature production profile, no flags.\n"
        findings = cl.lint_text("x.md", text)
        self.assertEqual(findings, [])

    def test_proven_with_mutation_keyword_clears(self):
        text = "The fix is proven: reverting the guard reddens the ratchet, then it is restored.\n"
        findings = cl.lint_text("x.md", text)
        self.assertEqual(findings, [])

    def test_bare_proven_is_flagged(self):
        text = "x86 beast battery: PROVEN, PASS.\n"
        findings = cl.lint_text("x.md", text)
        self.assertTrue(any(f.rule == "unproven-claim" for f in findings))

    def test_bare_prove_conjugations_are_flagged(self):
        # claim-lint:ok: this comment names the rule's vocabulary rather
        # than making a claim; the vocabulary is PROVEN_RE in
        # scripts/claim-lint.py and the assertion below is the check.
        # Review m1: the rule fired on three conjugations of the claim word
        # and skipped prove/proved/proving/demonstrated.
        for word in ("prove", "proved", "proving", "demonstrated"):
            text = "This %s the guard holds on the shipped profile.\n" % word
            findings = cl.lint_text("x.md", text)
            self.assertTrue(
                any(f.rule == "unproven-claim" for f in findings),
                "%r did not fire unproven-claim" % word,
            )

    def test_observed_live_without_path_is_flagged(self):
        text = "(observed live: EXT2_LOCK_PARK_FIRST fired during an unrelated test)\n"
        findings = cl.lint_text("x.md", text)
        self.assertTrue(any(f.rule == "live-no-artifact" for f in findings))

    def test_observed_live_with_resolving_serial_path_clears(self):
        with tempfile.TemporaryDirectory() as tmp:
            os.makedirs(os.path.join(tmp, "serials"))
            with open(os.path.join(tmp, "serials", "aarch64-boot19.txt"), "w") as fh:
                fh.write("serial\n")
            text = (
                "(observed live in `serials/aarch64-boot19.txt`: the marker fired "
                "after the leg's threads ran)\n"
            )
            self.assertEqual(cl.lint_text("EVIDENCE.md", text, tmp), [])

    def test_airtight_is_flagged(self):
        text = "Using the prior tick's count is airtight against misattribution.\n"
        findings = cl.lint_text("x.md", text)
        self.assertTrue(any(f.rule == "absolute-guarantee" for f in findings))

    def test_artifact_path_missing_is_flagged(self):
        text = "Full serial preserved at `docs/planning/green-program/tty/serials/does-not-exist-19700101.txt`.\n"
        findings = cl.lint_text("x.md", text, repo_root=REPO_ROOT)
        self.assertTrue(any(f.rule == "artifact-path-missing" for f in findings))

    def test_artifact_path_present_relative_to_doc_dir_clears(self):
        with tempfile.TemporaryDirectory() as tmp:
            doc_dir = os.path.join(tmp, "docs", "planning", "green-program", "tty")
            os.makedirs(os.path.join(doc_dir, "serials"))
            with open(os.path.join(doc_dir, "serials", "boot19.txt"), "w") as fh:
                fh.write("serial\n")
            text = "Full serial preserved at `serials/boot19.txt`.\n"
            rel = os.path.join("docs", "planning", "green-program", "tty", "EVIDENCE.md")
            findings = cl.lint_text(rel, text, repo_root=tmp)
            self.assertEqual(findings, [])

    def test_captured_serial_txt_is_skipped(self):
        # Review m3: nobody can annotate a claim inside a captured serial.
        with tempfile.TemporaryDirectory() as tmp:
            d = os.path.join(tmp, "docs", "arc", "serials")
            os.makedirs(d)
            path = os.path.join(d, "boot-1.txt")
            with open(path, "w") as fh:
                fh.write("every arm passed, zero reds, PROVEN\n")
            self.assertEqual(cl.lint_file(path, tmp), [])

    def test_hand_authored_prose_under_a_serials_dir_is_linted(self):
        # Review R2-B1: the R2 skip was by DIRECTORY, so the PROVE/RCA
        # narratives and mutation scripts that live beside the captures were
        # skipped too -- 30 files, 124 findings, including the file a
        # blocking finding of this campaign was raised against.
        with tempfile.TemporaryDirectory() as tmp:
            d = os.path.join(tmp, "docs", "arc", "serials")
            os.makedirs(d)
            for name, body in (
                ("fix2-prove.md", "Every arm of the gate passed.\n"),
                ("mutation1-apply.sh",
                 "#!/bin/bash\n# Every close path decrements the refcount.\n"),
            ):
                path = os.path.join(d, name)
                with open(path, "w") as fh:
                    fh.write(body)
                self.assertTrue(
                    any(f.rule == "universal-claim"
                        for f in cl.lint_file(path, tmp)),
                    "%s is hand-authored prose, not a capture" % name,
                )

    def test_a_source_dir_named_serial_is_not_a_capture_tree(self):
        # kernel/src/serial/ is a source directory whose only relationship to
        # the rule was the word "serial" in its name (review R2-B1).
        self.assertFalse(cl.is_capture_file("kernel/src/serial/command.rs"))
        self.assertFalse(cl.is_capture_file("docs/arc/serials/README.md"))
        self.assertTrue(cl.is_capture_file("docs/arc/serials/boot-1.txt"))
        self.assertTrue(cl.is_capture_file("docs/arc/confirm/run.log"))

    def test_cited_directory_does_not_clear_a_universal(self):
        # Review R2-M1: `os.path.exists` was true for a directory, so a
        # paragraph could be cleared by citing the folder beside the doc
        # instead of a file inside it.
        with tempfile.TemporaryDirectory() as tmp:
            doc_dir = os.path.join(tmp, "docs", "arc")
            os.makedirs(os.path.join(doc_dir, "serials"))
            rel = os.path.join("docs", "arc", "EVIDENCE.md")
            text = "Every serial referenced here is in `serials/`.\n"
            findings = cl.lint_text(rel, text, tmp)
            self.assertTrue(
                any(f.rule == "universal-claim" for f in findings),
                "a directory is not an artifact and cannot exempt a claim",
            )

    def test_archived_at_a_missing_path_is_flagged(self):
        # Review r2-m1: `archived` is the verb both of sweep2 B1's dangling
        # in-repo citations use, and it was not in the verb list.
        text = ("Every failing serial was archived at "
                "`docs/planning/green-program/tty/serials/no-such-file.txt`.\n")
        findings = cl.lint_text("x.md", text, REPO_ROOT)
        self.assertTrue(
            any(f.rule == "artifact-path-missing" for f in findings))

    def test_bare_word_review_does_not_discharge(self):
        # Review r2-m2: "review" as an English word did not name anything a
        # reader could open.
        text = ("<!-- claim-lint:ok: see the review -->\n"
                "Every close path decrements the refcount.\n")
        findings = cl.lint_text("x.md", text, REPO_ROOT)
        self.assertTrue(any(f.rule == "universal-claim" for f in findings))

    def test_named_review_file_discharges(self):
        text = ("<!-- claim-lint:ok: fix2-review.md B1 -->\n"
                "Every close path decrements the refcount.\n")
        self.assertEqual(cl.lint_text("x.md", text, REPO_ROOT), [])

    def test_named_test_function_discharges(self):
        text = ("<!-- claim-lint:ok: test_bare_universal_is_flagged -->\n"
                "Every close path decrements the refcount.\n")
        self.assertEqual(cl.lint_text("x.md", text, REPO_ROOT), [])

    def test_untracked_new_file_is_linted_in_diff_mode(self):
        # Review R2-M2: `git diff` does not list untracked files, so a
        # brand-new EVIDENCE doc -- the shape an arc starts with -- was
        # invisible in the mode the checklist prescribes.
        with tempfile.TemporaryDirectory() as tmp:
            def git(*args):
                return subprocess.run(["git"] + list(args), cwd=tmp,
                                      capture_output=True, text=True)
            git("init", "-q", "-b", "main")
            git("config", "user.email", "t@example.com")
            git("config", "user.name", "t")
            with open(os.path.join(tmp, "seed.md"), "w") as fh:
                fh.write("seed\n")
            git("add", "-A")
            git("commit", "-qm", "seed")
            base = git("rev-parse", "HEAD").stdout.strip()
            with open(os.path.join(tmp, "EVIDENCE-new.md"), "w") as fh:
                fh.write("Every arm of the gate was exercised.\n")
            out = subprocess.run(
                [sys.executable, CLAIM_LINT_PATH, "--base", base,
                 "--repo-root", tmp],
                cwd=tmp, capture_output=True, text=True,
            )
            self.assertIn("EVIDENCE-new.md", out.stdout,
                          "an untracked new file must be linted in diff mode")
            self.assertEqual(out.returncode, 1)

    def test_rust_doc_comment_is_scanned(self):
        text = "/// Every close path calls slave_close() unconditionally.\nfn f() {}\n"
        findings = cl.lint_text("x.rs", text)
        self.assertTrue(any(f.rule == "universal-claim" for f in findings))

    def test_shell_comment_is_scanned(self):
        text = "#!/bin/bash\n# Using the prior tick's count is airtight against misattribution.\necho hi\n"
        findings = cl.lint_text("x.sh", text)
        self.assertTrue(any(f.rule == "absolute-guarantee" for f in findings))

    def test_code_fence_is_not_scanned(self):
        text = "```\nEvery close path calls slave_close.\n```\n"
        findings = cl.lint_text("x.md", text)
        self.assertEqual(findings, [])

    def test_rust_code_line_is_not_scanned(self):
        text = 'let s = "every path is fine";\n'
        findings = cl.lint_text("x.rs", text)
        self.assertEqual(findings, [])

    def test_changed_hunk_intersection(self):
        # Review B2: diff mode reports only findings whose paragraph overlaps
        # a line the branch actually changed.
        f = cl.Finding("x.md", 10, "universal-claim", "t", "", 12)
        self.assertTrue(cl.overlaps(f, [(12, 14)]))
        self.assertTrue(cl.overlaps(f, [(1, 10)]))
        self.assertFalse(cl.overlaps(f, [(13, 20)]))
        self.assertFalse(cl.overlaps(f, [(1, 9)]))


class AutoCloseKeywordTests(unittest.TestCase):
    """The `auto-close-keyword` rule: a close/fix/resolve keyword bound to
    "#N" (or an equivalent GitHub reference -- see
    test_reference_forms_github_also_parses below) is not a claim that might
    be true or false -- it is text GitHub itself reads and acts on at
    merge/commit time, auto-closing the named issue regardless of anything a
    human wrote next to it. #737 auto-closed at the exact moment PR #799
    (2026-09-05) merged, ahead of the round's own, later, explicit close --
    what mechanism fired is not fully recoverable after the fact (the PR
    body as it reads today carries no such phrase, and GitHub does not
    expose body-edit history via its API), so this rule guards the shape
    rather than asserting which byte sequence caused that specific close.
    The convention from here on is a plain "#N" with no such keyword
    directly in front of it.

    Every literal `close`/`fix`/`resolve` + issue-number fixture below uses
    a number far outside this repo's real issue range (issue counts are in
    the hundreds; these are 8-digit) precisely so that pasting this file's
    bytes into a real PR body or commit message cannot re-trigger the
    incident this rule exists to prevent (review F6) -- #737, #4, #10 and
    #12 are real, merged issues/PRs in this repo, and the original fixtures
    spelled them out directly.
    """

    FAKE = "99999737"    # stands in for the incident's own #737
    FAKE2 = "99999912"   # stands in for #12
    FAKE3 = "99999904"   # stands in for #4
    FAKE4 = "99999910"   # stands in for #10
    FAKE5 = "99999901"   # stands in for #9001

    def test_vocabulary_and_shape(self):
        cases = [
            ("Fixes #%s\n" % self.FAKE, True),
            ("This is #%s\n" % self.FAKE, False),
            ("closes: #%s\n" % self.FAKE2, True),
            ("Fixed #%s.\n" % self.FAKE3, True),
            ("resolve #%s\n" % self.FAKE5, True),
            ("Close #%s for tracking.\n" % self.FAKE4, True),
            # a word that merely CONTAINS "close" is not the keyword --
            # disclose/foreclose etc. must not fire on the substring.
            ("This was disclosed #%s in the retro.\n" % self.FAKE, False),
            # the keyword and the reference have to be adjacent (only
            # whitespace/colon between); a keyword earlier in the sentence
            # with other words before the issue number is not the shape
            # GitHub's own parser recognizes.
            ("The bug closes out a class of issues, tracked as #%s.\n"
             % self.FAKE2, False),
        ]
        for text, expect_flag in cases:
            findings = cl.lint_text("x.md", text)
            hit = any(f.rule == "auto-close-keyword" for f in findings)
            self.assertEqual(
                hit, expect_flag,
                "text=%r expected auto-close-keyword=%r, findings=%r"
                % (text, expect_flag, [f.rule for f in findings]),
            )

    def test_reference_forms_github_also_parses(self):
        # Review F5: GitHub's own "linking a pull request to an issue" docs
        # also auto-close on a cross-repo `OWNER/REPO#N` reference and on a
        # full issue/PR URL; `GH-N` is the legacy autolink form from
        # GitHub's original closing-keywords announcement. These are live
        # GitHub-parsed shapes on their own merits, even though this tree
        # carried a different shape (a bare `#N`) when the rule shipped
        # (`grep -rIn 'breenix#[0-9]'` comes up empty).
        cases = [
            ("closes ryanbreen/breenix#%s\n" % self.FAKE, True),
            ("fixes https://github.com/ryanbreen/breenix/issues/%s\n"
             % self.FAKE, True),
            ("closes GH-%s\n" % self.FAKE, True),
            ("resolved gh-%s\n" % self.FAKE, True),
        ]
        for text, expect_flag in cases:
            findings = cl.lint_text("x.md", text)
            hit = any(f.rule == "auto-close-keyword" for f in findings)
            self.assertEqual(
                hit, expect_flag,
                "text=%r expected auto-close-keyword=%r, findings=%r"
                % (text, expect_flag, [f.rule for f in findings]),
            )

    def test_negation_still_fires_and_the_documented_rewrite_does_not(self):
        # Review F2: an honest-scoping negation ("this design does not
        # resolve #N") is NOT exempted, on purpose -- GitHub's own matcher
        # does not parse negation either (it looks for the keyword
        # immediately before "#N", and stops there), so "does not resolve #N"
        # is exactly as likely to auto-close on merge as "resolves #N"
        # would be. Exempting the negated form would reopen the incident
        # this rule exists to prevent, not close a false-positive gap.
        negated = "This design does not resolve #%s.\n" % self.FAKE
        findings = cl.lint_text("x.md", negated)
        self.assertTrue(
            any(f.rule == "auto-close-keyword" for f in findings),
            "a negated claim must still fire -- GitHub does not read 'not'",
        )
        # The rewrite this rule's own message and claim-linting.md recommend
        # -- break the keyword/number adjacency -- keeps the same meaning
        # and is not a GitHub-parsed shape either, so it is a safe way to
        # write the honest claim, not a way to dodge the linter.
        safe_rewrites = [
            "This design does not resolve issue #%s.\n" % self.FAKE,
            "#%s is not resolved by this design.\n" % self.FAKE,
        ]
        for text in safe_rewrites:
            findings = cl.lint_text("x.md", text)
            self.assertFalse(
                any(f.rule == "auto-close-keyword" for f in findings),
                "documented rewrite %r must not fire" % text,
            )

    def test_not_dischargeable_by_claim_lint_ok(self):
        # The annotation records that a human checked something; it does not
        # stop GitHub from reading the merged bytes and auto-closing the
        # issue. This rule must fire even with a citation-bearing annotation
        # right above it, unlike the rest of the rules in this file.
        text = ("<!-- claim-lint:ok: reviewed, see #728 -->\n"
                "Fixes #%s\n" % self.FAKE)
        findings = cl.lint_text("x.md", text)
        self.assertTrue(
            any(f.rule == "auto-close-keyword" for f in findings),
            "a claim-lint:ok annotation must not discharge auto-close-keyword",
        )

    def test_not_exempted_by_nm_count_or_evidence_path(self):
        # The other exemptions (N-of-M, a resolving evidence path) don't apply
        # here either: there is no true-or-false claim here for them to
        # exempt.
        with tempfile.TemporaryDirectory() as tmp:
            os.makedirs(os.path.join(tmp, "confirm"))
            with open(os.path.join(tmp, "confirm", "run.txt"), "w") as fh:
                fh.write("serial\n")
            text = ("Fixes #%s -- 12/12 arms green, per "
                    "`confirm/run.txt`.\n" % self.FAKE)
            findings = cl.lint_text("EVIDENCE.md", text, tmp)
            self.assertTrue(any(f.rule == "auto-close-keyword" for f in findings))

    def test_fires_in_rust_and_shell_comments_too(self):
        self.assertTrue(any(
            f.rule == "auto-close-keyword"
            for f in cl.lint_text(
                "x.rs", "/// Fixes #%s once merged.\nfn f() {}\n" % self.FAKE)
        ))
        self.assertTrue(any(
            f.rule == "auto-close-keyword"
            for f in cl.lint_text(
                "x.sh", "#!/bin/bash\n# closes: #%s\necho hi\n" % self.FAKE2)
        ))

    def test_extensionless_files_target_is_linted_not_skipped(self):
        # Review F3: `--files` is the round checklist's own required step for
        # a PR body or a scratchpad doc, and those surfaces do not
        # reliably carry a recognized extension -- `git commit`'s own
        # `COMMIT_EDITMSG` has no extension at all. Before the fix,
        # `lint_file()` returned `[]` for any unrecognized extension before
        # even opening the file, so a trigger phrase in an extensionless
        # file silently passed.
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "COMMIT_EDITMSG")
            with open(path, "w") as fh:
                fh.write("Fixes #%s\n" % self.FAKE)
            out = subprocess.run(
                [sys.executable, CLAIM_LINT_PATH, "--files", path],
                capture_output=True, text=True,
            )
            self.assertEqual(
                out.returncode, 1,
                "an extensionless --files target must be linted, not "
                "silently skipped and reported clean; stdout=%s" % out.stdout,
            )
            self.assertIn("auto-close-keyword", out.stdout)
            self.assertNotIn(
                "SKIPPED", out.stdout,
                "an explicit --files target must never be reported skipped",
            )

    def test_lint_file_distinguishes_skipped_from_clean(self):
        # Unit-level check of the skip-vs-clean distinction lint_file() now
        # makes: it signals a skip with a distinct return value, separate
        # from the empty list it returns for a file that was opened and
        # read clean. Folding the two together is exactly what let the
        # extensionless COMMIT_EDITMSG case above report "clean (1 file(s)
        # checked)" without the file being read.
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "no_extension_here")
            with open(path, "w") as fh:
                fh.write("Fixes #%s\n" % self.FAKE)
            self.assertIsNone(
                cl.lint_file(path, tmp, assume_text=False),
                "an unrecognized extension must be skipped (None), not "
                "silently linted, unless the caller opts in",
            )
            found = cl.lint_file(path, tmp, assume_text=True)
            self.assertIsNotNone(found)
            self.assertTrue(any(f.rule == "auto-close-keyword" for f in found))

    def test_skipped_targets_reported_separately_from_checked(self):
        # Auto-discovered targets (whole-repo or diff mode) keep the strict
        # extension allowlist -- but a skip must be REPORTED, not folded
        # into "checked" (review F3's second half: the summary line used to
        # count a skipped target as checked no matter which mode found it).
        with tempfile.TemporaryDirectory() as tmp:
            vc = "git"
            subprocess.run([vc, "init", "-q"], cwd=tmp, check=True)
            subprocess.run([vc, "config", "user.email", "t@example.com"],
                            cwd=tmp, check=True)
            subprocess.run([vc, "config", "user.name", "t"],
                            cwd=tmp, check=True)
            with open(os.path.join(tmp, "clean.md"), "w") as fh:
                fh.write("This change is a small, targeted fix.\n")
            with open(os.path.join(tmp, "notes.bin"), "w") as fh:
                fh.write("Fixes #%s\n" % self.FAKE)
            subprocess.run([vc, "add", "clean.md", "notes.bin"],
                            cwd=tmp, check=True)
            subprocess.run([vc, "commit", "-q", "-m", "seed"],
                            cwd=tmp, check=True)
            out = subprocess.run(
                [sys.executable, CLAIM_LINT_PATH, "--all", "--repo-root", tmp],
                capture_output=True, text=True,
            )
            self.assertEqual(out.returncode, 0, out.stdout)
            self.assertIn("1 file(s) checked", out.stdout)
            self.assertIn("1 target(s) SKIPPED", out.stdout)
            self.assertIn("notes.bin", out.stdout)

    # ------------------------------------------------------------------
    # ANTI-VACUITY: the real CLI, via `--files`, on three short one-line
    # fixtures -- the shape a human actually runs, not just the in-process
    # library call above. Asserts the literal process exit code.
    # ------------------------------------------------------------------
    def _cli_cases(self):
        return [
            ("Fixes #%s\n" % self.FAKE, 1),
            ("This is #%s\n" % self.FAKE, 0),
            ("closes: #%s\n" % self.FAKE2, 1),
        ]

    def test_cli_exit_codes_for_the_three_anti_vacuity_cases(self):
        with tempfile.TemporaryDirectory() as tmp:
            for i, (text, expected_exit) in enumerate(self._cli_cases()):
                path = os.path.join(tmp, "case%d.md" % i)
                with open(path, "w") as fh:
                    fh.write(text)
                out = subprocess.run(
                    [sys.executable, CLAIM_LINT_PATH, "--files", path],
                    capture_output=True, text=True,
                )
                self.assertEqual(
                    out.returncode, expected_exit,
                    "text=%r expected exit %d, got %d; stdout=%s"
                    % (text, expected_exit, out.returncode, out.stdout),
                )
                if expected_exit == 1:
                    self.assertIn(
                        "auto-close-keyword", out.stdout,
                        "a rejecting case must name the rule that fired",
                    )
                else:
                    self.assertNotIn("auto-close-keyword", out.stdout)


class CommitMsgModeTests(unittest.TestCase):
    """`--commit-msg <file>` (R21): lints a git commit message as prose,
    with every rule, regardless of the file's extension -- a commit message
    saved to a temp file for `git commit -F` typically has none. The
    fixture set below is the round's own anti-vacuity leg for this mode; see
    docs/planning/green-program/claim-linting.md's `--commit-msg` section
    for the incident it exists to catch: commit `e6dd14a6` quoted this
    rule's own trigger vocabulary immediately in front of three real, open
    issue numbers inside its own commit message while describing a
    rewording made in a DIFFERENT file, and GitHub auto-closed all three the
    moment the commit landed on `main` -- a shape neither of the round
    checklist's two mandated runs (the tree diff, the PR body) reads.

    Every issue number below is synthetic and out of this repo's real range
    (`AutoCloseKeywordTests.FAKE*` explains why: pasting a fixture's own
    bytes into a real commit message must not be able to reproduce the
    incident these tests exist to prevent).
    """

    FAKE = "99999562"    # stands in for #562
    FAKE2 = "99999761"   # stands in for #761
    FAKE3 = "99999763"   # stands in for #763

    def _run_cli(self, text):
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "COMMIT_EDITMSG")
            with open(path, "w") as fh:
                fh.write(text)
            return subprocess.run(
                [sys.executable, CLAIM_LINT_PATH, "--commit-msg", path],
                capture_output=True, text=True,
            )

    # ------------------------------------------------------------------
    # The four required fixtures, run through the real CLI.
    # ------------------------------------------------------------------

    def test_quoted_close_keyword_inside_double_quotes_is_fatal(self):
        # This is the e6dd14a6 shape itself: the keyword-adjacent-to-#N text
        # sits inside a quoted, past-tense description of a rewording made
        # elsewhere -- not a live directive the author intended -- and
        # GitHub's parser does not care about the quotes or the intent.
        text = ('docs: reword an example that used to read "closes #%s" '
                'next to an issue number.\n' % self.FAKE)
        out = self._run_cli(text)
        self.assertEqual(out.returncode, 1, out.stdout)
        self.assertIn("auto-close-keyword", out.stdout)

    def test_broken_spelling_is_clean(self):
        # The documented way to describe the shape without triggering it
        # (claim-linting.md's own `Clos<ed> #N` convention): syntactically
        # unable to match AUTO_CLOSE_KEYWORD_RE or GitHub's own parser.
        text = "docs: rewrite the example so it reads clos<es> #%s.\n" % self.FAKE2
        out = self._run_cli(text)
        self.assertEqual(out.returncode, 0, out.stdout)

    def test_plain_issue_reference_is_clean(self):
        text = "fix(786): repair the census scan (#%s).\n" % self.FAKE3
        out = self._run_cli(text)
        self.assertEqual(out.returncode, 0, out.stdout)

    def test_unquantified_universal_still_fires(self):
        # The other five rules apply in --commit-msg mode too -- this is not
        # an auto-close-keyword-only mode.
        text = "fix: every close path now decrements the refcount.\n"
        out = self._run_cli(text)
        self.assertEqual(out.returncode, 1, out.stdout)
        self.assertIn("universal-claim", out.stdout)

    # ------------------------------------------------------------------
    # The "even inside quotes/backticks" behaviour this mode adds over the
    # normal (--files) path: a fenced example does not shield the keyword.
    # ------------------------------------------------------------------

    def test_fenced_example_is_fatal_in_commit_msg_mode_but_not_in_files_mode(self):
        text = "docs: quote the bad example.\n\n```\ncloses #%s\n```\n" % self.FAKE
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "msg.md")
            with open(path, "w") as fh:
                fh.write(text)
            commit_msg_out = subprocess.run(
                [sys.executable, CLAIM_LINT_PATH, "--commit-msg", path],
                capture_output=True, text=True,
            )
            files_out = subprocess.run(
                [sys.executable, CLAIM_LINT_PATH, "--files", path],
                capture_output=True, text=True,
            )
        self.assertEqual(
            commit_msg_out.returncode, 1,
            "a fenced auto-close phrase must still fire in --commit-msg "
            "mode: %s" % commit_msg_out.stdout,
        )
        self.assertIn("auto-close-keyword", commit_msg_out.stdout)
        self.assertEqual(
            files_out.returncode, 0,
            "baseline contrast: the normal doc-linting path blanks fenced "
            "code and must NOT fire here -- if it does, this is no longer "
            "demonstrating what --commit-msg mode adds: %s" % files_out.stdout,
        )

    # ------------------------------------------------------------------
    # In-process unit checks against lint_commit_msg_text() directly.
    # ------------------------------------------------------------------

    def test_lint_commit_msg_text_runs_all_six_rules(self):
        findings = cl.lint_commit_msg_text(
            "Every close path is airtight, was proven, and was observed live.\n",
            REPO_ROOT)
        fired = {f.rule for f in findings}
        self.assertEqual(
            fired,
            {"universal-claim", "absolute-guarantee", "unproven-claim",
             "live-no-artifact"},
        )

    def test_lint_commit_msg_text_no_duplicate_auto_close_finding(self):
        # Pass 1 skips auto-close-keyword and pass 2 runs ONLY it, unfenced --
        # an ordinary (unfenced) occurrence must be reported exactly once, not
        # twice (1 of 1, not 2 of 1).
        findings = cl.lint_commit_msg_text(
            "Fixes #%s\n" % self.FAKE, REPO_ROOT)
        auto_close = [f for f in findings if f.rule == "auto-close-keyword"]
        self.assertEqual(len(auto_close), 1, auto_close)

    def test_extensionless_file_is_linted_as_prose(self):
        # A `git commit -F`-style temp file, and .git/COMMIT_EDITMSG itself,
        # carry no extension.
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "COMMIT_EDITMSG")
            with open(path, "w") as fh:
                fh.write("Fixes #%s\n" % self.FAKE)
            out = subprocess.run(
                [sys.executable, CLAIM_LINT_PATH, "--commit-msg", path],
                capture_output=True, text=True,
            )
            self.assertEqual(out.returncode, 1, out.stdout)
            self.assertNotIn("SKIPPED", out.stdout)


class HistoricalCorpusInContextTests(unittest.TestCase):
    """Layer 2a (HEADLINE): the linter run over the whole file as it shipped.

    This is the mode the tool actually runs in. A specimen counts as CAUGHT
    only when a finding's own paragraph contains the offending sentence."""

    @classmethod
    def setUpClass(cls):
        with open(os.path.join(CORPUS_DIR, "historical_false_claims.json")) as fh:
            cls.corpus = json.load(fh)

    def test_in_context_catch_rate_and_report(self):
        recoverable = [e for e in self.corpus if e.get("surface") == "repo"]
        caught, missed, unreachable = [], [], []
        for e in recoverable:
            content = git_show(e["shipped_commit"], e["shipped_path"])
            if content is None:
                unreachable.append(e)
                continue
            # Anti-vacuity: if the sentence is not in the recovered bytes
            # there is no measurement here. Fail loudly, don't report MISS.
            self.assertIn(
                e["probe"], content,
                "%s: probe not present at %s:%s -- the corpus metadata is "
                "stale, not the linter" % (e["id"], e["shipped_commit"],
                                           e["shipped_path"]),
            )
            findings = cl.lint_text(e["shipped_path"], content, REPO_ROOT)
            hit = [f for f in findings if norm(e["probe"]) in norm(f.text)]
            if hit:
                e = dict(e, on_claim=any(
                    trigger_is_in(f.rule, e["probe"]) for f in hit))
                caught.append(e)
            else:
                missed.append(e)

        if unreachable:
            self.skipTest(
                "this checkout cannot reach %d shipped commit(s): %s"
                % (len(unreachable), [e["id"] for e in unreachable])
            )

        total = len(recoverable)
        on_claim = [e for e in caught if e.get("on_claim")]
        print(
            "\n[historical corpus, IN CONTEXT -- whole file as shipped] "
            "caught %d of %d recoverable specimens; %d of those %d catches "
            "fire on a rule whose trigger is inside the false sentence "
            "itself, the rest only on a neighbouring sentence of the same "
            "paragraph:" % (len(caught), total, len(on_claim), len(caught))
        )
        for e in caught:
            print("  CAUGHT %-4s %-9s %s@%s"
                  % (e["id"], "on-claim" if e.get("on_claim") else "incidental",
                     e["shipped_path"].split("/")[-1], e["shipped_commit"]))
        for e in missed:
            print("  MISS   %-4s %s@%s" % (e["id"], e["shipped_path"].split("/")[-1],
                                           e["shipped_commit"]))
            reason = e.get("in_context_known_miss") or e.get("known_miss")
            if reason:
                print("         reason: %s" % reason)
        print("  (%d further specimen(s) shipped on a surface this repo does "
              "not contain: %s -- isolated-quote layer only)"
              % (len(self.corpus) - total,
                 [e["id"] for e in self.corpus if e.get("surface") != "repo"]))

        expected = [e for e in recoverable
                    if "in_context_known_miss" not in e and "known_miss" not in e]
        regressed = [e for e in expected if e in missed]
        self.assertEqual(
            regressed, [],
            "regression: previously in-context-caught false claims are no "
            "longer caught: %s" % [e["id"] for e in regressed],
        )
        stale = [e for e in recoverable
                 if ("in_context_known_miss" in e or "known_miss" in e)
                 and e in caught]
        self.assertEqual(
            stale, [],
            "these entries are marked a known in-context miss but are now "
            "caught -- update historical_false_claims.json: %s"
            % [e["id"] for e in stale],
        )


class HistoricalCorpusIsolatedQuoteTests(unittest.TestCase):
    """Layer 2b (SECONDARY, over-reports): each quote as a standalone file.

    A lone sentence has no document around it, so nothing in the paragraph can
    exempt it -- this rate is higher than the in-context rate and must never
    be published as the tool's catch rate. It is kept because it is the only
    layer that covers the six specimens whose shipped surface this repo does
    not contain."""

    @classmethod
    def setUpClass(cls):
        with open(os.path.join(CORPUS_DIR, "historical_false_claims.json")) as fh:
            cls.corpus = json.load(fh)

    def test_isolated_quote_catch_rate_and_report(self):
        caught, missed = [], []
        for entry in self.corpus:
            content = wrap_quote(entry["quote"], entry["filetype"])
            findings = cl.lint_text("spec" + entry["filetype"], content)
            fired_rules = {f.rule for f in findings}
            expected = set(entry.get("expected_rules", []))
            hit = bool(fired_rules & expected) if expected else bool(fired_rules)
            (caught if hit else missed).append(entry)

        total = len(self.corpus)
        print(
            "\n[historical corpus, ISOLATED QUOTES -- secondary, over-reports] "
            "caught %d of %d:" % (len(caught), total)
        )
        for e in caught:
            print("  CAUGHT %-4s %s" % (e["id"], e["source"]))
        for e in missed:
            print("  MISS   %-4s %s" % (e["id"], e["source"]))
            if "known_miss" in e:
                print("         reason: %s" % e["known_miss"])

        # claim-lint:ok: enforced by the assertEqual immediately below, whose
        # specimens are scripts/claim_lint_corpus/historical_false_claims.json.
        # Regression gate: every entry not declared a known miss must still
        # be caught. A drop here means a rule regressed, not that the corpus
        # changed -- fix the rule or, if the specimen genuinely can't be
        # caught mechanically, move it to `known_miss` with a stated reason
        # (that is itself a claim-discipline act, not a shortcut).
        expected_catches = [e for e in self.corpus if "known_miss" not in e]
        regressed = [e for e in expected_catches if e in missed]
        self.assertEqual(
            regressed, [],
            "regression: previously-caught false claims are no longer caught: %s"
            % [e["id"] for e in regressed],
        )
        stale_known_miss = [e for e in self.corpus if "known_miss" in e and e in caught]
        self.assertEqual(
            stale_known_miss, [],
            "these entries are marked known_miss but the linter now catches "
            "them -- update historical_false_claims.json: %s"
            % [e["id"] for e in stale_known_miss],
        )


class VerifiedTrueClaimsTests(unittest.TestCase):
    """Layer 3a: hand-picked sentences the review process verified TRUE.
    Reports how many need a claim-lint:ok annotation to pass -- that is
    expected friction, not a bug, and is called out per-entry in the
    corpus file's `note` field."""

    @classmethod
    def setUpClass(cls):
        with open(os.path.join(CORPUS_DIR, "verified_true_claims.json")) as fh:
            cls.corpus = json.load(fh)

    # Entries expected to pass clean (they carry a same-paragraph citation or
    # no trigger word survives exemption) vs. entries expected to need a
    # claim-lint:ok annotation despite being true (no citation in the
    # sentence) -- both are asserted, so a regression in either direction
    # (a citation stops being recognized, or an exemption gets too loose)
    # fails the suite instead of silently drifting.
    EXPECT_CLEAN = {"T3", "T4"}
    EXPECT_FLAGGED = {"T1", "T2"}

    def test_report_false_positive_shape(self):
        print("\n[verified-true corpus]")
        for entry in self.corpus:
            content = wrap_quote(entry["quote"], entry["filetype"])
            findings = cl.lint_text("spec" + entry["filetype"], content, REPO_ROOT)
            status = "FLAGGED (needs claim-lint:ok)" if findings else "clean"
            print("  %-4s %-30s %s" % (entry["id"], status, entry["source"]))
            if entry["id"] in self.EXPECT_CLEAN:
                self.assertEqual(
                    findings, [],
                    "%s was expected to pass clean but got flagged: %s"
                    % (entry["id"], [f.rule for f in findings]),
                )
            elif entry["id"] in self.EXPECT_FLAGGED:
                self.assertTrue(
                    findings,
                    "%s was expected to need annotation but passed clean -- "
                    "an exemption may have gotten too loose" % entry["id"],
                )


class RealDocumentReportTests(unittest.TestCase):
    """Layer 3b: run against real, currently-in-repo evidence docs the
    program already treats as correct. Informational -- these documents are
    edited independently of this tool, so this does not hard-fail the suite,
    but it does print the honest count every run, which is the point."""

    def _report(self, rel_path):
        path = os.path.join(REPO_ROOT, rel_path)
        if not os.path.exists(path):
            self.skipTest("%s not present in this checkout" % rel_path)
        findings = cl.lint_file(path, REPO_ROOT)
        with open(path) as fh:
            content = fh.read()
        paragraphs = cl.extract_paragraphs(rel_path, content)
        flagged = len({f.line for f in findings})
        by_rule = {}
        for f in findings:
            by_rule[f.rule] = by_rule.get(f.rule, 0) + 1
        print(
            "\n[real doc] %s: %d paragraph(s), %d flagged, %d finding(s) %s"
            % (rel_path, len(paragraphs), flagged, len(findings), by_rule or "{}")
        )
        return findings, paragraphs

    def test_workload_envelopes(self):
        self._report("docs/planning/green-program/WORKLOAD-ENVELOPES.md")

    def test_tty_evidence(self):
        self._report("docs/planning/green-program/tty/EVIDENCE-2026-08-30.md")


class PerRoundLoadReportTests(unittest.TestCase):
    """Layer 4: what an author actually faces on a real fix round, reported
    both ways. Whole-file is what the tool did before the --changed-only
    default; changed-hunks-only is the shipping default. Report, not gate."""

    # Eleven rounds, not three. R2 published a per-round range measured on the
    # three rounds it chose; the R2 review measured eight more of its own and
    # the range did not hold (review R2-M4). The eleven are replayed here so
    # the published range is re-derived on each run, and so the set is not
    # the set that produced the flattering number.
    ROUNDS = [
        ("aa5f0fd8", "#721 fix round"),
        ("a6679e7c", "sweep-3 fix round"),
        ("9a77c3dc", "#748 fix round"),
        ("6ba3bcc4", "R4 doc fix round"),
        ("2a2328aa", "coreproof rung-2 prove"),
        ("73c58fda", "#540 x86 prod gate"),
        ("16d6ff5b", "x86 TTY oracle gate"),
        ("cbc6873b", "nic-bus doc-truth"),
        ("1f098d11", "tracing x86 evidence"),
        ("06a1c1a6", "TTY x86 fix-round"),
        ("5777bb7b", "#717 trap guard"),
    ]

    def test_per_round_load(self):
        head = subprocess.run(["git", "rev-parse", "--verify", "--quiet",
                               self.ROUNDS[0][0]], capture_output=True,
                              text=True, cwd=REPO_ROOT)
        if head.returncode != 0:
            self.skipTest("this checkout does not contain the round commits")
        print("\n[per-round load]")
        wholes, changeds = [], []
        for commit, label in self.ROUNDS:
            files = subprocess.run(
                ["git", "diff", "--name-only", "--diff-filter=ACMR",
                 "%s^" % commit, commit],
                capture_output=True, text=True, cwd=REPO_ROOT,
            ).stdout.split()
            whole = changed = nfiles = 0
            for rel in files:
                ext = os.path.splitext(rel)[1]
                if ext not in cl.TEXT_EXTENSIONS:
                    continue
                if cl.is_capture_file(rel):
                    continue
                nfiles += 1
                content = subprocess.run(
                    ["git", "show", "%s:%s" % (commit, rel)],
                    capture_output=True, text=True, cwd=REPO_ROOT,
                ).stdout
                found = cl.lint_text(rel, content, REPO_ROOT)
                whole += len(found)
                ranges = cl.changed_line_ranges("%s^" % commit, rel, REPO_ROOT,
                                               head=commit)
                changed += len([f for f in found if cl.overlaps(f, ranges)])
            wholes.append(whole)
            changeds.append(changed)
            print("  %s (%-22s) files=%2d  whole-file=%3d  changed-hunks=%3d"
                  % (commit, label, nfiles, whole, changed))
        print("  RANGE over %d rounds: whole-file %d-%d, changed-hunks %d-%d; "
              "hunk scoping suppressed %d%%-%d%% per round"
              % (len(self.ROUNDS), min(wholes), max(wholes),
                 min(changeds), max(changeds),
                 min(round(100 * (w - c) / w) for w, c in zip(wholes, changeds) if w),
                 max(round(100 * (w - c) / w) for w, c in zip(wholes, changeds) if w)))


if __name__ == "__main__":
    unittest.main(verbosity=2)
