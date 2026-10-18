#!/usr/bin/env python3
"""Checkpoint a probe_int routine: a diagnostic variant of profile.tgsi for locating a host deviation.

    python3 probe-checkpoints.py PROFILE_TGSI FIRST_LINE LAST_LINE ROUTINE OUT_TGSI

Inside ROUTINE's block, up to 64 instructions between FIRST_LINE and LAST_LINE (1-based) that write a
TEMP's x component become checkpoints: the fragment whose window x is j copies checkpoint j's register
into TEMP[61], and the routine's output packs TEMP[61] instead of its result. Drawing a 64-pixel PROBE
row of the routine and reading it back gives the 64 intermediate words in one submission; the generator's
interpreter (tools/gpu-profile/emitted_tests.rs) run on the same text gives the words the program
defines. OUT_TGSI.lines names each checkpoint's instruction. Used for provenance B19.
"""
import re,sys
src=open(sys.argv[1]).read()
lines=src.splitlines()
lo,hi=int(sys.argv[2]),int(sys.argv[3])
routine_block=int(sys.argv[4])
imm_idx=[i for i,l in enumerate(lines) if l.startswith('IMM[')]
last_imm=int(re.match(r'IMM\[(\d+)\]',lines[imm_idx[-1]]).group(1))
N=64
newimm=[f'IMM[{last_imm+1+k}] UINT32 {{{4*k}, {4*k+1}, {4*k+2}, {4*k+3}}}' for k in range(N//4)]
def jref(j): return f'IMM[{last_imm+1+j//4}].{"xyzw"[j%4]*4}'
block_start=[i for i,l in enumerate(lines) if l.startswith('USEQ TEMP[42].x, CONST[4].yyyy')][routine_block]
block_end=next((i for i in range(block_start+1,len(lines)) if lines[i].startswith('USEQ TEMP[42].x, CONST[4].yyyy')), len(lines))
u2f=max(i for i in range(block_start,block_end) if lines[i].startswith('U2F '))
pack=u2f-8
m=re.match(r'^USHR TEMP\[\d+\]\.\w, (TEMP\[\d+\])\.',lines[pack]); assert m, lines[pack]
res=m.group(1)
cands=[i for i in range(lo-1,hi) if block_start<i<pack and re.match(r'^[A-Z0-9]+ TEMP\[\d+\]\.x, ',lines[i]) and not lines[i].startswith('F2I')]
step=max(1,len(cands)/N)
chosen=sorted(set(cands[int(k*step)] for k in range(min(N,len(cands)))))
ck={c:j for j,c in enumerate(chosen)}
out=[]
for i,l in enumerate(lines):
    if l.startswith('DCL TEMP['): l='DCL TEMP[0..61]'
    if i==imm_idx[-1]:
        out.append(l); out.extend(newimm); continue
    if i==pack:
        out.append(f'MOV {res}.x, TEMP[61].xxxx')
    out.append(l)
    if i in ck:
        d=re.match(r'^[A-Z0-9]+ (TEMP\[\d+\])\.x',l).group(1)
        out += [f'USEQ TEMP[60].x, TEMP[0].xxxx, {jref(ck[i])}','UIF TEMP[60].xxxx',f'MOV TEMP[61].x, {d}.xxxx','ENDIF']
open(sys.argv[5],'w').write('\n'.join(out)+'\n')
open(sys.argv[5]+'.lines','w').write('\n'.join(f'{j} line {c+1}: {lines[c]}' for j,c in enumerate(chosen))+'\n')
print('block', block_start+1, block_end, 'pack line', pack+1, 'result', res, 'checkpoints', len(chosen))
