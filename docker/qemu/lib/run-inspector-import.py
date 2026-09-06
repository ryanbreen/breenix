#!/usr/bin/env python3
"""Persist gate provenance, then attempt a bounded local import."""
import datetime
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import uuid


def main():
    evidence, arch, profile, verdict, status, start, *command = sys.argv[1:]
    directory = Path(evidence)
    serials = sorted(p.name for p in directory.glob("serial*")
                     if p.is_file() and p.suffix in (".txt", ".log"))
    if not serials:
        return  # preflight: no evidence from this boot
    captures = sorted(p.name for p in directory.iterdir()
                      if p.is_file() and p.suffix in (".txt", ".log")
                      and p.name not in serials and p.name != "inspector-import.log")
    metadata = dict(schemaVersion=1, id=str(uuid.uuid4()), arch=arch,
                    profile=profile, verdict=verdict, exitCode=int(status),
                    startedAt=datetime.datetime.fromtimestamp(
                        int(start) / 1000, datetime.timezone.utc).isoformat(),
                    endedAt=datetime.datetime.now(datetime.timezone.utc).isoformat(),
                    command=command, serials=serials, captures=captures)
    sidecar = directory / "run-inspector.json"
    temporary = directory / ("run-inspector." + metadata["id"] + ".tmp")
    temporary.write_text(json.dumps(metadata) + "\n")
    temporary.replace(sidecar)
    binary = os.environ.get("BREENIX_RUNS_BIN")
    if binary is None:
        binary = shutil.which("breenix-runs")
        if binary is None:
            binary = str(Path(__file__).resolve().parents[3] /
                         "tools/breenix-runs/.build/release/breenix-runs")
    if not os.path.isfile(binary) or not os.access(binary, os.X_OK):
        print("warning: Run Inspector importer unavailable; metadata retained", flush=True)
        return
    limit = min(max(float(os.environ.get("BREENIX_RUNS_IMPORT_TIMEOUT", "15")), 0.1), 60)
    child = None

    def interrupted(signum, frame):
        raise InterruptedError("import interrupted by signal " + str(signum))

    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    try:
        child = subprocess.Popen([binary, "import", str(directory)], start_new_session=True)
        print("importer pid=" + str(child.pid), flush=True)
        status = child.wait(timeout=limit)
        if status:
            print("warning: Run Inspector importer exited " + str(status), flush=True)
    except (subprocess.TimeoutExpired, InterruptedError) as error:
        print("warning: " + str(error), flush=True)
    finally:
        if child is not None and child.poll() is None:
            # Signal the child group created and recorded by Popen above.
            os.killpg(child.pid, signal.SIGKILL)
            child.wait(timeout=2)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print("warning: Run Inspector capture/import failed: " + str(error), flush=True)
