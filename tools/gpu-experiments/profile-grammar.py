"""G4 grammar experiments through g3-egl.py's public renderer API.

QEMU_GL_FRAMEWORKS identifies the pinned GL bundle's Frameworks directory.
Candidates are Breenix-authored TGSI, rather than captured renderer programs.
"""
from pathlib import Path
import re
base = 'FRAG\nDCL OUT[0], COLOR\nDCL CONST[0]\nDCL TEMP[0]\nIMM[0] UINT32 {0, 1, 4294967295, 0}\nMOV TEMP[0], IMM[0]\nUADD TEMP[0].x, CONST[0].xxxx, IMM[0].yyyy\nU2F OUT[0], TEMP[0]\nEND\n'
assert grammar('G4 replicated scalar baseline', base, 25) == 0
assert grammar('G4 scalar source shorthand', base.replace('CONST[0].xxxx', 'CONST[0].x'), 25) == 22
assert grammar('G4 integer conditional', base.replace('U2F OUT', 'UIF IMM[0].yyyy\nMOV TEMP[0], IMM[0]\nENDIF\nU2F OUT'), 31) == 0
assert grammar('G4 bounded loop syntax', base.replace('U2F OUT', 'BGNLOOP\nBRK\nENDLOOP\nU2F OUT'), 28) == 0
root = Path(__file__).resolve().parents[2]
source = root / 'kernel/src/gpu/virgl/profile'
text = (source / 'profile.tgsi').read_bytes().rstrip(b'\0').decode()
tokens = int(re.search(r'tokens: (\d+)', (source / 'mod.rs').read_text()).group(1))
assert grammar('G4 combined baseline', text, tokens) == 0
assert grammar('G4 missing immediate type', text.replace('IMM[0] UINT32', 'IMM[0]', 1), tokens) == 22
