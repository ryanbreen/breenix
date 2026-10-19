#!/usr/bin/env python3
"""Record a runner's process identity; authorize cleanup only after it is gone."""

import json
import os
from pathlib import Path
import subprocess
import sys


def started(pid):
    result = subprocess.run(['ps', '-p', str(pid), '-o', 'lstart='],
                            capture_output=True, text=True, check=False)
    return result.stdout.strip() if result.returncode == 0 else ''


def dead(path):
    try:
        owner = json.loads(path.read_text())
        pid, birth = owner['pid'], owner['started']
        if type(pid) is not int or pid <= 0 or not isinstance(birth, str) or not birth:
            return False
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return True
        except PermissionError:
            return False
        current = started(pid)
        # An unavailable process query is not proof that an owner died.
        return bool(current) and current != birth
    except (OSError, ValueError, KeyError, TypeError):
        return False


def main():
    action, filename = sys.argv[1:3]
    path = Path(filename)
    if action == 'dead':
        return 0 if dead(path) else 1
    if action != 'record':
        return 2
    pid = int(sys.argv[3])
    birth = started(pid)
    if not birth:
        return 1
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + '.tmp-' + str(os.getpid()))
    temporary.write_text(json.dumps({'pid': pid, 'started': birth}) + '\n')
    temporary.replace(path)
    return 0


if __name__ == '__main__':
    sys.exit(main())
