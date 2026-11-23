"""Compare patched trees and complete image configs for E25 BusyBox."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile


def tree_digests(root):
    assert root.is_dir(), f"missing patched tree: {root}"
    return {
        path.relative_to(root).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(root.rglob("*")) if path.is_file() and not path.is_symlink()
    }


def compare(expected, actual, label):
    assert expected and actual, f"{label}: empty input"
    differences = sorted(key for key in expected.keys() | actual.keys()
                         if expected.get(key) != actual.get(key))
    assert not differences, f"{label}: {len(differences)} differences: {differences[:20]}"
    return len(expected)


def patch_snapshot(archive, series, applet):
    with tempfile.TemporaryDirectory(prefix="busybox-patch-parity-") as temporary:
        directory = Path(temporary)
        subprocess.run(["tar", "-xjf", archive, "-C", directory], check=True)
        source = directory / "busybox-1.37.0"
        for name in series.read_text().splitlines():
            if not name or name.startswith("#"):
                continue
            with series.with_name(name).open("rb") as patch:
                result = subprocess.run([applet, "-p1"], cwd=source, stdin=patch,
                                        stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
            assert result.returncode == 0, f"{name}: {result.stdout.decode(errors='replace')}"
        print(json.dumps(tree_digests(source), sort_keys=True))


def config_options(path):
    options = {}
    for line in path.read_text().splitlines():
        enabled = re.fullmatch(r"(CONFIG_[A-Z0-9_]+)=(.*)", line)
        disabled = re.fullmatch(r"# (CONFIG_[A-Z0-9_]+) is not set", line)
        if enabled or disabled:
            key = (enabled or disabled)[1]
            assert key not in options, f"{path}: duplicate option {key}"
            options[key] = enabled[2] if enabled else "n"
    return options


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_subparsers(dest="mode", required=True)
    snapshot = modes.add_parser("patch-snapshot")
    snapshot.add_argument("archive", type=Path)
    snapshot.add_argument("series", type=Path)
    snapshot.add_argument("patch", type=Path)
    trees = modes.add_parser("trees")
    trees.add_argument("baseline", type=Path)
    trees.add_argument("candidate", type=Path,
                       help="patched source tree or a patch-snapshot JSON file")
    configs = modes.add_parser("configs")
    configs.add_argument("x86", type=Path)
    configs.add_argument("arm", type=Path)
    args = parser.parse_args()
    if args.mode == "patch-snapshot":
        patch_snapshot(args.archive.resolve(), args.series.resolve(), args.patch.absolute())
    elif args.mode == "trees":
        expected = json.loads(args.baseline.read_text())
        actual = (tree_digests(args.candidate) if args.candidate.is_dir()
                  else json.loads(args.candidate.read_text()))
        count = compare(expected, actual, str(args.candidate))
        print(f"{count} matching file SHA-256 values")
    else:
        count = compare(config_options(args.x86), config_options(args.arm), "image config")
        print(f"{count} matching image config options")


if __name__ == "__main__":
    main()
