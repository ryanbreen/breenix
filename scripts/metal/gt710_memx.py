#!/usr/bin/env python3
# /*
#  * Copyright 2012 Red Hat Inc.
#  * Copyright 2013 Red Hat Inc.
#  *
#  * Permission is hereby granted, free of charge, to any person obtaining a
#  * copy of this software and associated documentation files (the "Software"),
#  * to deal in the Software without restriction, including without limitation
#  * the rights to use, copy, modify, merge, publish, distribute, sublicense,
#  * and/or sell copies of the Software, and to permit persons to whom the
#  * Software is furnished to do so, subject to the following conditions:
#  *
#  * The above copyright notice and this permission notice shall be included in
#  * all copies or substantial portions of the Software.
# claim-lint:ok: Original MIT grant, pinned source hashes in docs/gui/provenance/gt710-c5a-sources.json.
#  *
#  * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
#  * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
#  * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
#  * THE COPYRIGHT HOLDER(S) OR AUTHOR(S) BE LIABLE FOR ANY CLAIM, DAMAGES OR
#  * OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
#  * ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
#  * OTHER DEALINGS IN THE SOFTWARE.
#  *
#  * Authors: Ben Skeggs
#  */
# /*
#  * Copyright 2010 Red Hat Inc.
#  *
#  * Permission is hereby granted, free of charge, to any person obtaining a
#  * copy of this software and associated documentation files (the "Software"),
#  * to deal in the Software without restriction, including without limitation
#  * the rights to use, copy, modify, merge, publish, distribute, sublicense,
#  * and/or sell copies of the Software, and to permit persons to whom the
#  * Software is furnished to do so, subject to the following conditions:
#  *
#  * The above copyright notice and this permission notice shall be included in
#  * all copies or substantial portions of the Software.
# claim-lint:ok: Original MIT grant, pinned source hashes in docs/gui/provenance/gt710-c5a-sources.json.
#  *
#  * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
#  * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
#  * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
#  * THE COPYRIGHT HOLDER(S) OR AUTHOR(S) BE LIABLE FOR ANY CLAIM, DAMAGES OR
#  * OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
#  * ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
#  * OTHER DEALINGS IN THE SOFTWARE.
#  *
#  * Authors: Ben Skeggs
#  */
# /*
#  * Copyright 2014 Red Hat Inc.
#  *
#  * Permission is hereby granted, free of charge, to any person obtaining a
#  * copy of this software and associated documentation files (the "Software"),
#  * to deal in the Software without restriction, including without limitation
#  * the rights to use, copy, modify, merge, publish, distribute, sublicense,
#  * and/or sell copies of the Software, and to permit persons to whom the
#  * Software is furnished to do so, subject to the following conditions:
#  *
#  * The above copyright notice and this permission notice shall be included in
#  * all copies or substantial portions of the Software.
# claim-lint:ok: Original MIT grant, pinned source hashes in docs/gui/provenance/gt710-c5a-sources.json.
#  *
#  * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
#  * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
#  * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
#  * THE COPYRIGHT HOLDER(S) OR AUTHOR(S) BE LIABLE FOR ANY CLAIM, DAMAGES OR
#  * OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
#  * ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
#  * OTHER DEALINGS IN THE SOFTWARE.
#  *
#  * Authors: Ben Skeggs
#  */
# /*
#  * Copyright 2005-2006 Erik Waling
#  * Copyright 2006 Stephane Marchesin
#  * Copyright 2007-2009 Stuart Bennett
#  *
#  * Permission is hereby granted, free of charge, to any person obtaining a
#  * copy of this software and associated documentation files (the "Software"),
#  * to deal in the Software without restriction, including without limitation
#  * the rights to use, copy, modify, merge, publish, distribute, sublicense,
#  * and/or sell copies of the Software, and to permit persons to whom the
#  * Software is furnished to do so, subject to the following conditions:
#  *
#  * The above copyright notice and this permission notice shall be included in
#  * all copies or substantial portions of the Software.
# claim-lint:ok: Original MIT grant, pinned source hashes in docs/gui/provenance/gt710-c5a-sources.json.
#  *
#  * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
#  * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
#  * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
#  * THE AUTHORS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
#  * WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF
#  * OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
#  * SOFTWARE.
#  */
# SPDX-License-Identifier: MIT
# Adapted from individually checked MIT nouveau files at Linux v6.8
# e8f897f4afef0031fe618a8e94127a0934896aba. See gt710-c5a.md and THIRD_PARTY_NOTICES.
# sddr3.c authors: Ben Skeggs and Roy Spliet.
# ramfuc.h, pmu/memx.c and include/nvkm/core/device.h declare SPDX MIT.
# claim-lint:ok: The preceding original license grants are retained under G1.
"""Offline GK104 DDR3 calculation. No transport, MMIO, deployment or execution API.

A partial snapshot emits a conditional review trace, not fabricated PMU words.
With complete synthetic inputs, the same code lowers ramfuc no-op suppression
and memx.c batching to the wire words. Read/modify/write data is NOT masked
before OR, matching ramfuc.h and core/device.h.
"""
import argparse
import copy
from dataclasses import dataclass
import json
from pathlib import Path

