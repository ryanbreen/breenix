#!/usr/bin/env python3
"""
Tests for scripts/claim-lint.py.

Three layers:

1. Unit tests on the rule mechanics (discharge annotation, N-of-M exemption,
   idiom exemption, artifact-path existence check) -- small, hand-built
   fixtures with an unambiguous right answer.

2. The historical corpus: scripts/claim_lint_corpus/historical_false_claims.json
   holds verbatim false claims this campaign's review slot actually caught,
   each with the file:line/issue it shipped in and a citation into the
   review that quotes it. This asserts every entry NOT marked `known_miss`
   is still caught (a regression test against detector rot) and reports the
   historical catch rate as N of M, naming the misses and why, exactly as
   the task requires -- run directly (not assumed) every time this suite runs.

3. verified_true_claims.json + two real, currently-in-repo evidence docs
   (WORKLOAD-ENVELOPES.md, tty/EVIDENCE-2026-08-30.md) measure the false-
   positive rate against prose the review process actually accepted as true.
   The real-doc pass is a report, not a brittle pass/fail gate -- these docs
   are edited independently of this tool and an exact count is not a
   contract.

Run: python3 scripts/test_claim_lint.py [-v]
"""
import importlib.util
import json
import os
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


class MechanicsTests(unittest.TestCase):
    """Layer 1: the rule mechanics in isolation."""

    def test_bare_universal_is_flagged(self):
        findings = cl.lint_text("x.md", "Every close path decrements the refcount.\n")
        self.assertTrue(any(f.rule == "universal-claim" for f in findings))

    def test_discharge_annotation_clears_a_universal(self):
        text = (
            "Every close path decrements the refcount.\n"
            "<!-- claim-lint:ok: mutation-proven, see fix2-review.md B1 -->\n"
        )
        findings = cl.lint_text("x.md", text)
        self.assertEqual(findings, [])

    def test_nm_count_clears_a_universal(self):
        text = "Every one of the 12/12 arms passed the baseline run.\n"
        findings = cl.lint_text("x.md", text)
        self.assertEqual(findings, [])

    def test_evidence_log_path_clears_a_universal(self):
        text = "All boots reached the clean verdict, per `confirm/aarch64-run1-serial.txt`.\n"
        findings = cl.lint_text("x.md", text)
        self.assertEqual(findings, [])

    def test_source_path_does_not_clear_a_universal(self):
        # claim-lint:ok: this comment states the test's own premise, proven
        # by the assertion three lines below it, not asserted on its own.
        # The gtty BLOCKING-1 shape: citing the *.rs file the claim is about
        # does not establish the claim across every caller.
        text = (
            "while every close path (`kernel/src/ipc/fd.rs`, "
            "`kernel/src/task/process_task.rs`) calls `pair.slave_close()` "
            "unconditionally for any released slave\n"
        )
        findings = cl.lint_text("x.md", text)
        self.assertTrue(any(f.rule == "universal-claim" for f in findings))

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

    def test_observed_live_without_path_is_flagged(self):
        text = "(observed live: EXT2_LOCK_PARK_FIRST fired during an unrelated test)\n"
        findings = cl.lint_text("x.md", text)
        self.assertTrue(any(f.rule == "live-no-artifact" for f in findings))

    def test_observed_live_with_serial_path_clears(self):
        text = (
            "(observed live in `serials/aarch64-boot19.txt`: the marker fired "
            "after the leg's threads ran)\n"
        )
        findings = cl.lint_text("x.md", text)
        self.assertEqual(findings, [])

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


class HistoricalCorpusTests(unittest.TestCase):
    """Layer 2: does the linter, run for real, catch the actual false claims
    this campaign's review slot found? This is the N-of-M deliverable."""

    @classmethod
    def setUpClass(cls):
        with open(os.path.join(CORPUS_DIR, "historical_false_claims.json")) as fh:
            cls.corpus = json.load(fh)

    def test_catch_rate_and_report(self):
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
            "\n[historical corpus] caught %d of %d real false claims:"
            % (len(caught), total)
        )
        for e in caught:
            print("  CAUGHT %-4s %s" % (e["id"], e["source"]))
        for e in missed:
            print("  MISS   %-4s %s" % (e["id"], e["source"]))
            if "known_miss" in e:
                print("         reason: %s" % e["known_miss"])

        # claim-lint:ok: enforced by the assertEqual immediately below, not
        # asserted in this comment alone.
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
        # Sanity: a corpus entry marked known_miss that starts passing isn't a
        # bug -- but the metadata is now stale. Fail loudly so it gets moved.
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
    EXPECT_CLEAN = {"T1", "T3", "T4"}
    EXPECT_FLAGGED = {"T2"}

    def test_report_false_positive_shape(self):
        print("\n[verified-true corpus]")
        for entry in self.corpus:
            content = wrap_quote(entry["quote"], entry["filetype"])
            findings = cl.lint_text("spec" + entry["filetype"], content)
            status = "FLAGGED (needs claim-lint:ok)" if findings else "clean"
            print("  %-4s %-30s %s" % (entry["id"], status, entry["source"]))
            if entry["id"] in self.EXPECT_CLEAN:
                self.assertEqual(
                    findings, [],
                    "%s was expected to pass clean (has a same-paragraph "
                    "citation) but got flagged: %s"
                    % (entry["id"], [f.rule for f in findings]),
                )
            elif entry["id"] in self.EXPECT_FLAGGED:
                self.assertTrue(
                    findings,
                    "%s was expected to need annotation (true claim, no "
                    "citation) but passed clean -- an exemption may have "
                    "gotten too loose" % entry["id"],
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
        by_rule = {}
        for f in findings:
            by_rule[f.rule] = by_rule.get(f.rule, 0) + 1
        print(
            "\n[real doc] %s: %d paragraph(s), %d finding(s) %s"
            % (rel_path, len(paragraphs), len(findings), by_rule or "{}")
        )
        return findings, paragraphs

    def test_workload_envelopes(self):
        self._report("docs/planning/green-program/WORKLOAD-ENVELOPES.md")

    def test_tty_evidence(self):
        self._report("docs/planning/green-program/tty/EVIDENCE-2026-08-30.md")


if __name__ == "__main__":
    unittest.main(verbosity=2)
