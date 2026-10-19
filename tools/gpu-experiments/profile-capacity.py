"""Reproduce B10's historical capacity candidates through g3-egl.py.

Only Breenix-authored shader text is read from the named commit. No renderer
implementation source is used. Run from any checkout containing that commit.
"""
from pathlib import Path
import subprocess

root = Path(__file__).resolve().parents[2]
text = subprocess.check_output([
    'git', '-C', str(root), 'show',
    '09922c9652d3820b82e1c1cad007180a7f08f25a:kernel/src/gpu/virgl/profile/profile.tgsi',
]).rstrip(b'\0').decode()
assert grammar('G4 B10 capacity 21257', text, 21257) == 22
assert grammar('G4 B10 capacity 50000', text, 50000) == 0

# Preserve the historical header/immediates and select a complete outer branch.
# Nested UIF/loop blocks remain byte-for-byte in that candidate.
import re
lines = text.splitlines(keepends=True)
starts = [i for i, line in enumerate(lines)
          if re.match(r'^USEQ TEMP\[\d+\]\.x, CONST\[4\]\.yyyy, IMM\[', line)]
assert len(starts) == 25
prefix = ''.join(lines[:starts[0]])
for routine in [0, 11, 12, 23, 24]:
    end = starts[routine + 1] if routine + 1 < len(starts) else len(lines) - 1
    branch = prefix + ''.join(lines[starts[routine]:end]) + 'END\n'
    assert grammar(f'G4 B10 branch {routine} capacity 50000', branch, 50000) == 0
