#!/usr/bin/env python3
"""Check the frozen-copy manifest, scripts/frozen-copies.json (ruling 30, #1719).

Fails (exit 1) when a listed file is missing or its bytes differ from the recorded
sha256, when a listed file is not exempt from whitespace checks, or when any other
tracked file is exempt (its `whitespace` attribute unset, so `git diff --check` skips
it) without being a machine capture as claim-lint defines one (`is_capture_file`: a
`.txt` or `.log` file under a `serials/` or `confirm/` directory).

Run: python3 scripts/check-frozen-copies.py
"""
import hashlib
import importlib.util
import json
import os
import subprocess
import sys

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MANIFEST = os.path.join(REPO_ROOT, "scripts", "frozen-copies.json")


def capture_rule():
    spec = importlib.util.spec_from_file_location("claim_lint", os.path.join(REPO_ROOT, "scripts", "claim-lint.py"))
    mod = importlib.util.module_from_spec(spec)
    sys.modules["claim_lint"] = mod  # its dataclasses look their module up
    spec.loader.exec_module(mod)
    return mod.is_capture_file


def load(manifest=MANIFEST):
    with open(manifest, encoding="utf-8") as fh:
        return json.load(fh)["copies"]


def whitespace_exempt(repo_root=REPO_ROOT):
    """Tracked files whose `whitespace` attribute is unset."""
    files = subprocess.run(["git", "ls-files", "-z"], cwd=repo_root, capture_output=True, check=True).stdout
    out = subprocess.run(
        ["git", "check-attr", "-z", "--stdin", "whitespace"],
        cwd=repo_root, input=files, capture_output=True, check=True,
    ).stdout.split(b"\0")
    # -z output is path, attribute, value triples.
    return {out[i].decode() for i in range(0, len(out) - 2, 3) if out[i + 2] == b"unset"}


def main():
    problems = []
    copies = load()
    listed = {c["path"] for c in copies}
    for c in copies:
        path = os.path.join(REPO_ROOT, c["path"])
        try:
            with open(path, "rb") as fh:
                digest = hashlib.sha256(fh.read()).hexdigest()
        except FileNotFoundError:
            problems.append("%s: listed but missing" % c["path"])
            continue
        if digest != c["sha256"]:
            problems.append("%s: sha256 %s, manifest records %s (copied from %s:%s)"
                            % (c["path"], digest, c["sha256"], c["source_tag"], c["source_path"]))
    is_capture = capture_rule()
    exempt = whitespace_exempt()
    for p in sorted(p for p in exempt - listed if not is_capture(p)):
        problems.append("%s: exempt from whitespace checks but not in the manifest" % p)
    for p in sorted(listed - exempt):
        problems.append("%s: in the manifest but not exempt from whitespace checks (.gitattributes)" % p)
    for p in problems:
        print("frozen-copies: " + p)
    if problems:
        return 1
    print("frozen-copies: %d file(s) match their recorded sha256; every other whitespace exemption is a machine capture." % len(copies))
    return 0


if __name__ == "__main__":
    sys.exit(main())
