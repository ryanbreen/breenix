"""Exercise the service-sequence census's counter snapshot collector."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class CounterCensus(unittest.TestCase):
    def test_latest_snapshot_zero_and_unavailable_are_distinct(self):
        source = (ROOT / "docker/qemu/run-aarch64-service-sequence-gate.sh").read_text()
        start = source.index("        boot_divergence=$(sed")
        end = source.index("        boot_refusals=", start)
        collector = source[start:end]
        with tempfile.TemporaryDirectory() as directory:
            serial = Path(directory) / "serial.txt"
            for text, expected in [
                ("[CTX596_COUNTER:INLINE_ELR_DIVERGENCE=0]\n", "0 0 0 0"),
                ("[CTX596_COUNTER:INLINE_ELR_DIVERGENCE=9]\n"
                 "[CTX596_COUNTER:INLINE_ELR_DIVERGENCE=12]\n", "12 12 1 0"),
                ("[CTX596_ELR_DIVERGENCE]\n", "unavailable 0 0 1"),
            ]:
                with self.subTest(expected=expected):
                    serial.write_text(text)
                    script = ('serial_file="$1"\ndivergence_lines=0\n'
                              'divergence_boots=0\ndivergence_unavailable=0\n'
                              + collector
                              + 'printf "%s %s %s %s" "$boot_divergence" "$divergence_lines" '
                                '"$divergence_boots" "$divergence_unavailable"\n')
                    result = subprocess.run(["bash", "-eu", "-c", script, "collector", str(serial)],
                                            check=True, text=True, capture_output=True)
                    self.assertEqual(result.stdout, expected)


if __name__ == "__main__":
    unittest.main()
