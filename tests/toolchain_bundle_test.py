"""Check the producer record and receipts of a built toolchain bundle."""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import tomllib
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[1]
WORKSPACE = ROOT / "toolchain"
CHANNEL = tomllib.loads((WORKSPACE / "rust-toolchain.toml").read_text())["toolchain"]["channel"]


def digest(path):
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


class ToolchainGuestBuildTest(unittest.TestCase):
    def test_arm_guest_link_inputs_are_keyed_by_content(self):
        script = ROOT / "scripts/toolchain-build-tools.py"
        spec = importlib.util.spec_from_file_location("toolchain_build_tools", script)
        builder = importlib.util.module_from_spec(spec)
        with mock.patch.object(sys, "path", [str(script.parent), *sys.path]):
            spec.loader.exec_module(builder)

        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            fork = root / "rust-fork"
            fork.mkdir()
            (fork / ".rust-fork-snapshot.json").write_text('{"commit": "test-pin"}')
            (root / "rust-fork.toml").write_text('commit = "test-pin"\n')
            linker = root / "userspace/programs/linker.ld"
            linker.parent.mkdir(parents=True)
            binaries = root / "binaries"
            binaries.mkdir()
            for name in (*builder.TOOLS, "bcc", "bmake"):
                (binaries / name).write_bytes(name.encode())
            for name in ("interop", "brt"):
                archive = root / f"target/toolchain-bundle/{name}/aarch64-breenix/release/lib{name}.a"
                archive.parent.mkdir(parents=True)
                archive.write_bytes(name.encode())

            with (mock.patch.object(builder, "ROOT", root),
                  mock.patch.object(builder.subprocess, "run"),
                  mock.patch.object(builder, "cargo_tools", return_value=binaries) as cargo):
                def build(name, archive, script):
                    dev = root / name
                    (dev / "lib").mkdir(parents=True)
                    (dev / "lib/libc.a").write_bytes(archive)
                    linker.write_bytes(script)
                    builder.build_guest_tools("aarch64", root / "work", dev, [], [], {}, {})
                    _, env, _, target_dir, *_ = cargo.call_args.args
                    flags = env["CARGO_ENCODED_RUSTFLAGS"].split("\x1f")
                    library = next(flag.removeprefix("native=") for flag in flags if flag.startswith("native="))
                    script_path = next(flag.removeprefix("link-arg=-T") for flag in flags
                                       if flag.startswith("link-arg=-T"))
                    self.assertEqual((Path(library) / "libc.a").read_bytes(), archive)
                    self.assertEqual(Path(script_path).read_bytes(), script)
                    return target_dir, flags, Path(library) / "libc.a", Path(script_path)

                first = build("first", b"guest libc", b"guest script")
                self.assertEqual(build("same", b"guest libc", b"guest script"), first)
                self.assertNotEqual(build("changed-libc", b"updated libc", b"guest script")[0], first[0])
                self.assertNotEqual(build("changed-script", b"guest libc", b"updated script")[0], first[0])
                self.assertEqual(first[2].read_bytes(), b"guest libc")
                self.assertEqual(first[3].read_bytes(), b"guest script")


