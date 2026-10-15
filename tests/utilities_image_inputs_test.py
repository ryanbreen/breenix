"""The Breenix image build has no foreign libc recipe or unrecorded ELF input."""
from pathlib import Path
import ast
import re
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[1]

# The SDK suite's host harness builds, links and stages applications, so it is a recipe like
# any other; it is also the clean-room scan, which names the foreign libcs it rejects. Its marks
# sit in one delimited block of assignments, and elsewhere it may name them only through these
# identifiers and its toolchain/no-musl case.
SCANNER = 'scripts/sdk-suite.py'
MARKS_BEGIN = '# Clean-room marks: begin.'
MARKS_END = '# Clean-room marks: end.'
MARK_NAMES = {'MUSL_MARKS', 'GLIBC_MARKS', 'OTHER_LIBC_MARKS', 'MUSL_LIBM_SYMBOLS', 'MUSL_LIBM_BYTES'}
SCANNER_NAMES = re.compile(r"\b(?:MUSL_MARKS|MUSL_LIBM_SYMBOLS|MUSL_LIBM_BYTES)\b|'toolchain/no-musl'")


def without_scanner_marks(text):
    """The harness's text with its marks block and the names that refer to it taken out; fails
    if the block holds anything but assignments of those marks."""
    before, rest = text.split(MARKS_BEGIN, 1)
    block, after = rest.split('\n', 1)[1].split(MARKS_END, 1)
    for node in ast.parse(block).body:
        targets = node.targets if isinstance(node, ast.Assign) else []
        if not targets or any(not isinstance(t, ast.Name) or t.id not in MARK_NAMES for t in targets):
            raise AssertionError(f'{SCANNER}: its clean-room marks block holds more than the marks')
    return SCANNER_NAMES.sub('', before + after)


class ImageInputsTests(unittest.TestCase):
    def test_build_and_install_inputs_use_only_breenix_libc(self):
        # These are Breenix's build/install recipes, the Developer SDK's and its
        # suite harness's included. Vendored Rust libc platform declarations and
        # its upstream CI are not Breenix build recipes.
        names = subprocess.check_output([
            'git', '-C', str(ROOT), 'ls-files', '-z', '--', 'scripts', '.cargo',
            'run.sh', 'xtask', 'sdk', 'vendor/busybox/*.config', 'userspace/**/build.sh',
            'userspace/**/Makefile', 'userspace/**/*.ld']).split(b'\0')
        self.assertIn(SCANNER.encode(), names)
        for name in filter(None, names):
            path = ROOT / name.decode()
            if path.is_file():
                text = path.read_text()
                if name.decode() == SCANNER:
                    text = without_scanner_marks(text)
                self.assertNotIn('musl', text.lower(), str(path.relative_to(ROOT)))

    def test_the_scanner_exception_covers_only_its_marks(self):
        text = (ROOT / SCANNER).read_text()
        self.assertNotIn('musl', without_scanner_marks(text).lower())
        recipe = "    run(['cc', 'app.o', '/usr/lib/musl/lib/libc.a'])\n"
        self.assertIn('musl', without_scanner_marks(text + recipe).lower())
        smuggled = text.replace(MARKS_END, "run(['cc', '-lc'])\n" + MARKS_END, 1)
        with self.assertRaises(AssertionError):
            without_scanner_marks(smuggled)

    def test_both_image_paths_validate_the_build_manifest_before_install(self):
        source = (ROOT / 'scripts/create_ext2_disk.sh').read_text()
        check = source.index('userspace-manifest.py" check --arch "$ARCH"')
        self.assertLess(check, source.index('docker run'))
        self.assertLess(check, source.index('MOUNT_DIR=$(mktemp -d)'))
        self.assertIn('sh /install-libc-busybox.sh /binaries /required-applets /mnt/ext2', source)
        self.assertIn('"$SCRIPT_DIR/install-libc-busybox.sh" "$USERSPACE_DIR"', source)
        self.assertNotIn('busybox.elf belongs', (ROOT / 'userspace/programs/build.sh').read_text())


if __name__ == '__main__':
    unittest.main()
