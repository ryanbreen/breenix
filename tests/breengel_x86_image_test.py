#!/usr/bin/env python3
"""Exercise BreenGL's image population and layout rules without mounting a disk.

Both architectures stage data at /usr/local/test/share/breengel; install-layout
moves it to /usr/libexec/breenix/share/breengel, the shared suite's runtime path.
"""
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
PROGRAMS = ("suite-breengel", "kiosk_test", "raw-window_test")


class BreengelImageTests(unittest.TestCase):
    def test_x86_and_arm_build_lists_include_breengel_producers(self):
        source = (ROOT / "userspace/programs/build.sh").read_text()
        start = source.index("STD_BINARIES=(")
        end = source.index("# A retained target", start)
        for arch in ("x86_64", "aarch64"):
            with self.subTest(arch=arch), tempfile.TemporaryDirectory() as temporary:
                result = subprocess.run(
                    ["/bin/bash", "-e", "-c", source[start:end] +
                     '\nprintf "%s\\n" "${STD_BINARIES[@]}"'],
                    env=dict(os.environ, ARCH=arch, TESTS_DIR=temporary),
                    capture_output=True, text=True, check=True)
                names = [entry.rsplit(":", 1)[-1] for entry in result.stdout.splitlines()]
                for program in PROGRAMS:
                    self.assertIn(program, names)

    def test_both_image_paths_install_the_same_breengel_payload_on_x86_and_arm(self):
        source = (ROOT / "scripts/create_ext2_disk.sh").read_text()
        self.assertIn('BREENGEL_DATA="$PROJECT_ROOT/testdata/breengel"', source)
        self.assertIn('BREENGEL_FRAMES="$PROJECT_ROOT/docs/suites/frames/breengel"', source)
        blocks = source.split("# Install TrueType fonts")[1:]
        self.assertEqual(len(blocks), 2, "Docker and native population paths")
        for index, block in enumerate(blocks):
            population, rest = block.split("# Create default system font configuration", 1)
            layout_call = ('sh /install-layout.sh /mnt/ext2 /binaries /layout.tsv' if index == 0 else
                           'sh "$SCRIPT_DIR/install-layout.sh" "$MOUNT_DIR" "$USERSPACE_DIR" '
                           '"$PROJECT_ROOT/libs/libbreenix/src/layout.tsv"')
            self.assertIn(layout_call, rest)
            # Redirect the Docker mount names into host directories. Execute the
            # actual population commands; no mount, Docker or root privileges.
            for path, variable in (("/mnt/ext2", "MOUNT_DIR"), ("/fonts", "PROJECT_ROOT/fonts"),
                                   ("/breengel-data", "BREENGEL_DATA"),
                                   ("/breengel-frames", "BREENGEL_FRAMES")):
                value = '"$PROJECT_ROOT/fonts"' if variable == "PROJECT_ROOT/fonts" else f'"${variable}"'
                if index == 0:
                    population = re.sub(r"(?<![\w/])" + re.escape(path), lambda match: value, population)
            for arch in ("x86_64", "aarch64"):
                with self.subTest(path=index, arch=arch), tempfile.TemporaryDirectory() as temporary:
                    directory = Path(temporary)
                    root, binaries, stubs = (directory / name for name in ("root", "binaries", "stubs"))
                    for path in (root / "etc", binaries, stubs):
                        path.mkdir(parents=True)
                    for name in ("passwd", "group"):
                        (root / "etc" / name).touch()
                    for program in PROGRAMS:
                        (binaries / f"{program}.elf").write_bytes(program.encode())
                    # Ownership is outside this content check; copy, move,
                    # placement and mode operations remain real.
                    chown = stubs / "chown"
                    chown.write_text("#!/bin/sh\nexit 0\n")
                    chown.chmod(0o755)
                    env = dict(os.environ, ARCH=arch, BREENIX_ARCH=arch,
                               PROJECT_ROOT=str(ROOT), MOUNT_DIR=str(root),
                               BREENGEL_DATA=str(ROOT / "testdata/breengel"),
                               BREENGEL_FRAMES=str(ROOT / "docs/suites/frames/breengel"),
                               PATH=str(stubs) + os.pathsep + os.environ["PATH"])
                    subprocess.run(["/bin/bash", "-e", "-c", population],
                                   env=env, capture_output=True, text=True, check=True)
                    self.check_payload(root, "usr/local/test/share/breengel")
                    subprocess.run(["/bin/sh", str(ROOT / "scripts/install-layout.sh"),
                                    str(root), str(binaries), str(ROOT / "libs/libbreenix/src/layout.tsv")],
                                   env=env, capture_output=True, text=True, check=True)
                    for program, path in (("suite-breengel", "sbin/suite-breengel"),
                                          ("kiosk_test", "usr/libexec/breenix/kiosk_test"),
                                          ("raw-window_test", "usr/libexec/breenix/raw-window_test")):
                        self.assertEqual((root / path).read_bytes(), program.encode())
                        self.assertEqual((root / path).stat().st_mode & 0o777, 0o755)
                    self.check_payload(root, "usr/libexec/breenix/share/breengel")
                    self.assertFalse((root / "usr/local/test").exists())

    def check_payload(self, root, data_path):
        fonts = list((ROOT / "fonts").glob("*.ttf"))
        self.assertTrue(fonts)
        for font in fonts:
            self.assertEqual((root / "usr/share/fonts" / font.name).read_bytes(), font.read_bytes())
        for source, destination in ((ROOT / "testdata/breengel", root / data_path),
                                    (ROOT / "docs/suites/frames/breengel", root / data_path / "frames")):
            files = [path for path in source.rglob("*") if path.is_file()]
            self.assertTrue(files, str(source))
            for path in files:
                self.assertEqual((destination / path.relative_to(source)).read_bytes(), path.read_bytes())


if __name__ == "__main__":
    unittest.main()