class ToolchainBundleTest(unittest.TestCase):
    def setUp(self):
        self.dev = self.bundle / "root/opt/breenix-dev/25.2"
        self.record = json.loads((self.bundle / "build.json").read_text())
        self.rustc = Path(subprocess.check_output(
            ["rustup", "which", "--toolchain", CHANNEL, "rustc"], text=True,
        ).strip())

    def tools_built_by_rustc(self):
        tools = self.record.get("tools")
        self.assertIsInstance(tools, dict, "bundle build record has no Cargo tool provenance")
        self.assertEqual(set(tools), {"bas", "bld", "bar", "bnm"} |
                         {"bcc", "bmake"})
        self.assertEqual(self.record, json.loads((self.dev / "build.json").read_text()))
        arch = self.record["arch"]
        machine = {"x86_64": 62, "aarch64": 183}[arch]  # ELF gABI, e_machine.
        fork = tomllib.loads((ROOT / "rust-fork.toml").read_text())["commit"]
        rustc = {
            "version": subprocess.check_output([self.rustc, "--version"], text=True).strip(),
            "sha256": digest(self.rustc),
        }
        for name, entry in tools.items():
            with self.subTest(tool=name):
                program = self.dev / "bin" / {"bcc": "cc", "bmake": "make"}.get(name, name)
                expected = {
                    "cargoPackage": name,
                    "rustForkCommit": fork,
                    "rustc": rustc,
                    "target": f"{arch}-breenix",
                    "sha256": digest(program),
                }
                if name in ("bcc", "bmake"):
                    expected["path"] = f"bin/{program.name}"
                self.assertEqual(entry, expected)
                with program.open("rb") as file:
                    header = file.read(20)
                self.assertEqual(header[:6], b"\x7fELF\x02\x01")
                self.assertEqual(struct.unpack_from("<HH", header, 16), (2, machine))

    def cc_built_by_rustc(self):
        entry = self.record["tools"]["bcc"]
        executable = self.dev / "bin/cc"
        fork = tomllib.loads((ROOT / "rust-fork.toml").read_text())["commit"]
        self.assertEqual(entry, {
            "cargoPackage": "bcc",
            "path": "bin/cc",
            "rustForkCommit": fork,
            "rustc": {"version": subprocess.check_output([self.rustc, "--version"], text=True).strip(),
                      "sha256": digest(self.rustc)},
            "target": f"{self.record['arch']}-breenix",
            "sha256": digest(executable),
        })
        self.assertFalse(any(path.suffix == ".c" for path in (WORKSPACE / "bcc").rglob("*")))

    def make_built_by_rustc(self):
        entry = self.record["tools"]["bmake"]
        executable = self.dev / "bin/make"
        self.assertEqual(entry["path"], "bin/make")
        self.assertEqual(entry["sha256"], digest(executable))
        manifest = (self.dev / "manifest.sha256").read_text()
        self.assertIn(f"{digest(executable)}  opt/breenix-dev/25.2/bin/make\n", manifest)
        for category in ("make", "project"):
            for source in (WORKSPACE / "corpus" / category).iterdir():
                if category == "project" and source.name == "value.s" and self.record["arch"] == "aarch64":
                    source = WORKSPACE / "corpus/project-aarch64/value.s"
                with self.subTest(fixture=source.name):
                    path = f"usr/share/breenix/toolchain/{category}/{source.name}"
                    self.assertEqual(digest(self.bundle / "root" / path), digest(source))
                    self.assertIn(f"{digest(source)}  {path}\n", manifest)
        self.assertFalse(any(path.suffix == ".c" for path in (WORKSPACE / "bmake").rglob("*")))

        with tempfile.TemporaryDirectory(prefix="toolchain-stale-make-") as temp:
            bundle = Path(temp)
            tree = bundle / "root"
            shutil.copytree(self.bundle / "root", tree)
            for generated in ("manifest.sha256", "build.json"):
                (tree / "opt/breenix-dev/25.2" / generated).unlink()
            stale = "usr/share/breenix/toolchain/make/sample.src"
            (tree / stale).write_text("stale fixture\n")
            tool_record = bundle / "tools.json"
            tool_record.write_text(json.dumps(self.record["tools"]))
            sealed = subprocess.run([
                sys.executable, ROOT / "scripts/toolchain-image.py", "seal", bundle,
                self.record["commit"], self.record["arch"], "--tool-record", tool_record,
            ], capture_output=True, text=True, timeout=10)
            self.assertEqual(sealed.returncode, 1, sealed.stderr)
            self.assertIn(f"staged bytes differ from the source for {stale}", sealed.stderr)

    def arm_bundle_has_cc_and_make(self):
        self.assertEqual(set(self.record["tools"]), {"bas", "bld", "bar", "bnm", "bcc", "bmake"})
        self.assertEqual({path.name for path in (self.dev / "bin").iterdir()},
                         {"bas", "bld", "bar", "bnm", "cc", "make", "as", "ld"})
        for tool, alias in (("bas", "as"), ("bld", "ld")):
            self.assertEqual(digest(self.dev / "bin" / tool), digest(self.dev / "bin" / alias))
        self.assertEqual(self.record["candidateProducer"], "mac-bcc" if sys.platform == "darwin" else "host-bcc")

    def arm_bundle_stages_language_abi(self):
        self.assertEqual(self.record["arch"], "aarch64")
        root = self.bundle / "root"
        corpus = Path("usr/share/breenix/toolchain")
        manifest = set((self.dev / "manifest.sha256").read_text().splitlines())
        for category in ("abi", "tls", "interop"):
            source = WORKSPACE / "corpus" / ("abi-aarch64" if category == "abi" else category)
            for path in source.iterdir():
                if not path.is_file() or path.suffix not in (".c", ".h", ".expect"):
                    continue
                relative = corpus / ("abi-aarch64" if category == "abi" else category) / path.name
                staged = root / relative
                self.assertEqual(staged.read_bytes(), path.read_bytes())
                self.assertIn(f"{digest(staged)}  {relative}", manifest)
                if category == "abi" and path.suffix == ".c":
                    obj = relative.with_suffix(".o")
                    header = (root / obj).read_bytes()[:20]
                    self.assertEqual(header[:6], b"\x7fELF\x02\x01")
                    self.assertEqual(struct.unpack_from("<HH", header, 16), (1, 183))
                    self.assertIn(f"{digest(root / obj)}  {obj}", manifest)
        for name in ("interop", "brt"):
            path = f"lib/lib{name}.a"
            archive = self.dev / path
            self.assertEqual(archive.read_bytes()[:8], b"!<arch>\n")
            self.assertIn(f"{digest(archive)}  opt/breenix-dev/25.2/{path}", manifest)
            self.assertEqual(self.record[name], {
                "cargoPackage": name, "path": path, "target": "aarch64-breenix",
                "rustForkCommit": tomllib.loads((ROOT / "rust-fork.toml").read_text())["commit"],
                "rustc": self.record["tools"]["bcc"]["rustc"], "sha256": digest(archive),
            })
        names = (WORKSPACE / "corpus/cases").read_text().splitlines()
        expected = set(names)
        self.assertEqual(len(expected), 30)
        candidates = root / corpus / "c-core/mac"
        self.assertEqual({p.name for p in candidates.iterdir()}, {f"{n}.elf" for n in expected})
        for path in candidates.iterdir():
            self.assertIn(f"{digest(path)}  {corpus / 'c-core/mac' / path.name}", manifest)

    def bootstrap_abi_tls_inputs_staged(self):
        root = self.bundle / "root"
        corpus = Path("usr/share/breenix/toolchain")
        source = WORKSPACE / "corpus"
        manifest = set((self.dev / "manifest.sha256").read_text().splitlines())

        def staged(relative):
            path = root / relative
            self.assertTrue(path.is_file(), f"fixture is missing {relative}")
            self.assertIn(f"{digest(path)}  {relative}", manifest)
            return path

        abi = ("aggregates-exhaustion", "varargs", "sse-float", "x87-long-double")
        tls = ("template-init-zero-fill-aligned", "errno-per-thread", "tls-across-fork-exec")
        interop = ("c-calls-rust", "rust-calls-c")
        for category, names in (("abi", abi), ("tls", tls), ("interop", interop)):
            for name in names:
                with self.subTest(category=category, program=name):
                    stems = (f"{name}-caller", f"{name}-callee") if category == "abi" else (name,)
                    for stem in stems:
                        relative = Path(category) / f"{stem}.c"
                        code = staged(corpus / relative).read_bytes()
                        self.assertEqual(code, (source / relative).read_bytes())
                        if category == "abi":
                            header = staged(corpus / category / f"{stem}.o").read_bytes()[:20]
                            self.assertEqual(header[:6], b"\x7fELF\x02\x01")
                            self.assertEqual(struct.unpack_from("<HH", header, 16), (1, 62))
                        if category == "tls":
                            self.assertNotIn(b"_Alignas", code)
                    relative = Path(category) / f"{name}.expect"
                    expectation = staged(corpus / relative).read_bytes()
                    self.assertEqual(expectation, (source / relative).read_bytes())
                    status, newline, _ = expectation.partition(b"\n")
                    self.assertTrue(newline and status.isdigit() and 0 <= int(status) <= 255)

        archive_path = "lib/libinterop.a"
        archive = staged(Path("opt/breenix-dev/25.2") / archive_path)
        self.assertEqual(archive.read_bytes()[:8], b"!<arch>\n")
        self.assertEqual(self.record.get("interop"), {
            "cargoPackage": "interop",
            "path": archive_path,
            "rustForkCommit": tomllib.loads((ROOT / "rust-fork.toml").read_text())["commit"],
            "rustc": {"version": subprocess.check_output([self.rustc, "--version"], text=True).strip(),
                      "sha256": digest(self.rustc)},
            "target": "x86_64-breenix-c",
            "sha256": digest(archive),
        })
        self.assertEqual(self.record, json.loads((self.dev / "build.json").read_text()))

    def x86_candidates_cover_full_corpus(self):
        root = self.bundle / "root"
        expected = (WORKSPACE / "corpus/cases").read_text().splitlines()
        self.assertEqual(len(expected), 30)
        mac = Path("usr/share/breenix/toolchain/c-core/mac")
        candidates = {f"{name}.elf" for name in expected}
        self.assertEqual({path.name for path in (root / mac).iterdir()}, candidates)
        case_list = ROOT / "userspace/programs/src/fixture/toolchain-cc-cases"
        self.assertEqual(case_list.read_text().splitlines(), expected)
        manifest = set((self.dev / "manifest.sha256").read_text().splitlines())
        for name in sorted(candidates):
            relative = mac / name
            self.assertIn(f"{digest(root / relative)}  {relative}", manifest)

    def c11_inputs_staged(self):
        root = self.bundle / "root"
        corpus = Path("usr/share/breenix/toolchain")
        c11 = WORKSPACE / "corpus/c11"
        constraints = json.loads((c11 / "constraints.json").read_text())
        programs = ("declarations-initializers", "generic-static-assert-alignment",
                    "floating", "atomics", "profile-macros")
        inputs = {"constraints.json", "constraint-diagnostics.expect",
                  *constraints["guest"].values(),
                  *(f"{name}.{suffix}" for name in programs for suffix in ("c", "expect"))}
        manifest = set((self.dev / "manifest.sha256").read_text().splitlines())
        staged = root / corpus / "c11"
        self.assertEqual({path.relative_to(staged).as_posix() for path in staged.rglob("*")
                          if path.is_file()}, inputs)
        for path in sorted(inputs):
            with self.subTest(path=path):
                source = (WORKSPACE / "corpus/c11-aarch64/floating.c"
                          if self.record["arch"] == "aarch64" and path == "floating.c" else c11 / path)
                self.assertEqual((staged / path).read_bytes(), source.read_bytes())
                self.assertIn(f"{digest(source)}  {corpus / 'c11' / path}", manifest)
        for path in (corpus / "tls/threads.h", Path("opt/breenix-dev/25.2/lib/libbrt.a")):
            self.assertIn(f"{digest(root / path)}  {path}", manifest)

    def c17_libc_inputs_staged(self):
        root = self.bundle / "root"
        corpus = Path("usr/share/breenix/toolchain")
        sources = {corpus / "c17" / path.name: path for path in (WORKSPACE / "corpus/c17").iterdir() if path.is_file()}
        sources[corpus / "libc-suite.mk"] = WORKSPACE / "corpus/libc-suite.mk"
        sources.update({corpus / "libc-suite" / path.name: path for path in (ROOT / "userspace/libc-suite").iterdir()
                        if path.suffix in (".c", ".h") or path.name == "categories"})
        manifest = set((self.dev / "manifest.sha256").read_text().splitlines())
        for path, source in sources.items():
            with self.subTest(path=path):
                self.assertEqual((root / path).read_bytes(), source.read_bytes())
                self.assertIn(f"{digest(source)}  {path}", manifest)

    def arm_bundle_stages_c17_libc(self):
        self.assertEqual(self.record["arch"], "aarch64")
        root = self.bundle / "root"
        corpus = Path("usr/share/breenix/toolchain")
        manifest = set((self.dev / "manifest.sha256").read_text().splitlines())
        sources = {corpus / "c17" / p.name: p for p in (WORKSPACE / "corpus/c17").iterdir() if p.is_file()}
        sources[corpus / "libc-suite.mk"] = WORKSPACE / "corpus/libc-suite.mk"
        sources[corpus / "libc-suite/libc.json"] = ROOT / "docs/suites/libc.json"
        sources.update({corpus / "libc-suite" / p.name: p for p in (ROOT / "userspace/libc-suite").iterdir()
                        if p.suffix in (".c", ".h") or p.name == "categories"})
        for relative, source in sources.items():
            with self.subTest(path=relative):
                self.assertEqual((root / relative).read_bytes(), source.read_bytes())
                self.assertIn(f"{digest(source)}  {relative}", manifest)

        spec = importlib.util.spec_from_file_location("libc_symbol_audit", ROOT / "scripts/libc-symbol-audit.py")
        audit = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(audit)
        clang = Path(os.environ["BREENIX_TOOLCHAIN_CLANG"])
        compiler = [str(clang), "--no-default-config", "--target=aarch64-unknown-linux-gnu",
                    "-Ulinux", "-U__linux", "-U__linux__", "-U__gnu_linux__", "-D__breenix__=1",
                    "-nostdinc", "-isystem", str(audit.INCLUDE), "-isystem", str(WORKSPACE / "include")]
        functions = audit.declared_functions("aarch64", audit.public_headers(), compiler=compiler)
        inventory = corpus / "libc-functions.txt"
        self.assertEqual((root / inventory).read_text().splitlines(), sorted(functions))
        self.assertTrue(functions)
        self.assertIn(f"{digest(root / inventory)}  {inventory}", manifest)

        for name in ("libc.a", "libbrt.a"):
            relative = Path("opt/breenix-dev/25.2/lib") / name
            data = (root / relative).read_bytes()
            self.assertIn(f"{digest(root / relative)}  {relative}", manifest)
            self.assertEqual(data[:8], b"!<arch>\n")
            offset, objects = 8, 0
            while offset < len(data):
                header = data[offset:offset + 60]
                self.assertEqual(header[58:60], b"`\n")
                size = int(header[48:58])
                member = data[offset + 60:offset + 60 + size]
                if header[:3] == b"#1/":
                    member = member[int(header[:16].decode().strip()[3:]):]
                if member.startswith(b"\x7fELF"):
                    self.assertEqual(member[:6], b"\x7fELF\x02\x01")
                    self.assertEqual(struct.unpack_from("<H", member, 18)[0], 183)
                    objects += 1
                offset += 60 + size + (size & 1)
            self.assertEqual(offset, len(data))
            self.assertGreater(objects, 0)

        categories = [line.split()[0] for line in (ROOT / "userspace/libc-suite/categories").read_text().splitlines()
                      if line.strip() and not line.lstrip().startswith("#")]
        self.assertEqual(len(categories), 7)
        for category in categories:
            name = f"libc-{category}_test.elf"
            relative = Path("usr/libexec/breenix") / name
            data = (root / relative).read_bytes()
            self.assertEqual(data, (ROOT / "userspace/programs/aarch64" / name).read_bytes())
            self.assertEqual(data[:6], b"\x7fELF\x02\x01")
            self.assertEqual(struct.unpack_from("<HH", data, 16), (2, 183))
            self.assertIn(f"{digest(root / relative)}  {relative}", manifest)

    def brt_built_by_rustc(self):
        path = "lib/libbrt.a"
        archive = self.dev / path
        self.assertTrue(archive.is_file(), f"fixture is missing {path}")
        self.assertEqual(archive.read_bytes()[:8], b"!<arch>\n")
        fork = tomllib.loads((ROOT / "rust-fork.toml").read_text())["commit"]
        self.assertTrue(fork.startswith("666252fd"))
        self.assertEqual(self.record.get("brt"), {
            "cargoPackage": "brt",
            "path": path,
            "rustForkCommit": fork,
            "rustc": {"version": subprocess.check_output([self.rustc, "--version"], text=True).strip(),
                      "sha256": digest(self.rustc)},
            "target": "x86_64-breenix",
            "sha256": digest(archive),
        })
        manifest = (self.dev / "manifest.sha256").read_text().splitlines()
        self.assertIn(f"{digest(archive)}  opt/breenix-dev/25.2/{path}", manifest)

    def bcc_links_aggregate_atomic(self):
        version = subprocess.check_output([self.rustc, "-vV"], text=True)
        host = next(line.removeprefix("host: ") for line in version.splitlines() if line.startswith("host: "))
        host_bins = ROOT / "target/toolchain-bundle/host" / host / "release"
        self.assertEqual(digest(self.dev / "bin/ld"), self.record["tools"]["bld"]["sha256"])
        with tempfile.TemporaryDirectory(prefix="toolchain-aggregate-atomic-") as temporary:
            work = Path(temporary)
            dev = work / "dev"
            shutil.copytree(self.dev, dev)
            # Run the same Cargo packages as the guest aliases, using the staged sysroot.
            for package, alias in (("bcc", "cc"), ("bas", "as"), ("bld", "ld")):
                shutil.copy2(host_bins / package, dev / "bin" / alias)
            source = work / "atomic.c"
            source.write_text("""
#include <stdatomic.h>
struct counters { long count, sum, inverse; };
static _Atomic(struct counters) totals;
int main(void) {
    struct counters snapshot = atomic_load(&totals);
    return (int)snapshot.count;
}
""")
            cc, ld = dev / "bin/cc", dev / "bin/ld"
            obj, binary = work / "atomic.o", work / "atomic"
            subprocess.run([cc, "-c", source, "-o", obj], check=True, timeout=10)
            link = [ld, "-T", dev / "linker.ld", "--gc-sections", "-o", work / "direct", obj]
            libc, brt = dev / "lib/libc.a", dev / "lib/libbrt.a"
            control = subprocess.run([*link, libc], capture_output=True, text=True, timeout=10)
            self.assertNotEqual(control.returncode, 0)
            self.assertIn("__atomic_load", control.stderr)
            result = subprocess.run([cc, source, "-o", binary], capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            subprocess.run([*link, brt, libc], check=True, timeout=10)
            self.assertEqual(binary.read_bytes(), (work / "direct").read_bytes())

    def arm_encoding_inputs_are_staged(self):
        relative = Path("usr/share/breenix/toolchain/assembler")
        manifest = (self.dev / "manifest.sha256").read_text().splitlines()
        for name in ("encodings.s", "encodings.hex"):
            with self.subTest(file=name):
                source = WORKSPACE / "corpus/assembler-aarch64" / name
                staged = self.bundle / "root" / relative / name
                self.assertTrue(staged.is_file(), f"ARM bundle is missing {relative / name}")
                self.assertEqual(staged.read_bytes(), source.read_bytes())
                self.assertIn(f"{digest(source)}  {relative / name}", manifest)

    def aarch64_stages_linker_objects(self):
        self.assertEqual(self.record["arch"], "aarch64")
        corpus = self.bundle / "root/usr/share/breenix/toolchain"
        manifest = (self.dev / "manifest.sha256").read_text().splitlines()
        for name in ("got-main", "got-entry", "archive-value-with-long-name", "tls-template"):
            path = corpus / "objs" / (name + ".o")
            self.assertTrue(path.is_file(), str(path))
            self.assertEqual(struct.unpack_from("<H", path.read_bytes(), 18)[0], 183)
            self.assertIn(f"{digest(path)}  usr/share/breenix/toolchain/objs/{name}.o", manifest)
        for name in ("got-main", "tls-template"):
            path = corpus / "linker" / (name + ".expect")
            source = WORKSPACE / "corpus/linker" / path.name
            self.assertEqual(path.read_bytes(), source.read_bytes())
            self.assertIn(f"{digest(path)}  usr/share/breenix/toolchain/linker/{path.name}", manifest)

    def aarch64_host_bld_links_staged_objects(self):
        self.assertEqual(self.record["arch"], "aarch64")
        corpus = self.bundle / "root/usr/share/breenix/toolchain"
        names = (WORKSPACE / "corpus/cases").read_text().splitlines()
        self.assertEqual(len(names), 30)
        with tempfile.TemporaryDirectory(prefix="toolchain-arm-host-link-") as temp:
            env = dict(os.environ, RUSTC=str(self.rustc))
            subprocess.run(["cargo", f"+{CHANNEL}", "build", "--release", "--locked",
                            "--manifest-path", WORKSPACE / "Cargo.toml", "--target-dir", temp,
                            "-p", "bld", "-p", "elfcheck"], env=env, check=True)
            bld, elfcheck = (Path(temp) / "release" / name for name in ("bld", "elfcheck"))
            obj = corpus / "objs"
            groups = [(name, [obj / (name + ".o")]) for name in names]
            groups += [("got-main", [obj / (name + ".o") for name in
                                      ("got-main", "got-entry", "archive-value-with-long-name")]),
                       ("tls-template", [obj / "tls-template.o"])]
            for name, inputs in groups:
                with self.subTest(program=name):
                    output = Path(temp) / (name + ".elf")
                    subprocess.run([bld, "-T", self.dev / "linker.ld", "--gc-sections", "-o", output,
                                    *inputs, self.dev / "lib/libc.a"], check=True, timeout=60)
                    subprocess.run([elfcheck, "--machine", "aarch64", "--exec", output],
                                   check=True, timeout=10)

    def receipts_from_cargo_host_bas(self):
        assembler = self.bundle / "root/usr/share/breenix/toolchain/assembler"
        names = (("sse-exit", "x87-exit") if self.record["arch"] == "x86_64"
                 else ("fp-exit", "call-exit"))
        receipts = {name: json.loads((assembler / f"{name}.link.json").read_text()) for name in names}
        for name, receipt in receipts.items():
            self.assertIn("hostBas", receipt, f"{name} has no Cargo host bas provenance")

        # Rebuild the staged bas package for the host so its assembler can run
        # here. The guest object-runs case compares the same SHA-256 values
        # after assembling these sources with the staged guest ELF.
        version = subprocess.check_output([self.rustc, "-vV"], text=True)
        host = next(line.removeprefix("host: ") for line in version.splitlines() if line.startswith("host: "))
        with tempfile.TemporaryDirectory(prefix="toolchain-host-bas-") as temp:
            env = dict(os.environ, RUSTC=str(self.rustc))
            for key in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"):
                env.pop(key, None)
            flags = []
            if env.get("BREENIX_USERSPACE_REMAP") == "1":
                cargo_home = Path(env.get("CARGO_HOME", Path.home() / ".cargo"))
                flags = [f"--remap-path-prefix={ROOT}=/breenix", f"--remap-path-prefix={cargo_home}=/cargo"]
            env["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(flags)
            subprocess.run([
                "cargo", f"+{CHANNEL}", "build", "--release", "--locked", "-p", "bas",
                "--target", host, "--target-dir", temp, "--manifest-path", WORKSPACE / "Cargo.toml",
            ], cwd=WORKSPACE, env=env, check=True, timeout=60)
            bas = Path(temp) / host / "release/bas"
            producer = {"cargoPackage": "bas", "target": host, "sha256": digest(bas)}
            for name, receipt in receipts.items():
                with self.subTest(program=name):
                    self.assertEqual(receipt["hostBas"], producer)
                    obj = Path(temp) / f"{name}.o"
                    subprocess.run([bas, *(["--target", "aarch64"] if self.record["arch"] == "aarch64" else []),
                                    "-o", obj, assembler / f"{name}.s"], check=True, timeout=10)
                    self.assertEqual(receipt["inputObjects"], {obj.name: digest(obj)})
                    elf = assembler / f"{name}.elf"
                    self.assertEqual(receipt["elf"], {"name": elf.name, "sha256": digest(elf)})

    def x86_artifacts_equal_baseline(self):
        self.assertEqual(self.record["arch"], "x86_64")
        self.assertIsNotNone(self.baseline, "pass an origin/main x86 bundle with --baseline")
        self.assertEqual(json.loads((self.baseline / "build.json").read_text())["arch"], "x86_64")
        relative = "root/usr/share/breenix/toolchain/assembler"
        actual, expected = self.bundle / relative, self.baseline / relative
        self.assertEqual({p.name for p in actual.iterdir()}, {p.name for p in expected.iterdir()})
        for path in actual.iterdir():
            with self.subTest(artifact=path.name):
                if path.name.endswith(".link.json"):
                    receipt = json.loads(path.read_text())
                    baseline = json.loads((expected / path.name).read_text())
                    for field in ("inputObjects", "elf", "map"):
                        self.assertEqual(receipt[field], baseline[field])
                else:
                    self.assertEqual(path.read_bytes(), (expected / path.name).read_bytes())

    def read_fault_probe_is_not_shipped(self):
        self.assertIsNotNone(self.probe, "pass the guest-target bld-read-probe with --probe")
        shipped = self.dev / "bin/bld"
        probe = self.probe.read_bytes()
        with shipped.open("rb") as file:
            header = file.read(20)
        self.assertEqual(probe[:20], header, "the probe must be for the same ELF target")
        self.assertIn(b"read EINTR", probe)
        self.assertIn(b"read short", probe)
        self.assertNotEqual(digest(shipped), digest(self.probe))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("bundle", type=Path)
    parser.add_argument("--probe", type=Path)
    parser.add_argument("--baseline", type=Path)
    args, names = parser.parse_known_args()
    ToolchainBundleTest.bundle = args.bundle
    ToolchainBundleTest.probe = args.probe
    ToolchainBundleTest.baseline = args.baseline
    checks = ["ToolchainBundleTest.tools_built_by_rustc",
              "ToolchainBundleTest.receipts_from_cargo_host_bas",
              "ToolchainBundleTest.c11_inputs_staged",
              "ToolchainBundleTest.c17_libc_inputs_staged"]
    checks.extend(["ToolchainBundleTest.cc_built_by_rustc",
                   "ToolchainBundleTest.make_built_by_rustc"])
    if json.loads((args.bundle / "build.json").read_text())["arch"] == "x86_64":
        checks.extend(["ToolchainBundleTest.bootstrap_abi_tls_inputs_staged",
                       "ToolchainBundleTest.x86_candidates_cover_full_corpus",
                       "ToolchainBundleTest.brt_built_by_rustc",
                       "ToolchainBundleTest.bcc_links_aggregate_atomic"])
    else:
        checks.extend(["ToolchainBundleTest.arm_bundle_stages_c17_libc",
                       "ToolchainBundleTest.arm_bundle_has_cc_and_make",
                       "ToolchainBundleTest.arm_bundle_stages_language_abi",
                       "ToolchainBundleTest.arm_encoding_inputs_are_staged",
                       "ToolchainBundleTest.aarch64_stages_linker_objects",
                       "ToolchainBundleTest.aarch64_host_bld_links_staged_objects"])
    if args.baseline is not None:
        checks.append("ToolchainBundleTest.x86_artifacts_equal_baseline")
    if args.probe is not None:
        checks.append("ToolchainBundleTest.read_fault_probe_is_not_shipped")
    unittest.main(argv=[sys.argv[0], *names], defaultTest=checks)
