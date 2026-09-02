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

    def test_capture_directories_are_skipped(self):
        # Review m3: nobody can annotate a claim inside a captured serial.
        with tempfile.TemporaryDirectory() as tmp:
            d = os.path.join(tmp, "docs", "arc", "serials")
            os.makedirs(d)
            path = os.path.join(d, "boot-1.txt")
            with open(path, "w") as fh:
                fh.write("every arm passed, zero reds, PROVEN\n")
            self.assertEqual(cl.lint_file(path, tmp), [])

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
            (caught if hit else missed).append(e)

        if unreachable:
            self.skipTest(
                "this checkout cannot reach %d shipped commit(s): %s"
                % (len(unreachable), [e["id"] for e in unreachable])
            )

        total = len(recoverable)
        print(
            "\n[historical corpus, IN CONTEXT -- whole file as shipped] "
            "caught %d of %d recoverable specimens:" % (len(caught), total)
        )
        for e in caught:
            print("  CAUGHT %-4s %s@%s" % (e["id"], e["shipped_path"].split("/")[-1],
                                           e["shipped_commit"]))
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

    ROUNDS = [
        ("aa5f0fd8", "#721 fix round"),
        ("a6679e7c", "sweep-3 fix round"),
        ("9a77c3dc", "#748 fix round"),
    ]

    def test_per_round_load(self):
        head = subprocess.run(["git", "rev-parse", "--verify", "--quiet",
                               self.ROUNDS[0][0]], capture_output=True,
                              text=True, cwd=REPO_ROOT)
        if head.returncode != 0:
            self.skipTest("this checkout does not contain the round commits")
        print("\n[per-round load]")
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
                if cl.CAPTURE_DIR_RE.search(rel):
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
            print("  %s (%-18s) files=%2d  whole-file=%3d  changed-hunks=%3d"
                  % (commit, label, nfiles, whole, changed))


if __name__ == "__main__":
    unittest.main(verbosity=2)
