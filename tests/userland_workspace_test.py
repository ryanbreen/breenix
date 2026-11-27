"""Check that userland can build with the target sysroot alone."""

import json
from pathlib import Path
import subprocess
import sys
import tomllib
import unittest


WORKSPACE = Path(__file__).resolve().parents[1] / "userland"


def check_workspace(root):
    lock = root / "Cargo.lock"
    with lock.open("rb") as file:
        locked = tomllib.load(file).get("package", [])
    problems = [
        f"locked package {package['name']} is not a path package"
        for package in locked
        if "source" in package or "checksum" in package
    ]
    if problems:
        return problems

    # Resolve the full graph, including path dependencies outside the workspace
    # and target-specific dependencies. Cargo identifies renamed proc macros
    # and custom build scripts by target kind.
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--offline", "--frozen",
         "--all-features", "--manifest-path", str(root / "Cargo.toml")],
        cwd=root,
    ))
    if not metadata["workspace_members"]:
        problems.append("userland workspace has no members")
    for package in metadata["packages"]:
        targets = {kind for target in package["targets"] for kind in target["kind"]}
        directory = Path(package["manifest_path"]).parent
        if (directory / "build.rs").exists() or "custom-build" in targets:
            problems.append(f"package {package['name']} has a build script")
        if "proc-macro" in targets:
            problems.append(f"package {package['name']} is a proc macro")
    return problems


class UserlandWorkspaceTest(unittest.TestCase):
    def test_workspace_builds_without_host_packages(self):
        problems = check_workspace(WORKSPACE)
        self.assertFalse(problems, "\n".join(problems))


if __name__ == "__main__":
    root = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else WORKSPACE
    try:
        problems = check_workspace(root)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        sys.exit(f"userland guard: {error}")
    if problems:
        sys.exit("\n".join(problems))
    print("userland guard: path packages only, with no build scripts or proc macros")
