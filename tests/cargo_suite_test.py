"""Replay the K7 guest commands with the pinned Mac twins."""
import hashlib
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
import os
from pathlib import Path
import re
import shutil
import signal
import struct
import subprocess
import tempfile
from threading import Thread
import time
import tomllib
import unittest
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[1]
K5 = ROOT / "toolchain/rust-host/k5"
K7 = ROOT / "toolchain/rust-host/k7"
CACHE = Path.home() / "Library/Caches/breenix-e25"


def read_json(path):
    return json.loads(path.read_text())


def digest(path):
    with path.open("rb") as artifact:
        return hashlib.file_digest(artifact, "sha256").hexdigest()


def project(step, fixture, workspace):
    def expand(value):
        return value.replace("{fixture}", str(fixture)).replace("{workspace}", str(workspace))

    return (
        [expand(arg) for arg in step["argv"]],
        Path(expand(step["cwd"])),
        {key: expand(value) for key, value in step["env"].items()},
    )


def run(step, deadline, executable=None):
    argv, cwd, env = step
    with subprocess.Popen(
        argv, cwd=cwd, env=env, executable=executable, start_new_session=True,
        stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    ) as child:
        try:
            stdout, stderr = child.communicate(timeout=max(0, deadline - time.monotonic()))
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            child.communicate()
            raise
        return subprocess.CompletedProcess(argv, child.returncode, stdout, stderr)


