#!/usr/bin/env python3
"""Run the N1570 constraint fixtures against a host-built bcc."""

import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile


CORPUS = Path(__file__).resolve().parents[2] / "corpus" / "c11"


def main(compiler):
    manifest = json.loads((CORPUS / "constraints.json").read_text())
    fixtures = manifest["fixtures"]
    clauses = {entry["clause"] for entry in fixtures}
    paths = {entry["file"] for entry in fixtures}
    assert fixtures and len(clauses) == len(paths) == len(fixtures)
    assert paths == {path.relative_to(CORPUS).as_posix()
                     for path in (CORPUS / "constraints").glob("*.c")}
    guest = set(manifest["guest"].values())
    assert 0 < len(guest) == len(manifest["guest"]) <= 10
    assert guest <= paths

    failures = []
    with tempfile.TemporaryDirectory(prefix="bcc-c11-constraints-") as directory:
        for entry in fixtures:
            clause, source, diagnostic = (entry[key] for key in ("clause", "file", "diagnostic"))
            assert re.fullmatch(r"6(?:\.\d+)+p\d+", clause)
            assert re.fullmatch(re.escape(source) + r":[1-9]\d*:[1-9]\d*: error: .+", diagnostic)
            for target in ("x86_64", "aarch64"):
                output = Path(directory) / f"{Path(source).stem}-{target}.s"
                result = subprocess.run(
                    [compiler, f"--target={target}", "-S", "-o", output, source],
                    cwd=CORPUS,
                    capture_output=True,
                    timeout=3,
                )
                wrote_output = output.exists()
                if (result.returncode, result.stdout, result.stderr, wrote_output) != (
                    1, b"", (diagnostic + "\n").encode(), False,
                ):
                    failures.append(
                        f"{clause} ({target}) expected {diagnostic!r} and no output file\n"
                        f"  status={result.returncode}, stdout={result.stdout!r}, "
                        f"stderr={result.stderr!r}, output_file={wrote_output}"
                    )

    if failures:
        sys.exit("\n".join(failures))


if __name__ == "__main__":
    main(sys.argv[1])