U32 = 0xffffffff
REVISION = 'e8f897f4afef0031fe618a8e94127a0934896aba'


def need(ok, message):
    if not ok:
        raise ValueError(message)


class Image:
    """A full private ROM or selected sparse table bytes, with bounds checks."""
    def __init__(self, data):
        self.parts = [(0,data)] if isinstance(data,bytes) else sorted(
            (int(k,0),bytes.fromhex(v)) for k,v in data.items())

    def bytes(self, offset, size):
        for start, data in self.parts:
            if start <= offset and offset+size <= start+len(data):
                return data[offset-start:offset-start+size]
        raise ValueError(f'missing table bytes {offset:#x}+{size:#x}')

    def n(self, offset, size=1):
        return int.from_bytes(self.bytes(offset,size),'little')


def bit_entries(r, offset):
    h = r.bytes(offset,12)
    need(h[:6] == b'\xff\xb8BIT\0' and h[8]>=12 and h[9]==6,'BIT geometry')
    result = {}
    for i in range(h[10]):
        e = r.bytes(offset+h[8]+i*6,6)
        result[chr(e[0])] = (e[1],int.from_bytes(e[4:6],'little'),int.from_bytes(e[2:4],'little'))
    return result


def ramcfg_index(r, entry, strap):
    # bios/ramcfg.c:31-85; bios/M0203.c:31-149.
    v,p,length = entry
    xlat = 0
    if v == 1 and length >= 5:
        xlat = r.n(p+3,2)
    if v == 2 and length >= 3:
        if length >= 7:
            t = r.n(p+3,2)
            if t and r.n(t)==0x10:
                h,l,c = r.n(t+1),r.n(t+2),r.n(t+3)
                if r.n(t+4)==0:  # M0203T_TYPE_RAMCFG
                    for i in range(c):
                        e=t+h+i*l
                        if r.n(e)>>4 == strap:
                            return r.n(e+1)&15
        xlat = r.n(p+1,2)
    return r.n(xlat+strap) if xlat else strap


def fields(raw, spec):
    r=Image(raw)
    return {name:(r.n(off,size)&mask)>>shift for name,off,size,mask,shift in spec}

RAMMAP_FIELDS = [
    ('rammap_min', 0x0, 2, 0xffff, 0),
    ('rammap_max', 0x2, 2, 0xffff, 0),
    ('rammap_11_08_01', 0x8, 1, 0x1, 0),
    ('rammap_11_08_0c', 0x8, 1, 0xc, 2),
    ('rammap_11_08_10', 0x8, 1, 0x10, 4),
    ('rammap_11_09_01ff', 0x9, 4, 0x1ff, 0),
    ('rammap_11_0a_03fe', 0x9, 4, 0x3fe00, 9),
    ('rammap_11_0a_0400', 0x9, 4, 0x40000, 18),
    ('rammap_11_0a_0800', 0x9, 4, 0x80000, 19),
    ('rammap_11_0b_01f0', 0x9, 4, 0x1f00000, 20),
    ('rammap_11_0b_0200', 0x9, 4, 0x2000000, 25),
    ('rammap_11_0b_0400', 0x9, 4, 0x4000000, 26),
    ('rammap_11_0b_0800', 0x9, 4, 0x8000000, 27),
    ('rammap_11_0d', 0xd, 1, 0xff, 0),
    ('rammap_11_0e', 0xe, 1, 0xff, 0),
    ('rammap_11_0f', 0xf, 1, 0xff, 0),
    ('rammap_11_11_0c', 0x11, 1, 0xc, 2),
]

