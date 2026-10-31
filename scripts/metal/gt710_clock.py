# /*
#  * Copyright 2012 Nouveau Community
#  * Copyright 2011 Red Hat Inc.
#  * Copyright 2012 Red Hat Inc.
#  * Copyright 2013 Red Hat Inc.
#  * Copyright 2014 Martin Peres
#  * Copyright 2015 Martin Peres
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
# claim-lint:ok: MIT grant in docs/gui/gt710-c1.md.
#  *
#  * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
#  * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
#  * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
#  * THE COPYRIGHT HOLDER(S) OR AUTHOR(S) BE LIABLE FOR ANY CLAIM, DAMAGES OR
#  * OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
#  * ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
#  * OTHER DEALINGS IN THE SOFTWARE.
#  *
#  * Authors: Martin Peres
#  */
# claim-lint:ok: original MIT grant preserved under G1.
"""Offline C1 VBIOS and readback decoder. Raw ROMs stay in private captures.

Source offsets and versions: docs/gui/gt710-c1.md. Bounds failures are errors;
absent optional tables are explicit. No table data is executed by this tool.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re


def need(ok, why):
    if not ok: raise ValueError(why)


class Image:
    def __init__(self, raw): self.raw = raw
    def bytes(self, off, size):
        need(type(off) is int and type(size) is int and 0 <= off <= len(self.raw) and 0 <= size <= len(self.raw)-off,'VBIOS range outside image')
        return self.raw[off:off+size]
    def n(self, off, size=1, signed=False): return int.from_bytes(self.bytes(off,size),'little',signed=signed)
    def table(self, off, versions, min_header=4):
        h=self.bytes(off,4); v,hs,es,count=h
        need(v in versions,f'unsupported table version {v:#x} at {off:#x}')
        need(hs>=min_header and (es>0 or count==0),'invalid table geometry')
        self.bytes(off,hs+es*count)
        return v,hs,es,count


def decode_image(raw):
    r=Image(raw); need(512 <= len(raw) <= 256*1024,'VBIOS size bound')
    # PCI firmware image chain: retain the entire PROM window but decode tables
    # only in the first legacy image, as nouveau does. Check each image checksum.
    p=0; images=[]
    while True:
        need(r.bytes(p,2)==b'\x55\xaa','ROM signature')
        c=p+r.n(p+0x18,2); need(r.bytes(c,4)==b'PCIR','PCIR signature')
        size=r.n(c+0x10,2)*512; need(size>=512,'empty PCIR image')
        block=r.bytes(p,size); need(sum(block)&255==0,'ROM checksum')
        images.append(dict(offset=p,bytes=size,code_type=r.n(c+0x14)))
        p+=size
        if r.n(c+0x15)&0x80: break
        need(len(images)<16,'ROM image-chain bound')
    first=Image(raw[:images[0]['bytes']])
    bit=first.raw.find(b'\xff\xb8BIT\0'); need(bit>=0,'BIT directory missing')
    h=first.bytes(bit,12); need(h[8]>=12 and h[9]==6,'BIT geometry')
    entries=first.bytes(bit+h[8],h[9]*h[10]); selected=[e for e in [entries[i:i+6] for i in range(0,len(entries),6)] if e[0]==80]
    need(len(selected)==1,'BIT P absent or duplicated'); e=selected[0]
    pv=e[1]; need(pv in (1,2),'BIT P version')
    payload=first.bytes(int.from_bytes(e[4:6],'little'),int.from_bytes(e[2:4],'little'))
    def ptr(v1,v2):
        f=v2 if pv==2 else v1
        return int.from_bytes(payload[f:f+4],'little') if f is not None and f+4<=len(payload) else 0
    result=dict(sha256=hashlib.sha256(raw).hexdigest(),bytes=len(raw),image_bytes=p,images=images,bit_version=pv)
    def optional(name,off,fn):
        result[name]=fn(first,off) if off else dict(status='absent')
    optional('performance',ptr(0,0),performance)
    optional('cstep',ptr(None,0x34),cstep)
    optional('boost',ptr(None,0x30),boost)
    optional('voltage',ptr(0x10,0x0c),voltage)
    optional('voltage_map',ptr(None,0x20),vmap)
    optional('thermal',ptr(0x0c,0x10),thermal)
    optional('timing',ptr(4,8),timing)
    optional('rammap',ptr(None,4),rammap)
    result['gpio']=gpio(first)
    return result


def geometry(r,p,version,min_base):
    v,h,b,s,n,c=r.bytes(p,6)
    need(v==version and h>=6 and b>=min_base and (s>0 or n==0),'invalid nested table geometry')
    r.bytes(p,h+c*(b+s*n))
    return h,b,s,n,c


def performance(r,p):
    h,b,s,n,c=geometry(r,p,0x40,12); need(s>=2 and n>=4,'performance domains missing')
    levels=[]
    for i in range(c):
        q=p+h+i*(b+s*n); state=r.n(q)
        if state==255: continue
        need(state not in [e['pstate'] for e in levels],'duplicate performance state')
        f=[(r.n(q+b+j*s,2)&0x3fff)*1000 for j in range(n)]
        levels.append(dict(pstate=state,voltage_id=r.n(q+2),gpc_khz=f[0],core_khz=f[0]//2,memory_khz=f[3],domain_khz=f,pcie_speed_code=r.n(q+11)&3))
    return dict(status='ok',version=0x40,offset=p,levels=levels)


def cstep(r,p):
    v,h,b,c,s,n=r.bytes(p,6)
    need(v==0x10 and h>=6 and b>=4 and s>=5,'invalid cstep geometry'); r.bytes(p,h+b*c+s*n)
    levels=[dict(pstate=(r.n(p+h+i*b,2)&0x1e0)>>5,index=r.n(p+h+i*b+3)) for i in range(c)]
    points=[dict(frequency_khz=r.n(p+h+c*b+i*s,2)*1000,unknown=list(r.bytes(p+h+c*b+i*s+2,2)),voltage_id=r.n(p+h+c*b+i*s+4)) for i in range(n)]
    return dict(status='ok',version=v,offset=p,levels=levels,points=points)


def boost(r,p):
    h,b,s,n,c=geometry(r,p,0x11,6); need(s>=6 or n==0,'boost subentry short')
    levels=[]
    for i in range(c):
        q=p+h+i*(b+s*n)
        domains=[dict(domain=r.n(q+b+j*s),percent=r.n(q+b+j*s+1),min_khz=r.n(q+b+j*s+2,2)*1000,max_khz=r.n(q+b+j*s+4,2)*1000) for j in range(n)]
        levels.append(dict(pstate=(r.n(q,2)&0x1e0)>>5,min_khz=r.n(q+2,2)*1000,max_khz=r.n(q+4,2)*1000,domains=domains))
    return dict(status='ok',version=0x11,offset=p,levels=levels)


def voltage(r,p):
    v=r.n(p)
    if v==0x12: h,c,s=5,r.n(p+2),r.n(p+1)
    elif v==0x20: h,c,s=r.n(p+1),r.n(p+2),r.n(p+3)
    else: v,h,s,c=r.table(p,(0x30,0x40,0x50))
    minimum={0x12:5,0x20:6,0x30:5,0x40:18,0x50:26}[v]
    need(h>=minimum and (s>0 or c==0),'voltage header short');r.bytes(p,h+c*s)
    out=dict(status='ok',version=v,offset=p,type='gpio',entries=[])
    if v in (0x12,0x20,0x30):
        need(s>=2,'voltage entry short');out['vidmask']=r.n(p+(5 if v==0x20 else 4))
        out['entries']=[dict(vid=r.n(p+h+i*s+1)>>(2 if v==0x30 else 0),uv=r.n(p+h+i*s)*10000) for i in range(c)]
    else:
        out.update(min_uv=r.n(p+0x0a,4) if v==0x50 else 0,max_uv=r.n(p+0x0e,4),base_uv=r.n(p+(0x12 if v==0x50 else 4),4)&(0xffffff if v==0x50 else 0xffffffff))
        if v==0x50 and r.n(p+4)&1:
            out.update(type='pwm',pwm_frequency_hz=r.n(p+5,4)//1000,pwm_range_uv=r.n(p+0x16,4))
        else:
            mask=r.n(p+(6 if v==0x50 else 11));step=r.n(p+(0x16 if v==0x50 else 8),2,signed=True)
            out.update(vidmask=mask,step_uv=step)
            if v==0x40 or r.n(p+4)&2:
                if v==0x40:
                    out['min_uv']=min(out['base_uv'],out['base_uv']+step*mask)
                    out['max_uv']=out['max_uv'] or max(out['base_uv'],out['base_uv']+step*mask)
                out['entries']=[dict(vid=i,uv=out['base_uv']+step*i) for i in range(mask+1) if out['min_uv']<=out['base_uv']+step*i<=out['max_uv']]
            else:
                need(s>=4,'voltage entry short')
                out['entries']=[dict(vid=(r.n(p+h+i*s,4)>>23)&255,uv=r.n(p+h+i*s,4)&0x1fffff) for i in range(c)]
    return out


def vmap(r,p):
    v,h,s,c=r.table(p,(0x10,0x20),4)
    need(s >= (20 if v==0x10 else 34),'voltage map entry short')
    rows=[]
    for i in range(c):
        q=p+h+i*s; start=0 if v==0x10 else 2
        rows.append(dict(mode=0 if v==0x10 else r.n(q),link=255 if v==0x10 else r.n(q+1),min_uv=r.n(q+start,4),max_uv=r.n(q+start+4,4),args=[r.n(q+start+8+j*4,4,signed=True) for j in range(3 if v==0x10 else 6)]))
    return dict(status='ok',version=v,offset=p,entries=rows)


def thermal(r,p):
    v,h,s,c=r.table(p,(0x10,0x20));need(s>=3,'thermal entry short')
    names={4:'critical',7:'down_clock',8:'fan_boost',0x32:'shutdown'};thresholds={};rows=[];section=0
    for i in range(c):
        q=p+h+i*s;tag=r.n(q);value=r.n(q+1,2);rows.append(dict(tag=tag,value=value))
        if tag==0: section=value
        if section==0 and tag in names:thresholds[names[tag]]=dict(c=(value&0xff0)>>4,hysteresis=value&15)
    return dict(status='ok',version=v,offset=p,thresholds=thresholds,entries=rows)


def timing(r,p):
    v=r.n(p)
    if v==0x10:
        v,h,c,b=r.bytes(p,4);s=n=0;need(h>=4 and b>=15,'timing v10 geometry');r.bytes(p,h+c*b)
    else:h,b,s,n,c=geometry(r,p,0x20,0x33)
    rows=[]
    for i in range(c):
        q=p+h+i*(b+s*n);entry=dict(raw=r.bytes(q,b+s*n).hex())
        if v==0x20:
            fields={}
            for label,offset,mask,shift in [('2c_003f',0x2c,0x3f,0),('2c_1fc0',0x2c,0x1fc0,6),('2e_03',0x2e,3,0),('2e_30',0x2e,0x30,4),('2e_c0',0x2e,0xc0,6),('2f_03',0x2f,3,0),('30_07',0x30,7,0),('30_f8',0x30,0xf8,3),('31_0007',0x31,7,0),('31_0078',0x31,0x78,3),('31_0780',0x31,0x780,7),('31_0800',0x31,0x800,11),('31_7000',0x31,0x7000,12),('31_8000',0x31,0x8000,15)]:fields[label]=(r.n(q+offset,2 if offset in (0x2c,0x31) else 1)&mask)>>shift
            entry.update(registers=[r.n(q+j*4,4) for j in range(11)],fields=fields,subentries=[r.bytes(q+b+j*s,s).hex() for j in range(n)])
        else:
            entry['fields']={k:r.n(q+o) for k,o in [('WR',0),('WTR',1),('CL',2),('RC',3),('RFC',5),('RAS',7),('RP',9),('RCDRD',10),('RCDWR',11),('RRD',12)]}
        rows.append(entry)
    return dict(status='ok',version=v,offset=p,entries=rows)


def rammap(r,p):
    v=r.n(p);need(v in (0x10,0x11),'rammap version');h,b,s,n,c=geometry(r,p,v,5 if v==0x10 else 0x12)
    rows=[]
    for i in range(c):
        q=p+h+i*(b+s*n); rows.append(dict(min_mhz=r.n(q,2),max_mhz=r.n(q+2,2),raw=r.bytes(q,b).hex(),configurations=[dict(raw=r.bytes(q+b+j*s,s).hex(),timing_index=r.n(q+b+j*s+(1 if v==0x10 else 0))) for j in range(n)]))
    return dict(status='ok',version=v,offset=p,entries=rows)


def gpio(r):
    p=r.n(0x36,2)
    if not p:return dict(status='absent',entries=[])
    v,h=r.bytes(p,2);need(0x30<=v<=0x41 and h>=12,'DCB GPIO header');p=r.n(p+10,2)
    if not p:return dict(status='absent',entries=[])
    v,h,c,s=r.bytes(p,4);need(0x30<=v<=0x41 and h>=4 and s>=(5 if v==0x41 else 4),'GPIO geometry');r.bytes(p,h+c*s)
    rows=[]
    for i in range(c):
        q=p+h+i*s;info=r.n(q,4)
        logs=r.n(q+4) if v==0x41 else info>>23
        rows.append(dict(line=info&(63 if v==0x41 else 31),function=(info>>8)&255,log0=(logs>>4)&3,log1=(logs>>6)&3))
    return dict(status='ok',version=v,entries=rows)


def voltage_state(d,raw):
    v=d['voltage'];now=None;reason='voltage-table-absent'
    if v['status']=='ok':
        if v['type']=='pwm':
            if any(g['function']==0x81 for g in d['gpio']['entries']):
                divisor=raw.get(0x20340,0);duty=raw.get(0x20344,0)&0x7fffffff
                if divisor:now=v['base_uv']+v['pwm_range_uv']*duty//divisor
                reason='pwm-divisor-zero' if not divisor else 'pwm-readback'
            else:reason='pwm-gpio-absent'
        else:
            vid=0;valid=True
            for i,tag in enumerate([4,5,6,0x1a,0x73,0x74,0x75,0x76]):
                if not v['vidmask']&(1<<i):continue
                pins=[g for g in d['gpio']['entries'] if g['function']==tag]
                # Missing BIOS tags are masked by nouveau volt/gpio.c:init.
                if not pins:continue
                g=pins[0];a=0xd610+4*g['line']
                if a not in raw:valid=False;break
                vid|=int(bool(raw[a]&0x4000)==bool(g['log1']&1))<<i
            matches=[e['uv'] for e in v['entries'] if e['vid']==vid]
            if valid and len(matches)==1:now=matches[0]
            reason='gpio-readback' if now is not None else 'gpio-vid-unresolved'
    vm=d['voltage_map'];levels=[]
    def bounds(idx,seen):
        if vm['status']!='ok' or idx>=len(vm['entries']):return (idx*10000,idx*10000) if idx else (None,None)
        need(idx not in seen,'cyclic voltage map');e=vm['entries'][idx];lo,hi=e['min_uv'],e['max_uv']
        if e['link']!=255:
            a,b=bounds(e['link'],seen|{idx});return (None,None) if a is None else (lo+a,hi+b)
        return lo,hi
    for l in d['performance'].get('levels',[]):
        lo,hi=bounds(l['voltage_id'],set())
        more=None if now is None or lo is None else True if lo>now else False if hi<=now else None
        levels.append(dict(pstate=l['pstate'],voltage_id=l['voltage_id'],min_uv=lo,max_uv=hi,needs_more_voltage=more))
    points=[]
    for i,point in enumerate(d['cstep'].get('points',[])):
        lo,hi=bounds(point['voltage_id'],set())
        points.append(dict(index=i,frequency_khz=point['frequency_khz'],voltage_id=point['voltage_id'],min_uv=lo,max_uv=hi))
    return dict(method=v.get('type'),current_uv=now,status=reason,levels=levels,boost_points=points)


def benchmarks(rows):
    expected=[('core',1),('core',2),('memory',1),('memory',2)]
    need([(r.get('kind'),r.get('trial')) for r in rows]==expected,'four ordered benchmark trials required')
    for r in rows:
        need(r.get('completion') is True,'benchmark completion missing')
        need(all(type(r.get(k)) is int and 0<=r[k]<1<<64 for k in ('begin_ns','end_ns','elapsed_ns','work')),'benchmark integer fields')
        need(0<r['elapsed_ns']==r['end_ns']-r['begin_ns']<2_000_000_000,'benchmark PTIMER interval')
        work,unit=(16*512*512,'fragments') if r['kind']=='core' else (64<<20,'bytes')
        need(r['work']==work and r.get('unit')==unit,'benchmark work differs')
    return rows


def domains(raw):
    words=dict(raw);need(len(words)==len(raw),'duplicate clock word'); used=set()
    def reg(a):
        need(a in words,'missing clock word '+hex(a));used.add(a);return words[a]
    class Unavailable(Exception): pass
    crystal={0:13500,0x40:14318,0x400000:27000,0x400040:25000}[reg(0x101000)&0x400040]
    def pll(a):
        need(a in (0xe800,0xe820,0x132000,0x132020,0x137000,0x137020,0x137040,0x1370e0),'unrecognized PLL')
        ctrl,coef=reg(a),reg(a+4);m,n,p=coef&255,(coef>>8)&255,(coef>>16)&63;frac=0xf000
        if not ctrl&1:raise Unavailable('pll-disabled')
        if not m:raise Unavailable('zero-pll-divisor')
        if a in (0xe800,0xe820):src,p=crystal,1
        elif a==0x132000:src,p=pll(0x132020),2 if coef&0x10000000 else 1
        elif a==0x132020:src,frac=div(0,0x137320,0x137330),reg(a+16)>>16
        else:src=div((a&255)//32,0x137120,0x137140)
        f=(src*n+((((frac+4096)&65535)*src)>>13))//(m*(p or 1))
        if f>=1<<32:raise Unavailable('clock-overflow')
        return f
    def div(i,src,ctl):
        s,c=reg(src+4*i),reg(ctl+4*i)
        if s&3==0:return 108000 if s&0x30000==0x30000 else crystal
        if s&3==2:return 100000
        if s&3!=3:raise Unavailable('unknown-divider-source')
        f=pll(0xe820 if s&0x100 else 0xe800)
        return f*2//((c&63)+2) if c&0x80000000 else f
    def clk(i):
        c=reg(0x137250+4*i)
        if i<7:
            high=bool(reg(0x137100)&(1<<i));f=pll(0x137000+32*i) if high else div(i,0x137160,0x1371d0)
        else:
            src=reg(0x137160+4*i);f=div(i,0x137160,0x1371d0);high=src&3==3 and bool(src&0x100)
            if high and src&0x40000000:f=pll(0x1370e0)
        return f*2//(((c>>8 if high else c)&63)+2) if c&0x80000000 else f
    def mem():
        mode=reg(0x1373f4)&15
        if mode not in (1,2):raise Unavailable('unknown-memory-mode')
        return pll(0x132020 if mode==1 else 0x132000)
    out={}
    for n,fn in [('crystal',lambda:crystal),('href',lambda:100000)]+[(n,lambda i=i:clk(i)) for n,i in [('gpc',0),('rop',1),('hubk07',2),('hubk06',7),('hubk01',8),('pmu',12),('vdec',14)]]+[('memory',mem),('mpll',lambda:pll(0x132000)),('mpll_ref',lambda:pll(0x132020))]:
        try:out[n]=dict(value=fn(),status='ok')
        except Unavailable as e:out[n]=dict(value=None,status=str(e))
    need(used==set(words),'unexpected clock register')
    return out


def c1_record(row):
    need(row.get('schema')==1 and row.get('phase')=='startup','C1 schema')
    for name in ['raw','voltage_raw']:
        words=row[name]
        need(isinstance(words,list) and 1<=len(words)<=128 and all(isinstance(w,list) and len(w)==2 and all(type(v) is int and 0<=v<1<<32 for v in w) for w in words),'invalid raw words')
        need(len(dict(words))==len(words),'duplicate raw address')
    need(set(dict(row['voltage_raw']))=={0xd610+4*i for i in range(32)}|{0x20340,0x20344},'voltage register inventory')
    need(row['domains']==domains(row['raw']),'domain frequencies differ from hardware')
    need(row['sensor_restored'] is True,'sensor state was not restored')
    reads=row['sensor_raw'];writes=row['sensor_writes']
    allowed={0x22400,0x21000,0x212a8,0x20008,0x2000c,0x20400}
    need(all(isinstance(r,list) and len(r)==2 and r[0] in allowed and type(r[1]) is int and 0<=r[1]<1<<32 for r in reads),'invalid sensor reads')
    first={};last={}
    for a,v in reads:first.setdefault(a,v);last[a]=v
    need({0x22400,0x21000,0x212a8}<=set(first),'sensor gates/fuse absent')
    expected=[]
    for a,mask in [(0x22400,0x800),(0x21000,1)]:
        if not first[a]&mask:expected.append([a,first[a]|mask])
    for a,mask in [(0x21000,1),(0x22400,0x800)]:
        if not first[a]&mask:expected.append([a,first[a]])
    if first[0x212a8]==1:
        need({0x20008,0x2000c,0x20400}<=set(first),'sensor setup/read absent')
        a,b=first[0x20008],first[0x2000c]
        if a&0x80008000!=0x80000000 or b&0x80000003:
            expected += [[0x20008,(a&~0x80008000)|0x80000000],[0x2000c,b&~0x80000003],[0x2000c,b],[0x20008,a]]
        v=first[0x20400];reading=dict(value=v,status='ok') if v<=0x7fffffff else dict(value=None,status='sensor-read-error')
    else:reading=dict(value=None,status='sensor-unavailable')
    need(writes==expected and row['temperature']==reading,'sensor setup or reading differs')
    need(all(last[a]==first[a] for a in [0x22400,0x21000]+([0x20008,0x2000c] if first[0x212a8]==1 else [])),'sensor restore differs')
    ports=row['pcie'];need(len(ports)==2 and ports[0]['bdf']=='01:00.0' and ports[0]['bdf']!=ports[1]['bdf'],'endpoint/upstream PCIe pair')
    for p in ports:
        need(0x40<=p['cap_offset']<=0xdc and p['cap_offset']%4==0,'PCIe capability offset')
        need(all(type(p[k]) is int and 0<=p[k]<1<<32 for k in ['flags','device_cap','link_cap','link_control','link_status']),'PCIe words')
        need(bool(p['flags']&0x100)==(p['slot_cap'] is not None),'PCIe slot presence')
    need((ports[1]['flags']>>4)&15 in (4,6),'upstream is not a root/downstream port')
    return row


def link(p):
    def power(value,scale):return value/[1,10,100,1000][scale]
    d,l,s=p['device_cap'],p['link_cap'],p['link_status'];slot=p['slot_cap']
    return dict(bdf=p['bdf'],maximum_speed_code=l&15,maximum_width=(l>>4)&63,
                speed_code=s&15,width=(s>>4)&63,training=bool(s&0x800),
                device_power_limit_w=power((d>>18)&255,(d>>26)&3),
                slot_power_limit_w=None if slot is None else power((slot>>7)&255,(slot>>15)&3),
                slot_power_controller=None if slot is None else bool(slot&2))


def capture(directory):
    import runpy
    path=Path(directory);boot=json.loads((path/'run.json').read_text())
    need(boot['toolsCommit']==boot['commit']==boot['build']['commit'],'C1 tools/build/head differ')
    need(boot['build']['profile']=='gpu-clock' and boot['build']['features']==['gt710_clock_probe'] and not boot.get('suites') and not boot.get('suite'),'C1 diagnostic profile required')
    api=runpy.run_path(str(Path(__file__).with_name('gt710-suite')),run_name='c1_scorer')
    serial=api['capture_serial'](path)
    def one(prefix):
        found=re.findall('^'+prefix+' (.+)$',serial,re.M);need(len(found)==1,'missing or repeated '+prefix);return json.loads(found[0])
    row=c1_record(one('GT710_CLOCK_C1'));rom=one('GT710_VBIOS_V1');raw=bytes.fromhex(rom['hex'])
    need(len(raw)==rom['bytes'] and hashlib.sha256(raw).hexdigest()==rom['sha256'],'VBIOS digest differs')
    decoded=decode_image(raw)
    times=benchmarks([json.loads(m) for m in re.findall(r'^GT710_BENCH (.+)$',serial,re.M)])
    need('GT710_C1_DONE' in serial,'C1 upload completion absent')
    need(not re.search(r'^GT710_C1_ERROR|^GT710_.*ERROR|^LOCKUP|^PANIC',serial,re.M),'C1/card failure in capture')
    for n,f in row['domains'].items():
        need(f"GT710_CLOCK domain={n} khz={f['value'] if f['value'] is not None else 'null'} status={f['status']}" in serial,'clock serial summary differs')
    need(all(row['domains'][n]['status']=='ok' and row['domains'][n]['value']>0 for n in ['gpc','rop','hubk07','hubk06','hubk01','memory']),'required hardware clock unavailable')
    return dict(boot=boot['id'],commit=boot['commit'],clock=row,benchmarks=times,links=[link(p) for p in row['pcie']],vbios=decoded,voltage=voltage_state(decoded,dict(row['voltage_raw'])))


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--rom',type=Path);p.add_argument('--capture',type=Path);p.add_argument('--output',type=Path)
    a=p.parse_args();need(bool(a.rom)!=bool(a.capture),'choose --rom or --capture')
    report=decode_image(a.rom.read_bytes()) if a.rom else capture(a.capture)
    text=json.dumps(report,indent=2)+'\n'
    if a.output:a.output.write_text(text)
    else:print(text,end='')

if __name__=='__main__': main()
