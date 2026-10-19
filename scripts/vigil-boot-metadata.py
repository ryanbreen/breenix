#!/usr/bin/env python3
"""Complete the recorder's own new registration with launcher-supplied metadata.

The installed Vigil registration CLI does not accept display or renderer metadata.
Only start writes display, before the launcher starts its VM; finish archives the
actual renderer file before Vigil freezes the serial. Historical records are not
relabelled. Unknown display state leaves Vigil's field unchanged.
"""

import json
import os
from pathlib import Path
import pwd
import re
import shutil
import sys
import tempfile


def complete(action, boot_id, checkout):
    if not re.fullmatch(r"[0-9]{8}T[0-9]{6}Z[A-Za-z0-9_-]{0,80}", boot_id):
        raise ValueError("invalid boot id")
    home = Path(os.environ.get("VIGIL_TEST_HOME") or pwd.getpwuid(os.getuid()).pw_dir)
    folder = home / "Library/Application Support/Vigil/breenix/boots" / boot_id
    path = folder / "run.json"
    run = json.loads(path.read_text())
    if run["id"] != boot_id or Path(run["checkout"]).resolve() != Path(checkout).resolve():
        raise ValueError("registration belongs to another boot")
    if (folder / "exit-status").exists():
        raise ValueError("boot already finished")
    if action == "start":
        display = os.environ.get("BREENIX_VIGIL_DISPLAY", "")
        if not display:
            return
        if display not in ("true", "false"):
            raise ValueError("display must be true or false")
        run["display"] = display == "true"
        fd, tmp = tempfile.mkstemp(prefix=".run-", dir=folder)
        try:
            with os.fdopen(fd, "w") as out:
                json.dump(run, out, indent=2)
                out.write("\n")
            os.replace(tmp, path)
        finally:
            if os.path.exists(tmp):
                os.unlink(tmp)
    elif action == "finish":
        renderer = os.environ.get("BREENIX_HOST_RENDERER_JSON")
        if renderer:
            source = Path(renderer)
            # The launcher creates a fresh file beside this boot's serial. Use that
            # run's attestation and retain its original bytes.
            if source.parent.resolve() != (folder / "serial.txt").resolve().parent:
                raise ValueError("renderer does not belong to the serial's run folder")
            destination = folder / "host-renderer.json"
            if source.resolve() != destination.resolve():
                shutil.copyfile(source, destination)
        gpu_time = os.environ.get("BREENIX_GPU_TIME_JSONL")
        if gpu_time:
            source = Path(gpu_time)
            serial = (folder / "serial.txt").resolve()
            if source.resolve() != Path(str(serial) + ".gpu-time.jsonl"):
                raise ValueError("GPU time does not belong to the serial's run")
            shutil.copyfile(source, folder / "gpu-time.jsonl")
    else:
        raise ValueError("unknown recorder action")


if __name__ == "__main__":
    try:
        complete(*sys.argv[1:])
    except (OSError, ValueError, KeyError, TypeError) as error:
        sys.exit(f"Vigil boot metadata not recorded: {error}")