RAMCFG_FIELDS = [
    ('ramcfg_11_01_01', 0x1, 1, 0x1, 0),
    ('ramcfg_11_01_02', 0x1, 1, 0x2, 1),
    ('ramcfg_11_01_04', 0x1, 1, 0x4, 2),
    ('ramcfg_11_01_08', 0x1, 1, 0x8, 3),
    ('ramcfg_11_01_10', 0x1, 1, 0x10, 4),
    ('ramcfg_DLLoff', 0x1, 1, 0x20, 5),
    ('ramcfg_11_01_40', 0x1, 1, 0x40, 6),
    ('ramcfg_11_01_80', 0x1, 1, 0x80, 7),
    ('ramcfg_11_02_03', 0x2, 1, 0x3, 0),
    ('ramcfg_11_02_04', 0x2, 1, 0x4, 2),
    ('ramcfg_11_02_08', 0x2, 1, 0x8, 3),
    ('ramcfg_11_02_10', 0x2, 1, 0x10, 4),
    ('ramcfg_11_02_40', 0x2, 1, 0x40, 6),
    ('ramcfg_11_02_80', 0x2, 1, 0x80, 7),
    ('ramcfg_11_03_0f', 0x3, 1, 0xf, 0),
    ('ramcfg_11_03_30', 0x3, 1, 0x30, 4),
    ('ramcfg_11_03_c0', 0x3, 1, 0xc0, 6),
    ('ramcfg_11_03_f0', 0x3, 1, 0xf0, 4),
    ('ramcfg_11_04', 0x4, 1, 0xff, 0),
    ('ramcfg_11_06', 0x6, 1, 0xff, 0),
    ('ramcfg_11_07_02', 0x7, 1, 0x2, 1),
    ('ramcfg_11_07_04', 0x7, 1, 0x4, 2),
    ('ramcfg_11_07_08', 0x7, 1, 0x8, 3),
    ('ramcfg_11_07_10', 0x7, 1, 0x10, 4),
    ('ramcfg_11_07_40', 0x7, 1, 0x40, 6),
    ('ramcfg_11_07_80', 0x7, 1, 0x80, 7),
    ('ramcfg_11_08_01', 0x8, 1, 0x1, 0),
    ('ramcfg_11_08_02', 0x8, 1, 0x2, 1),
    ('ramcfg_11_08_04', 0x8, 1, 0x4, 2),
    ('ramcfg_11_08_08', 0x8, 1, 0x8, 3),
    ('ramcfg_11_08_10', 0x8, 1, 0x10, 4),
    ('ramcfg_11_08_20', 0x8, 1, 0x20, 5),
    ('ramcfg_11_09', 0x9, 1, 0xff, 0),
]

TIMING_FIELDS = [
    ('timing_20_2e_03', 0x2e, 1, 0x3, 0),
    ('timing_20_2e_30', 0x2e, 1, 0x30, 4),
    ('timing_20_2e_c0', 0x2e, 1, 0xc0, 6),
    ('timing_20_2f_03', 0x2f, 1, 0x3, 0),
    ('timing_20_2c_003f', 0x2c, 2, 0x3f, 0),
    ('timing_20_2c_1fc0', 0x2c, 2, 0x1fc0, 6),
    ('timing_20_30_07', 0x30, 1, 0x7, 0),
    ('timing_20_30_f8', 0x30, 1, 0xf8, 3),
    ('timing_20_31_0007', 0x31, 2, 0x7, 0),
    ('timing_20_31_0078', 0x31, 2, 0x78, 3),
    ('timing_20_31_0780', 0x31, 2, 0x780, 7),
    ('timing_20_31_0800', 0x31, 2, 0x800, 11),
    ('timing_20_31_7000', 0x31, 2, 0x7000, 12),
    ('timing_20_31_8000', 0x31, 2, 0x8000, 15),
]

def parse_rammap(r, p, index):
    v,h,b,s,n,c = r.bytes(p,6)
    need(v==0x11 and h>=6 and b>=0x12 and s>=10 and index<n,'GK104 rammap geometry')
    r.bytes(p,h+c*(b+s*n))
    result=[]
    for i in range(c):
        q=p+h+i*(b+s*n)
        raw=r.bytes(q,b);cfg=r.bytes(q+b+index*s,s)
        d=fields(raw,RAMMAP_FIELDS)
        d.update(fields(cfg,RAMCFG_FIELDS))
        d.update(min_mhz=r.n(q,2),max_mhz=r.n(q+2,2),rammap_offset=q,
                 ramcfg_offset=q+b+index*s,rammap_raw=raw.hex(),ramcfg_raw=cfg.hex(),
                 timing_index=cfg[0])
        result.append(d)
    return result


def parse_timing(raw):
    need(len(raw)>=0x33,'GK104 timing entry geometry')
    return dict(fields(raw,TIMING_FIELDS),timing=[int.from_bytes(raw[i*4:i*4+4],'little') for i in range(11)],
                timing_ver=0x20,timing_raw=raw.hex())


