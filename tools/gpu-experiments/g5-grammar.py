"""G5 catalogue text acceptance through the pinned public renderer API."""
from pathlib import Path
import re
base = 'FRAG\nDCL OUT[0], COLOR\nDCL CONST[0]\nDCL TEMP[0]\nMOV OUT[0], CONST[0]\nEND\n'
assert grammar('G5 grammar baseline', base, 64) == 0
assert grammar('G5 unconditional discard', base.replace('END\n', 'KILL\nEND\n'), 64) == 0
assert grammar('G5 float-to-unsigned', base.replace('MOV OUT[0], CONST[0]', 'F2U OUT[0], CONST[0]'), 64) == 0
conditional = base.replace('MOV OUT[0], CONST[0]', 'UIF CONST[0].xxxx\nMOV OUT[0], CONST[0]\nENDIF')
assert grammar('G5 conditional baseline', conditional, 64) == 0
assert grammar('G5 discard alternative', conditional.replace('ENDIF', 'ELSE\nKILL\nENDIF'), 64) == 0
sampler = base.replace('DCL TEMP[0]', 'DCL TEMP[0]\nDCL SAMP[0]\nDCL SVIEW[0], 2D, FLOAT')
assert grammar('G5 sampler baseline', sampler, 64) == 0
assert grammar('G5 sampler ranges', sampler.replace('SAMP[0]', 'SAMP[0..2]').replace('SVIEW[0]', 'SVIEW[0..2]'), 64) == 0
root = Path(__file__).resolve().parents[2]
programs = root / 'kernel/src/gpu/virgl/pipelines'
module = (programs / 'mod.rs').read_text()
for pipeline, tokens in re.findall(r'\((\d+), Program.*?tokens: (\d+)', module):
    text = (programs / (pipeline + '.tgsi')).read_text()
    assert grammar('G5 pipeline ' + pipeline, text, int(tokens)) == 0
