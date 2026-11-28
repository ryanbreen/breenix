"""Exercise the bootstrap launcher with a fake CURRENT toolchain."""

from pathlib import Path
import shlex
import subprocess
from tempfile import TemporaryDirectory
import unittest


LAUNCHER = Path(__file__).resolve().parents[1] / "userland/btest/bootstrap/bcargo"


class BcargoTest(unittest.TestCase):
    def test_cargo_environment_has_no_rustc_wrapper(self):
        with TemporaryDirectory() as directory:
            home = Path(directory)
            root = home / "rust"
            toolchain = root / "x86_64-test"
            cargo = toolchain / "bin/cargo"
            cargo.parent.mkdir(parents=True)
            (root / "CURRENT").write_text(f"{toolchain.name}\n", encoding="utf-8")
            cargo.write_text("#!/bin/sh\nexec /usr/bin/env\n", encoding="utf-8")
            cargo.chmod(0o755)

            # Use a fixture toolchain and the host's env executable.
            launcher = home / "bcargo"
            launcher.write_text(
                LAUNCHER.read_text(encoding="utf-8").replace(
                    "root=/opt/breenix/rust", f"root={shlex.quote(str(root))}", 1
                ).replace("/bin/env ", "/usr/bin/env "),
                encoding="utf-8",
            )
            result = subprocess.run(
                ["/bin/sh", str(launcher), "build", "-p", "yes"],
                cwd=home,
                env={"HOME": str(home), "RUSTC_WRAPPER": "/inherited-wrapper"},
                capture_output=True, text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            cargo_env = dict(line.split("=", 1) for line in result.stdout.splitlines())
            self.assertEqual(cargo_env["RUSTC"], str(toolchain / "bin/rustc"))
            self.assertNotIn("RUSTC_WRAPPER", cargo_env)


if __name__ == "__main__":
    unittest.main()