def parse_pll(r, p, kind, crystal):
    # bios/pll.c:94-120,201-220,369-384 (v40).
    v,h,l,c=r.bytes(p,4)
    need(v==0x40 and l>=11,'PLL table v40 geometry')
    for i in range(c):
        q=p+h+i*l
        if r.n(q)!=kind:continue
        data=r.n(q+1,2)
        names=['min_freq','max_freq','min_inputfreq','max_inputfreq']
        d={name:r.n(data+j*2,2)*1000 for j,name in enumerate(names)}
        d.update({name:r.n(data+j+8) for j,name in enumerate(['min_m','max_m','min_n','max_n','min_p','max_p'])})
        d.update(type=kind,offset=q,limits_offset=data,raw=r.bytes(q,l).hex(),
                 limits_raw=r.bytes(data,14).hex(),refclk=r.n(q+9,2)*1000 or crystal,
                 alternate_refclk_raw_khz=r.n(q+7,2)*1000,register=r.n(q+3,4))
        return d
    raise ValueError(f'PLL type {kind:#x} absent')


def decode(fixture):
    r=Image(fixture['segments']);bits=bit_entries(r,fixture['bit_offset'])
    regs={int(a,0):int(v,0) if isinstance(v,str) else v for a,v in fixture['registers'].items()}
    strap=(regs[0x101000]&0x3c)>>2
    index=ramcfg_index(r,bits['M'],strap)
    crystal={0:13500,0x40:14318,0x400000:27000,0x400040:25000}[regs[0x101000]&0x400040]
    pv,pp,length=bits['P'];need(pv==2 and length>=12,'BIT P v2 required')
    mp,tp=r.n(pp+4,4),r.n(pp+8,4)
    configs=parse_rammap(r,mp,index)
    v,h,b,s,n,c=r.bytes(tp,6);need(v==0x20 and b>=0x33,'timing table v20 required')
    for d in configs:
        ti=d['timing_index']
        if ti==0xff:
            d.update(timing_ver=0,timing=[0]*11)
            continue
        need(ti<c,'timing index out of bounds')
        q=tp+h+ti*(b+s*n)
        d.update(parse_timing(r.bytes(q,b)),timing_offset=q)
    cv,cp,length=bits['C']
    p=r.n(cp+8,2) if cv==1 and length>=10 else r.n(cp,4)
    # Board GPIO v41, the same DCB mapping used by gt710_clock.gpio.
    dcb=r.n(0x36,2);gp=r.n(dcb+10,2);gv,gh,gc,gs=r.bytes(gp,4)
    need(gv==0x41 and gs>=5,'GPIO v41 geometry')
    gpio=[]
    for i in range(gc):
        q=gp+gh+i*gs;info=r.n(q,4);logs=r.n(q+4)
        if (info>>8)&255 in (0x18,0x2e):
            gpio.append(dict(function=(info>>8)&255,register=0xd610+(info&63)*4,
                             values=[(((logs>>4)&3)^2)<<12,(((logs>>6)&3)^2)<<12]))
    return dict(strap=strap,ramcfg_index=index,crystal_khz=crystal,configs=configs,
                refpll=parse_pll(r,p,0x0c,crystal),mempll=parse_pll(r,p,0x04,crystal),gpio=gpio,
                rammap_offset=mp,timing_offset=tp)


def select(decoded, khz):
    for d in decoded['configs']:
        if d['min_mhz']<=khz//1000<=d['max_mhz']:return copy.deepcopy(d)
    raise ValueError(f'no RAMMAP entry for {khz} kHz')


