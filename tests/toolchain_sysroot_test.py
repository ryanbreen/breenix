"""Host checks for the sealed E25 reference build on both ELF targets."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
DRIVER = ROOT / "scripts/toolchain-ref-cc"
TARGETS = {"x86_64": 62, "aarch64": 183}


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


class toolchain_sysroot_test(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory(prefix="toolchain sysroot ")
        self.addCleanup(scratch.cleanup)
        self.root = Path(scratch.name).resolve()
        self.stage = self.root / "staged"
        self.stage.mkdir()
        self.output = self.root / "reference.elf"
        self.inventory = self.root / "inventory.json"
        self.source = self.stage / "reference.c"

    def build(self, arch, *args, env=None):
        return subprocess.run(
            [sys.executable, DRIVER, "--arch", arch, "--input-root", self.stage,
             "--clang", self.clang, "--clang-sha256", self.clang_pin,
             "--lld", self.lld, "--lld-sha256", self.lld_pin,
             "-o", self.output, "--inventory", self.inventory, *args, self.source],
            capture_output=True, text=True, timeout=15, env=env,
        )

    def test_unstaged_include_fails(self):
        outside = self.root / "untracked.h"
        outside.write_text("#define ANSWER 42\n")
        self.source.write_text(f'#include "{outside}"\nvoid _start(void) {{ (void)ANSWER; }}\n')
        for arch in TARGETS:
            with self.subTest(arch=arch):
                result = self.build(arch)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(str(outside), result.stderr)
                self.assertFalse(self.inventory.exists())
                self.assertFalse(self.output.exists())

    def test_reference_inventory(self):
        self.source.write_text(
            "#include <stdint.h>\n"
            "int answer(void);\n"
            "void _start(void) { const uint64_t value = (uint64_t)answer(); (void)value; }\n"
        )
        (self.root / "stdint.h").write_text('#error "ambient CPATH leaked"\n')
        env = dict(os.environ, CPATH=str(self.root))
        for arch, machine in TARGETS.items():
            with self.subTest(arch=arch):
                archive, member = self.archive(arch)
                result = self.build(arch, "--archive", archive, env=env)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stderr, "")
                elf = self.output.read_bytes()
                self.assertEqual(elf[:4], b"\x7fELF")
                self.assertEqual(int.from_bytes(elf[18:20], "little"), machine)
                record = json.loads(self.inventory.read_text())
                self.assertEqual(record["arch"], arch)
                self.assertEqual(record["elf"]["sha256"], digest(self.output))
                dependencies = [
                    self.source,
                    ROOT / "toolchain/include/stdint.h",
                    ROOT / "toolchain/include/limits.h",
                ]
                self.assertCountEqual(record["includes"], [
                    {"path": str(path), "sha256": digest(path)} for path in dependencies
                ])
                self.assertEqual(record["archives"], [{
                    "path": str(archive), "sha256": digest(archive),
                    "members": [{"name": member.name, "sha256": digest(member)}],
                }])
                script = ROOT / "userspace/programs/linker.ld"
                self.assertEqual(record["link_script"], {"path": str(script), "sha256": digest(script)})
                for name, path, pin in [("clang", self.clang, self.clang_pin),
                                        ("lld", self.lld, self.lld_pin)]:
                    producer = record["producer"][name]
                    self.assertEqual(producer["path"], str(path.resolve()))
                    self.assertEqual(producer["sha256"], pin)
                    self.assertTrue(producer["version"])
                self.assertIn("-nostdinc", record["flags"]["clang"][0])
                self.assertIn("-MD", record["flags"]["clang"][0])
                self.assertIn("--trace", record["flags"]["lld"])
                self.assertTrue(any(flag.startswith("-Map=") for flag in record["flags"]["lld"]))

    def test_external_archive_is_refused(self):
        self.source.write_text("void _start(void) {}\n")
        outside = self.root / "external.a"
        outside.write_bytes(b"!<arch>\n")
        alias = self.stage / "alias.a"
        alias.symlink_to(outside)
        result = self.build("x86_64", "--archive", alias)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(str(outside), result.stderr)
        self.assertFalse(self.inventory.exists())
        self.assertFalse(self.output.exists())

    def check_header_consumer(self, source):
        self.source.write_text(source)
        for arch in TARGETS:
            with self.subTest(arch=arch):
                result = self.build(arch)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stderr, "")

    def test_posix_limits(self):
        self.check_header_consumer("""
