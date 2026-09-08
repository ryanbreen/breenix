import os, signal
from pathlib import Path
# Only processes whose cwd belongs to this lane and which are debug children.
for p in Path('/proc').iterdir():
    if not p.name.isdigit(): continue
    try:
        cwd=str((p/'cwd').resolve())
        args=(p/'cmdline').read_bytes().split(b'\0')
        exe=Path(os.fsdecode(args[0])).name
        if (cwd == '<repo>' and exe in ('qemu-system-x86_64', 'gdb')) or (exe == 'tail' and b'<lane-tmp>/fresh-gdb-session/input.fifo' in args):
            os.kill(int(p.name), signal.SIGTERM)
    except (OSError, IndexError): pass