class RegistryRequest(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_error(404, "K7 registry control")

    def log_message(self, *_):
        pass


class RegistryListener(HTTPServer):
    connections = 0

    def get_request(self):
        connection = super().get_request()
        self.connections += 1
        return connection


class CargoSuite(unittest.TestCase):
    def test_suite_and_twin_replay(self):
        source_path = ROOT / "userspace/programs/src/suite_cargo.rs"
        self.assertTrue(source_path.is_file(), "missing K7 guest suite")
        source = source_path.read_text()
        manifest = read_json(ROOT / "docs/suites/cargo.json")
        suite_id, title, category, category_title = re.search(
            r'static SUITE: Suite = suite\(\s*"([^"]+)",\s*"([^"]+)",\s*'
            r'&\[category\(\s*"([^"]+)",\s*"([^"]+)"', source,
        ).groups()
        case_table = [
            {"id": case_id, "title": title}
            for case_id, title in re.findall(
                r'\bcase\(\s*"([^"]+)",\s*"([^"]+)"', source,
            )
        ]
        self.assertEqual((suite_id, title, manifest["binary"]),
                         (manifest["id"], manifest["title"], "/sbin/suite-cargo"))
        self.assertEqual(manifest["categories"], [
            {"id": category, "title": category_title, "cases": case_table},
        ])
        self.assertEqual(category, "cargo")
        self.assertEqual([case["id"] for case in case_table],
                         ["offline-path-workspace", "no-network"])
        limit = re.search(r'\.case_limit_ms\(([\d_]+)\)', source)
        self.assertIsNotNone(limit, "missing guest case deadline")
        limit_ms = int(limit[1])
        self.assertEqual(limit_ms, 60_000)
        invocations = json.loads(re.search(
            r'const INVOCATIONS: &str = r#"(.*?)"#;', source, re.S,
        )[1])
        self.assertEqual({case["id"] for case in case_table}, set(invocations))
        expect = read_json(K7 / "corpus/expect.json")
        self.assertEqual(set(expect), set(invocations))

        with tempfile.TemporaryDirectory(prefix="breenix-k7-") as directory:
            fixture = self.stage_twins(Path(directory))
            for name, check in (("offline-path-workspace", self.check_path_workspace),
                                ("no-network", self.check_no_network)):
                with self.subTest(case=name):
                    deadline = time.monotonic() + limit_ms / 1000
                    workspace = fixture / name
                    shutil.copytree(K7 / "corpus" / name, workspace)
                    steps = {key: project(step, fixture, workspace)
                             for key, step in invocations[name].items()}
                    self.assertEqual(steps["build"][0].count("--offline"), 1)
                    self.assertEqual(steps["build"][0].count("--frozen"), 1)
                    check(steps, expect[name], workspace, deadline)

    def stage_twins(self, fixture):
        k7 = read_json(K7 / "x86_64.json")
        default_rustc = CACHE / "k5/twin/mac/bin/rustc"
        default_sysroot = CACHE / "k5/twin/sysroot"
        cargo = os.environ.get("BREENIX_K7_MAC_CARGO")
        if cargo is None:
            g4 = Path((CACHE / "k7/g4.path").read_text().strip())
            target = k7["mac_target"]
            cargo = g4 / f"build/{target}/stage0-tools/{target}/release/cargo"
        cargo = Path(cargo).resolve()
        rustc = Path(os.environ.get("BREENIX_K5_MAC_RUSTC", default_rustc)).resolve()
        sysroot = Path(os.environ.get("BREENIX_K5_SYSROOT", default_sysroot)).resolve()
        bld = Path(os.environ.get("BREENIX_K7_BLD", ROOT / "toolchain/target/release/bld")).resolve()
        libc = Path(os.environ.get("BREENIX_K7_LIBC", ROOT / (
            "libs/libbreenix-libc/target/x86_64-breenix/release/libc.a"
        ))).resolve()

        self.assertEqual(digest(cargo), k7["mac_cargo_sha256"])
        self.assertEqual(digest(rustc), k7["k5_compiler_digests"]["mac_rustc_sha256"])
        self.assertTrue(sysroot.is_dir(), sysroot)
        self.assertTrue(bld.is_file(), bld)
        self.assertTrue(libc.is_file(), libc)
        (fixture / "bin").mkdir()
        (fixture / "lib").mkdir()
        (fixture / "targets").mkdir()
        for name, path in (("cargo", cargo), ("rustc", rustc), ("bld", bld)):
            (fixture / "bin" / name).symlink_to(path)
        (fixture / "sysroot").symlink_to(sysroot)
        (fixture / "lib/libc.a").symlink_to(libc)
        shutil.copyfile(ROOT / "userspace/programs/linker.ld", fixture / "lib/linker.ld")
        for target in ("x86_64-breenix", "x86_64-breenix-hosted"):
            shutil.copyfile(ROOT / f"{target}.json", fixture / f"targets/{target}.json")
        return fixture

    def check_result(self, result, expected):
        self.assertEqual(result.returncode, expected["status"], result.stderr)
        for stream in ("stdout", "stderr"):
            if stream in expected:
                self.assertEqual(getattr(result, stream), expected[stream].encode())

    def check_path_workspace(self, steps, expected, workspace, deadline):
        self.assertEqual(set(steps), {"build", "run"})
        build = steps["build"]
        self.check_result(run(build, deadline), expected["build"])
        binary = Path(steps["run"][0][0])
        with binary.open("rb") as elf:
            header = elf.read(20)
        self.assertEqual(header[:7], b"\x7fELF\x02\x01\x01")
        self.assertEqual(struct.unpack_from("<HH", header, 16), (2, 62))

        # macOS cannot exec the built x86-64 ELF. Build its native equivalent
        # with the same twins, then replay the run's argv, cwd and environment.
        argv, cwd, env = build
        native = read_json(K7 / "x86_64.json")["mac_target"]
        target = argv.index("--target") + 1
        native_argv = argv.copy()
        native_argv[target] = native
        native_env = {**env, "CARGO_ENCODED_RUSTFLAGS": "-Dwarnings"}
        self.check_result(run((native_argv, cwd, native_env), deadline), expected["build"])
        target_dir = cwd / argv[argv.index("--target-dir") + 1]
        native_binary = target_dir / native / binary.parent.name / binary.name
        self.check_result(run(steps["run"], deadline, executable=native_binary), expected["run"])

    def check_no_network(self, steps, expected, workspace, deadline):
        self.assertEqual(set(steps), {"build"})
        argv, cwd, env = steps["build"]
        config = workspace / ".cargo/config.toml"
        config_text = config.read_text()
        sources = tomllib.loads(config_text)["source"]
        registry = sources[sources["crates-io"]["replace-with"]]["registry"]
        url = urlsplit(registry.removeprefix("sparse+"))
        self.assertEqual((url.scheme, url.hostname, url.path), ("http", "127.0.0.1", "/"))
        with RegistryListener(("127.0.0.1", 0), RegistryRequest) as listener:
            # Project the fixture's inert loopback endpoint to the Mac listener.
            projected = f"sparse+http://127.0.0.1:{listener.server_port}/"
            config.write_text(config_text.replace(registry, projected))
            server = Thread(target=listener.serve_forever)
            server.start()
            try:
                self.check_result(run((argv, cwd, env), deadline), expected["build"])
                self.assertEqual(listener.connections, 0)
                registry_dir = Path(env["CARGO_HOME"]) / "registry"
                self.assertEqual(list(registry_dir.glob("*")), [])

                # Frozen implies offline as well. Preserve lock enforcement
                # in the control while allowing the registry request.
                online = [arg for arg in argv if arg != "--offline"]
                online[online.index("--frozen")] = "--locked"
                result = run((online, cwd, env), deadline)
                self.assertNotEqual(result.returncode, 0)
                self.assertGreaterEqual(listener.connections, 1, result.stderr)
            finally:
                listener.shutdown()
                server.join()


if __name__ == "__main__":
    unittest.main()
