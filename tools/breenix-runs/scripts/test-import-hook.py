#!/usr/bin/env python3
"""Fixture gate harness: compare pre-hook and post-hook status and output bytes."""
import json
import os
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[3]
helper = root / "docker/qemu/lib/run-inspector-import.sh"
with tempfile.TemporaryDirectory(dir=root / "tools/breenix-runs/.build") as temp:
    work = Path(temp)
    fake = work / "importer"
    fake.write_text("""#!/usr/bin/env python3
import os, signal, sys, time
mode=os.environ['FAKE_MODE']
if mode=='nonzero': sys.exit(42)
if mode=='unwritable': open(os.environ['BREENIX_RUNS_STORE']+'/manifest.json','w')
if mode=='interrupted': os.kill(os.getpid(),signal.SIGTERM)
if mode=='timeout': time.sleep(60)
print('importer chatter must not reach gate stdout')
""")
    fake.chmod(0o755)
    blocked = work / "not-a-directory"
    blocked.write_text("store cannot be created here")
    # Fixed verdicts are synthesized; no fixture execution is called a QEMU boot.
    harness = """
set -euo pipefail
source "$HELPER"
if [ "$PREFLIGHT" = 1 ]; then
    printf 'REFUSED: preflight failed before QEMU\n'
    exit 7
fi
printf '%s\n' "$VERDICT"
if [ "$HOOK" = 1 ]; then
    breenix_runs_import_nonfatal "$EVIDENCE" aarch64 testing "$VERDICT" "$STATUS" 1788730000000 fixture-gate 1 || :
fi
exit "$STATUS"
"""
    cases = 0
    for verdict, status in [("PASS", 0), ("FAIL", 3), ("PASS-WITH-ATTRIBUTED-LOCKUP", 0)]:
        for mode in ["success", "missing", "nonzero", "unwritable", "interrupted", "timeout", "optout", "preflight"]:
            evidence = work / (verdict + "-" + mode)
            evidence.mkdir()
            (evidence / "serial.txt").write_text("Breenix ARM64 Kernel Starting\n")
            env = dict(os.environ, HELPER=str(helper), EVIDENCE=str(evidence),
                       VERDICT=verdict, STATUS=str(status), FAKE_MODE=mode,
                       BREENIX_RUNS_BIN=str(work / "missing") if mode == "missing" else str(fake),
                       BREENIX_RUNS_STORE=str(blocked), BREENIX_RUNS_IMPORT_TIMEOUT="0.15",
                       BREENIX_RUNS_NO_IMPORT="1" if mode == "optout" else "0",
                       PREFLIGHT="1" if mode == "preflight" else "0")
            before = subprocess.run(["/bin/bash", "-c", harness], env=dict(env, HOOK="0"), capture_output=True, timeout=5)
            after = subprocess.run(["/bin/bash", "-c", harness], env=dict(env, HOOK="1"), capture_output=True, timeout=5)
            assert (after.returncode, after.stdout, after.stderr) == (before.returncode, before.stdout, before.stderr), (verdict, mode, after)
            sidecar = evidence / "run-inspector.json"
            if mode in ("preflight", "optout"):
                assert not sidecar.exists()
            else:
                metadata = json.loads(sidecar.read_text())
                assert metadata["verdict"] == verdict and metadata["exitCode"] == status
                if mode != "success":
                    assert "warning:" in (evidence / "inspector-import.log").read_text()
            cases += 1
            print(f"{verdict} / {mode}: identical exit={after.returncode}, stdout={after.stdout!r}, stderr={after.stderr!r}")
    print(f"{cases} fixture comparisons passed (no QEMU launched)")
