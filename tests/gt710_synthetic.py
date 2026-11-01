"""Synthetic BIOS layouts for DDR3 tests; generated fields, no captured ROM.

Layout references: MIT nouveau v6.8 bios/{bit,M0203,rammap,timing,pll,gpio}.c.
Offsets, flags, timing words and GPIO lines below are chosen for the tests.
"""
import importlib.util
from pathlib import Path
import sys
ROOT=Path(__file__).resolve().parents[1]
RAMMAP=0x1000
TARGET_CFG_FLAG=21+3*(18+2*10)+18+10+1

def fixture(complete=False):
 parts={}
 def table(p,data):parts[hex(p)]=bytearray(data);return parts[hex(p)]
 def put(b,p,n,v):b[p:p+n]=v.to_bytes(n,'little')
 bit=table(0x40,b'\xff\xb8BIT\0'+bytes([0,1,12,6,3,0])+bytes(18))
 bit[11]=-sum(bit[:12])&255
 for i,(tag,ver,length,p) in enumerate([(ord('M'),2,7,0x100),(ord('P'),2,12,0x120),(ord('C'),2,4,0x140)]):
  q=12+i*6;bit[q:q+2]=bytes([tag,ver]);put(bit,q+2,2,length);put(bit,q+4,2,p)
 table(0x100,bytes([2,0,0,0,3,0,0]))
 table(0x300,bytes([0x10,7,2,1,0,0,0,0x11,1]))
 p=table(0x120,bytes(12));put(p,4,4,RAMMAP);put(p,8,4,0x2000)
 p=table(0x140,bytes(4));put(p,0,4,0x3000)
 # v11: inclusive overlapping endpoint exercises first-match selection.
 mm=table(RAMMAP,bytes(21+4*(18+20)));mm[:6]=bytes([0x11,21,18,10,2,4])
 for i,(lo,hi,ti) in enumerate([(0,162,0),(163,460,1),(461,750,2),(750,1200,2)]):
  q=21+i*38;put(mm,q,2,lo);put(mm,q+2,2,hi)
  mm[q+8]=1;put(mm,q+9,4,0x10201+i);mm[q+13:q+16]=bytes([i+1,i+2,i+3]);mm[q+17]=(i&3)<<2
  for group in range(2):
   c=q+18+group*10;mm[c:c+10]=bytes([ti,0x14,0 if i<2 else 0x44,0x11,0,0,0,0x80 if i<2 else 0x10,1,i+1])
 timing=table(0x2000,bytes(6+3*51));timing[:6]=bytes([0x20,6,51,0,0,3])
 for i in range(3):
  q=6+i*51
  words=[0x01000000+i, (6+i)+(5+i)*128, (6+2*i)<<16]+[0x1000*j+i for j in range(3,11)]
  for j,v in enumerate(words):put(timing,q+j*4,4,v)
  timing[q+48]=i+1
 pll=table(0x3000,bytes(4+22));pll[:4]=bytes([0x40,4,11,2])
 for i,(kind,p,reg,lo,hi) in enumerate([(0x0c,0x3100,0x132020,600,1200),(4,0x3120,0x132000,1500,3000)]):
  q=4+i*11;pll[q]=kind;put(pll,q+1,2,p);put(pll,q+3,4,reg);put(pll,q+9,2,27)
  d=table(p,bytes(14))
  for j,v in enumerate([lo,hi,25,75]):put(d,j*2,2,v)
  d[8:14]=bytes([1,255,8,255,1,63])
 table(0x36,(0x4000).to_bytes(2,'little'))
 dcb=table(0x4000,bytes(12));dcb[0]=0x40;put(dcb,10,2,0x4100)
 gpio=table(0x4100,bytes([0x41,4,2,5])+bytes(10))
 for i,(line,tag) in enumerate([(3,0x18),(4,0x2e)]):put(gpio,4+i*5,4,line|(tag<<8));gpio[8+i*5]=0x40
 result=dict(bit_offset=0x40,segments={k:v.hex() for k,v in parts.items()},
             registers={'0x101000':hex(0x400004)},display_present=True)
 if complete:
  spec=importlib.util.spec_from_file_location('synthetic_memx',ROOT/'scripts/metal/gt710_memx.py')
  api=importlib.util.module_from_spec(spec);sys.modules[spec.name]=api;spec.loader.exec_module(api)
  for a in api.generate(result)['missing_registers']:result['registers'][hex(a)]=0
 return result
