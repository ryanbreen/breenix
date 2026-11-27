"""Check that userland can build with the target sysroot alone."""

import json
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory
import tomllib
import unittest


WORKSPACE = Path(__file__).resolve().parents[1] / "userland"


def cargo_metadata(root, *flags):
    return json.loads(subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--offline", "--frozen",
         "--all-features", *flags, "--manifest-path", str(root / "Cargo.toml")],
        cwd=root,
    ))


def dependency_features(metadata):
    members = set(metadata["workspace_members"])
    by_directory = {
        str(Path(package["manifest_path"]).parent): package
        for package in metadata["packages"]
    }
    features = set()
    for member in metadata["packages"]:
        if member["id"] not in members:
            continue
        for dependency in member["dependencies"]:
            package = by_directory.get(dependency.get("path"))
            if package is not None:
                key = dependency["rename"] or dependency["name"]
                features.update(f"{key}/{feature}" for feature in package["features"])
    return sorted(features)


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
    metadata = cargo_metadata(root)
    # --all-features selects workspace features. Cargo also accepts explicit
    # dependency/feature arguments for direct path dependencies of members.
    features = dependency_features(metadata)
    if features:
        metadata = cargo_metadata(root, "--features", ",".join(features))
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

    def test_build_scripts_are_rejected(self):
        for script, build in (("build.rs", "false"), ("generate.rs", '"generate.rs"')):
            with self.subTest(script=script), TemporaryDirectory() as directory:
                root = Path(directory)
                (root / "Cargo.toml").write_text(
                    '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n'
                    f'build = {build}\n', encoding="utf-8"
                )
                (root / "src").mkdir()
                (root / "src/lib.rs").touch()
                (root / script).write_text('fn main() {}\n', encoding="utf-8")
                subprocess.run(["cargo", "generate-lockfile", "--offline", "--quiet"],
                               cwd=root, check=True)
                self.assertEqual(check_workspace(root), ["package app has a build script"])

    def test_registry_lock_entry_is_rejected_before_cargo_resolution(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.lock").write_text(
                'version = 4\n\n[[package]]\nname = "registry-dependency"\n'
                'version = "0.1.0"\nsource = "registry+https://example.invalid/index"\n',
                encoding="utf-8"
            )
            self.assertEqual(check_workspace(root), [
                "locked package registry-dependency is not a path package",
            ])

    def test_dependency_feature_cannot_enable_a_proc_macro(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.toml").write_text(
                '[workspace]\nresolver = "2"\nmembers = ["app"]\n'
                'exclude = ["helper", "macros"]\n', encoding="utf-8"
            )
            for name, extra in {
                "app": '[dependencies]\nrenamed = { package = "helper", path = "../helper" }\n',
                "helper": '[dependencies]\nmacros = { path = "../macros", optional = true }\n',
                "macros": '[lib]\nproc-macro = true\n',
            }.items():
                (root / name / "src").mkdir(parents=True)
                (root / name / "src/lib.rs").touch()
                (root / name / "Cargo.toml").write_text(
                    f'[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2021"\n'
                    f'{extra}', encoding="utf-8"
                )
            subprocess.run(["cargo", "generate-lockfile", "--offline", "--quiet"],
                           cwd=root, check=True)
            self.assertIn("package macros is a proc macro", check_workspace(root))


if __name__ == "__main__":
    root = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else WORKSPACE
    try:
        problems = check_workspace(root)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        sys.exit(f"userland guard: {error}")
    if problems:
        sys.exit("\n".join(problems))
    print("userland guard: path packages only, with no build scripts or proc macros")