#define _POSIX_C_SOURCE 202405L
#include <stdint.h>
#include <limits.h>
_Static_assert(PATH_MAX >= _POSIX_PATH_MAX, "pathname limit");
_Static_assert(NAME_MAX >= _POSIX_NAME_MAX, "filename limit");
_Static_assert(SSIZE_MAX == INT64_MAX && CHAR_BIT == 8, "LP64 limits");
void _start(void) {}
""")

    def test_wchar_varargs(self):
        self.check_header_consumer("""
#include <wchar.h>
#ifdef va_start
#error Strict C wchar.h must not expose the stdarg macros
#endif
#include <stdarg.h>
void check(wchar_t *buffer, va_list args) {
    _Static_assert(sizeof(vswprintf(buffer, 1, L"%d", args)) == sizeof(int),
                   "vswprintf accepts the platform va_list");
}
void _start(void) {}
""")

    def test_fast_integer_formats(self):
        self.check_header_consumer("""
#include <inttypes.h>
#include <stdio.h>
int sscanf(const char *restrict, const char *restrict, ...)
    __attribute__((format(scanf, 2, 3)));
int printf(const char *restrict, ...) __attribute__((format(printf, 1, 2)));
void check(const char *input) {
    int_fast8_t i8;
    int_fast16_t i16;
    uint_fast8_t u8;
    uint_fast16_t u16;
    (void)sizeof(sscanf(input, "%" SCNdFAST8 " %" SCNdFAST16 " %" SCNuFAST8 " %" SCNuFAST16,
                        &i8, &i16, &u8, &u16));
    (void)sizeof(printf("%" PRIdFAST8 " %" PRIdFAST16 " %" PRIuFAST8 " %" PRIuFAST16,
                        i8, i16, u8, u16));
}
_Static_assert(INT_FAST8_MIN == -128 && INT_FAST8_MAX == 127 && UINT_FAST8_MAX == 255,
               "fast 8-bit limits");
_Static_assert(INT_FAST16_MIN == -32768 && INT_FAST16_MAX == 32767 && UINT_FAST16_MAX == 65535,
               "fast 16-bit limits");
void _start(void) {}
""")

    def archive(self, arch):
        objects = []
        for name, source in [("long_archive_member_name", "int answer(void) { return 42; }"),
                             ("unused", "int unused(void) { return 7; }")]:
            path = self.stage / f"{arch}-{name}.c"
            path.write_text(source + "\n")
            obj = path.with_suffix(".o")
            subprocess.run(
                [self.clang, "--no-default-config", f"--target={arch}-unknown-linux-gnu",
                 "-std=c11", "-Wall", "-Wextra", "-Werror", "-pedantic", "-nostdinc",
                 "-c", path, "-o", obj],
                check=True, timeout=10,
            )
            objects.append(obj)
        archive = self.stage / f"{arch}.a"
        subprocess.run([self.ar, "--format=gnu", "rcs", archive, *objects], check=True, timeout=10)
        return archive, objects[0]


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for tool in ["clang", "lld", "ar"]:
        parser.add_argument(f"--{tool}", type=Path, required=True)
    parser.add_argument("cases", nargs="*")
    options = parser.parse_args()
    for tool in ["clang", "lld", "ar"]:
        path = getattr(options, tool)
        if not path.is_absolute() or not path.is_file():
            parser.error(f"--{tool} requires an absolute executable path")
        setattr(toolchain_sysroot_test, tool, path)
    toolchain_sysroot_test.clang_pin = digest(options.clang)
    toolchain_sysroot_test.lld_pin = digest(options.lld)
    unittest.main(argv=[sys.argv[0], *options.cases], verbosity=2)