def pll(info, freq):
    # clk/pllgt215.c:30-90, with non-null pfN as ramgk104.c:1076 uses.
    p=max(info['min_p'],min(info['max_p'],info['max_freq']//freq))
    lo=max((info['refclk']+info['max_inputfreq'])//info['max_inputfreq'],info['min_m'])
    hi=min((info['refclk']+info['min_inputfreq'])//info['min_inputfreq'],info['max_m'])
    lo=min(lo,hi)
    for m in range(lo,hi+1):
        tmp=freq*p*m
        n,frac=divmod(tmp,info['refclk'])
        if frac<info['refclk']//2:n-=1
        frac=tmp-n*info['refclk']
        if n<info['min_n']:continue
        if n>info['max_n']:break
        fn=(((frac<<13)+info['refclk']//2)//info['refclk']-4096)&0xffff
        return m,n,p,fn
    raise ValueError('nouveau gt215_pll_calc returned EINVAL')


@dataclass(frozen=True)
class Value:
    number: object
    text: str

    def __and__(self, mask):
        mask &= U32
        if mask==0:return value(0)
        if mask==U32:return self
        if self.number is not None:return value(self.number&mask)
        return Value(None,f'({self.text} & {mask:#010x})')

    def __or__(self, other):
        other=value(other)
        if other.number==0:return self
        if self.number==0:return other
        if self.number is not None and other.number is not None:return value(self.number|other.number)
        return Value(None,f'({self.text} | {other.text})')

    def json(self):return self.number if self.number is not None else self.text


def value(x):
    return x if isinstance(x,Value) else Value(x&U32,f'{x&U32:#010x}')


def different(a,b):
    a,b=value(a),value(b)
    if a==b:return False
    if a.number is not None and b.number is not None:return a.number!=b.number
    return f'{a.text} != {b.text}'


def conjunction(a,b):
    if a is False or b is False:return False
    if a is True:return b
    if b is True:return a
    return f'({a}) && ({b})'


class Script:
    def __init__(self, registers):
        self.state={a:value(v) for a,v in registers.items()}
        self.missing=set();self.ops=[];self.serial=0;self.guard=True

    def read(self,a):
        if a not in self.state:
            self.missing.add(a);self.state[a]=Value(None,f'R[{a:#08x}]')
        return self.state[a]

    def write(self,a,data,source,mask=U32,requested=None,force=False,cpu=False):
        if self.guard is False:return
        data=value(data)
        self.serial+=1;symbol=f't{self.serial:03d}'
        old=self.read(a) if requested is not None else self.state.get(a)
        changed=True if requested is None or force or cpu else different(old,data)
        emit=conjunction(self.guard,changed)
        self.ops.append(dict(op='CPU_WR32' if cpu else 'WR32',register=a,mask=mask,
                             data=value(requested).json() if requested is not None else data.json(),
                             result=data.json(),symbol=symbol,args=[a,data.json()],force=force,
                             guard=self.guard,emission_condition=emit,
                             emitted=emit if isinstance(emit,bool) else None,source=source))
        if self.guard is False:return
        after=data if data.number is not None else Value(None,symbol)
        if self.guard is not True:
            old=self.read(a)
            if old!=data:after=Value(None,f'select({self.guard}, {after.text}, {old.text})')
        self.state[a]=after

    def mask(self,a,mask,data,source,force=False,cpu=False):
        if self.guard is False:return
        old=self.read(a);self.write(a,(old&~mask)|data,source,mask,data,force,cpu)

    def command(self,op,args,source):
        if self.guard is False:return
        self.ops.append(dict(op=op,args=args,source=source,guard=self.guard,
                             emission_condition=self.guard,
                             emitted=self.guard if isinstance(self.guard,bool) else None))

    def nsec(self,n,line):self.command('DELAY',[n],src(line))


def src(line):return f'fb/ramgk104.c:{line}'


def mode_registers(s,c):
    # fb/sddr3.c:72-139, including its inherited ODT, not a guessed BIOS ODT.
    need(c['timing_ver']==0x20,'DDR3 timing version not supported by this slice')
    t=c['timing'];cl=t[1]&31;cwl=(t[1]&0xf80)>>7;wr=(t[2]&0x7f0000)>>16
    clmap={5:2,6:4,7:6,8:8,9:10,10:12,11:14,12:1,13:3,14:5}
    wrmap={5:1,6:2,7:3,8:4,10:5,12:6,14:7,15:7,16:0}
    cwlmap={5:0,6:1,7:2,8:3,9:4,10:5}
    need(cl in clmap and cwl in cwlmap and wr in wrmap,'nouveau sddr3_calc returned EINVAL')
    old=[s.read(a) for a in (0x10f300,0x10f304,0x10f320)]
    cl,wr,cwl=clmap[cl],wrmap[wr],cwlmap[cwl]
    return [(old[0]&~0xf74)|((wr&7)<<9)|((cl&14)<<3)|((cl&1)<<2),
            (old[1]&~1)|c['ramcfg_DLLoff'],(old[2]&~0x38)|((cwl&7)<<3)]


def dll_reset(s):
    s.mask(0x10f300,0x100,0x100,src(684),force=True)
    s.mask(0x10f300,0x100,0,src(685))


def gpio_set(s,gpios,tag,index,line):
    for g in gpios:
        if g['function']!=tag:continue
        old=s.read(g['register'])
        s.mask(g['register'],0x3000,g['values'][index],src(line))
        guard=s.guard;s.guard=conjunction(guard,different(old,s.read(g['register'])))
        s.write(0xd604,1,src(line+2))
        s.nsec(64000 if tag==0x18 else 20000,line+3)
        s.guard=guard


def ddr3(s,c,coeff,mode,gpios,display):
    mr=mode_registers(s,c);m,n,p,fn=coeff;rcoef=(p<<16)|(n<<8)|m
    M=lambda a,mask,data,line:s.mask(a,mask,data,src(line))
    W=lambda a,data,line:s.write(a,data,src(line))
    M(0x10f808,0x40000000,0x40000000,711)
    s.command('ENTER',[],src(712))
    if display:W(0x62c000,0x0f0f0000,715)
    vc=not c['ramcfg_11_02_08'];mv=not c['ramcfg_11_02_04']
    if vc:gpio_set(s,gpios,0x2e,1,718)
    M(0x10f200,0x800,0,725)
    if c['ramcfg_11_03_f0']:M(0x10f808,0x4000000,0x4000000,727)
    W(0x10f314,1,729)
    if c['ramcfg_DLLoff']:
        guard=s.guard;s.guard=conjunction(guard,different(s.read(0x10f304)&1,value(1)))
        M(0x10f304,1,1,694);s.nsec(1000,695);s.guard=guard
    W(0x10f210,0,734);W(0x10f310,1,735)
    M(0x10f200,0x80000000,0x80000000,736);W(0x10f310,1,737)
    M(0x10f200,0x80000000,0,738);s.nsec(1000,739)
    W(0x10f090,0x60,741);W(0x10f090,0xc000007e,742)
    mask=data=0x800907e0
    data &= ~[0x400,0x80000000,0x100,0x40][c['ramcfg_11_03_c0']]
    data &= ~[0x200,0x80000,0x80,0x20][c['ramcfg_11_03_30']]
    for field,bits in [('ramcfg_11_02_80',0x3000000),('ramcfg_11_02_40',0x2000),('ramcfg_11_07_10',0x4000)]:
        if c[field]:mask|=bits
    mask|=3 if c['ramcfg_11_07_08'] else 0x14000000
    M(0x10f824,mask,data,780);M(0x132040,0x10000,0,782)
    M(0x1373f4,0,0x10010,784)
    W(0x1373ec,(s.read(0x1373ec)&~0x30000)|(c['ramcfg_11_03_30']<<16),787)
    M(0x1373f4,3,0,788);M(0x1373f4,0x10,0,789)
    condition=different(s.read(0x132024),value(rcoef))
    if condition is not True:
        c2=different(s.read(0x132034)&0xffff,value(fn))
        condition=True if c2 is True else c2 if condition is False else f'({condition}) || ({c2})'
    guard=s.guard;s.guard=conjunction(guard,condition)
    M(0x132000,1,0,794);M(0x132020,1,0,795);W(0x137320,0,796)
    M(0x132030,0xffff0000,fn<<16,797);M(0x132034,0xffff,fn,798)
    W(0x132024,rcoef,799);M(0x132028,0x80000,0x80000,800)
    M(0x132020,1,1,801);s.command('WAIT',[0x137390,0x20000,0x20000,64000],src(802))
    M(0x132028,0x80000,0,803);s.guard=guard
    M(0x1373f4,0x10,0x10,806);M(0x1373f4,3,1,807);M(0x1373f4,0x10000,0,808)
    gpio_set(s,gpios,0x18,int(mv),811)
    if c['ramcfg_11_02_40'] or c['ramcfg_11_07_10']:
        M(0x132040,0x10000,0x10000,820);s.nsec(20000,821)
    if mode!=2 and c['ramcfg_11_07_40']:M(0x10f670,0x80000000,0x80000000,826)
    W(0x10f65c,0x11*c['rammap_11_11_0c'],829)
    W(0x10f6b8,0x1010101*c['ramcfg_11_09'],830);W(0x10f6bc,0x1010101*c['ramcfg_11_09'],831)
    data=0
    for field,bits in [('ramcfg_11_02_80',0x3000000),('ramcfg_11_02_40',0x2000),('ramcfg_11_07_10',0x4000)]:
        if not c[field]:data|=bits
    data|=0x14000000 if c['ramcfg_11_07_08'] else 3
    M(0x10f824,0x10000,data,845);s.nsec(1000,846)
    M(0x10f82c,0x100000,0x100000 if c['ramcfg_11_08_01'] else 0,852)
    for i,a in enumerate([0x10f248,0x10f290,0x10f294,0x10f298,0x10f29c,0x10f2a0,0x10f2a4,0x10f2a8,0x10f2ac,0x10f2cc,0x10f2e8]):
        M(a,U32,c['timing'][(i-1)%11],855+i)
    mask=0x33f00000;data=0
    if not c['ramcfg_11_01_04']:data|=0x20200000
    if not c['ramcfg_11_07_80']:data|=0x12800000
    if c['ramcfg_11_03_f0']:
        if c['rammap_11_08_0c']:
            if not c['ramcfg_11_07_80']:mask|=0x20
            else:data|=0x20
            mask|=0x08000004
        data|=0x04000000
    else:mask|=0x44000020;data|=0x08000004
    M(0x10f808,mask,data,890);W(0x10f870,0x11111111*c['ramcfg_11_03_0f'],892)
    M(0x10f250,0x3f0,c['timing_20_2c_003f']<<4,894)
    data=max((c['timing'][10]&0x7f000000)>>24,c['timing_20_2c_1fc0'])
    M(0x10f24c,0x7f000000,data<<24,899)
    M(0x10f224,0x1f0000,c['timing_20_30_f8']<<16,901)
    W(0x10f090,0x4000007f,903);s.nsec(1000,904)
    W(0x10f314,1,906);W(0x10f310,1,907);W(0x10f210,0x80000000,908);s.nsec(1000,909)
    if not c['ramcfg_DLLoff']:M(0x10f304,1,0,912);dll_reset(s)
    M(0x10f320,0xfff,mr[2],916);M(0x10f304,U32,mr[1],917);W(0x10f300,mr[0],918);s.nsec(1000,919)
    if not c['ramcfg_DLLoff']:dll_reset(s);s.nsec(1000,923)
    if not vc:gpio_set(s,gpios,0x2e,0,927)
    if mode!=2:M(0x10f830,0x1000000,0x1000000,935);M(0x10f830,0x1000000,0,936)
    M(0x10f200,0x80000000,0x80000000,939);W(0x10f318,1,940)
    M(0x10f200,0x80000000,0,941);s.nsec(1000,942)
    s.command('LEAVE',[],src(944))
    if display:W(0x62c000,0x0f0f0f00,947)
    M(0x10f200,0x800,c['rammap_11_08_01']<<11,953)
    return [v.json() for v in mr]

PROG_FIELDS = [
    (0x10f468, [('rammap_11_0a_03fe',0x1ff000,12),('rammap_11_09_01ff',0x1ff,0)],1179),
    (0x10f420, [('rammap_11_0a_0400',1,0)],1185),
    (0x10f430, [('rammap_11_0a_0800',1,0)],1191),
    (0x10f400, [('rammap_11_0b_01f0',0x1f,0)],1197),
    (0x10f410, [('rammap_11_0b_0200',0x200,9)],1203),
    (0x10f440, [('rammap_11_0d',0xff0000,16),('rammap_11_0f',0xff00,8)],1213),
    (0x10f444, [('rammap_11_0e',0xff00,8),('rammap_11_0b_0800',0x80,7),('rammap_11_0b_0400',0x20,5)],1227),
]


def prog0(s,decoded,freq):
    config=select(decoded,freq)
    start=len(s.ops)
    for a,fs,line in PROG_FIELDS:
        mask=data=0
        for field,bits,shift in fs:
            if len({c[field] for c in decoded['configs']})>1:
                mask|=bits;data|=config[field]<<shift
        # core/device.h:121-126 writes even when mask=0 or the result is unchanged.
        s.mask(a,mask,data,src(line),cpu=True)
    ops=s.ops[start:];del s.ops[start:]
    return ops


OPCODES={'ENTER':1,'LEAVE':2,'WR32':3,'WAIT':4,'DELAY':5,'VBLANK':6,'TRAIN':7}


def pack(ops):
    """pmu/memx.c:19-42,110-140: <64-word cache, forced WAIT/DELAY flush."""
    words=[];current=None;cache=[]
    def flush():
        nonlocal current,cache
        if current is not None:words.extend([(len(cache)<<16)|OPCODES[current],*cache])
        current=None;cache=[]
    for op,args in ops:
        if len(cache)+len(args)>=64 or current is not None and current!=op:flush()
        current=op;cache.extend(args)
        if op in ('WAIT','DELAY','VBLANK'):flush()
    flush()
    return words


def unpack(words):
    result=[];p=0;names={v:k for k,v in OPCODES.items()}
    size={'ENTER':0,'LEAVE':0,'WR32':2,'WAIT':4,'DELAY':1,'VBLANK':1,'TRAIN':0}
    while p<len(words):
        header=words[p];p+=1;op=names[header&0xffff];n=header>>16
        need(p+n<=len(words),'truncated memx packet')
        data=words[p:p+n];p+=n;stride=size[op]
        if stride:
            need(n>0 and n%stride==0,'invalid memx packet length')
            result.extend((op,data[i:i+stride]) for i in range(0,n,stride))
        else:
            need(n==0,'invalid zero-argument packet');result.append((op,[]))
    return result


def generate(fixture,former_khz=405000,target_khz=900000):
    d=decode(fixture)
    lo,hi=select(d,former_khz),select(d,target_khz)
    xits=copy.deepcopy(hi if target_khz<former_khz else lo)
    other=lo if target_khz<former_khz else hi
    for field in ('ramcfg_11_02_04','ramcfg_11_02_03','timing_20_30_07'):xits[field]=other[field]
    phases=[]
    if xits!=lo:
        phases.append((min(former_khz,target_khz),xits,'xition'))
    phases.append((target_khz,hi,'target'))
    initial={int(a,0):int(v,0) if isinstance(v,str) else v for a,v in fixture['registers'].items()}
    s=Script(initial);out=[]
    for freq,c,label in phases:
        mode=2 if freq>d['refpll']['max_freq'] else 1
        # C5a is the one-PLL DDR3 slice. No invented second-PLL programming.
        need(mode==1,'offline slice requires mode 1; mode 2 needs separate review of the DDR3 gap')
        coeff=pll(d['refpll'],freq)
        before=prog0(s,d,1000)
        start=len(s.ops);mr=ddr3(s,c,coeff,mode,d['gpio'],fixture['display_present'])
        ops=s.ops[start:]
        after=prog0(s,d,freq)
        exact=all(o['emitted'] is not None and all(isinstance(a,int) for a in o['args']) for o in ops)
        words=pack([(o['op'],o['args']) for o in ops if o['emitted']]) if exact else None
        out.append(dict(label=label,frequency_khz=freq,config=c,mode=mode,coefficients=dict(zip(('M','N','P','fN'),coeff)),
                        calculated_mr=mr,cpu_before=before,ops=ops,cpu_after=after,words=words))
    exact=not s.missing and all(p['words'] is not None for p in out)
    return dict(source_revision=REVISION,decoded=d,phases=out,exact=exact,
                missing_registers=sorted(s.missing),words=[w for p in out for w in p['words']] if exact else None,
                final_registers={f'0x{a:06x}':v.json() for a,v in sorted(s.state.items())})


def hexword(v):return f'{v:#010x}' if isinstance(v,int) else str(v)


def review(plan):
    lines=['# C5a ordered offline DDR3 review trace','',
           'Source: MIT nouveau Linux v6.8 '+REVISION,
           'STATUS: '+('EXACT HOST MODEL' if plan['exact'] else 'UNRESOLVED CAPTURE INPUTS; NO EXECUTABLE WORDS'),'',
           'WR32 mask/data are ramfuc inputs; result is the full PMU write payload.',
           'tNNN names that row\'s result. R[addr] is missing initial state.',
           'A symbolic guard or emission test is unresolved, not assumed true.',
           'CPU_WR32 rows are gk104_ram_prog_0 host writes, outside memx.',
           'No VBLANK or TRAIN instruction is called on this DDR3 path.','',
           'Missing words: '+', '.join(f'{a:#08x}' for a in plan['missing_registers'])]
    for phase in plan['phases']:
        c=phase['config'];lines.extend(['',f"## {phase['label']}: {phase['frequency_khz']} kHz, mode {phase['mode']}",
            f"RAMMAP {c['rammap_offset']:#x}, RAMCFG {c['ramcfg_offset']:#x}, timing {c['timing_index']} at {c['timing_offset']:#x}",
            'PLL: '+json.dumps(phase['coefficients']),
            'Calculated MR0/1/2: '+', '.join(hexword(v) for v in phase['calculated_mr'])])
        for label,ops in [('CPU before EXEC',phase['cpu_before']),('MEMX script',phase['ops']),('CPU after EXEC',phase['cpu_after'])]:
            lines.extend(['',label])
            for i,o in enumerate(ops,1):
                flag='emit' if o['emitted'] is True else 'elide' if o['emitted'] is False else 'conditional'
                if 'register' in o:
                    text=f"{o['symbol']} {o['op']} reg={o['register']:#08x} mask={o['mask']:#010x} data={hexword(o['data'])} result={hexword(o['result'])}"
                else:text=o['op']+' '+', '.join(hexword(a) for a in o['args'])
                lines.append(f"{i:03d} {text} [{flag}; {o['source']}]"+(' FORCE' if o.get('force') else ''))
                if o['guard'] is not True:lines.append('    source guard: '+str(o['guard']))
                if o['emitted'] is None:lines.append('    emit iff: '+str(o['emission_condition']))
        if phase['words'] is not None:
            lines.extend(['','PMU words (headers + payload, each u32 little endian):'])
            lines.extend(f'{i*4:04x}: {w:08x}' for i,w in enumerate(phase['words']))
    return '\n'.join(lines)+'\n'


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('fixture',type=Path,help='sparse table/snapshot JSON')
    parser.add_argument('--registers',type=Path,help='explicit replacement snapshot JSON (no defaults)')
    parser.add_argument('--output',type=Path,help='ordered text review')
    parser.add_argument('--json',type=Path,help='decoded tables and full trace JSON')
    args=parser.parse_args();fixture=json.loads(args.fixture.read_text())
    if args.registers:fixture['registers']=json.loads(args.registers.read_text())
    plan=generate(fixture);text=review(plan)
    if args.output:args.output.write_text(text)
    else:print(text,end='')
    if args.json:args.json.write_text(json.dumps(plan,indent=2)+'\n')


if __name__=='__main__':main()
