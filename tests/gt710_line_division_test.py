"""A native loop prefix optimization preserves the 96-bit division result."""
from pathlib import Path
import importlib.util
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class LineDivision(unittest.TestCase):
    def test_small_dividends_skip_only_zero_quotient_prefixes(self):
        source = r'''
#include <array>
#include <cassert>
#include <cstdint>
#include "HEADER"
struct Ops {
 using V=uint32_t;
 V imm(V x){return x;}V or_(V a,V b){return a|b;}V and_(V a,V b){return a&b;}
 V add(V a,V b){return a+b;}V eqzero(V a){return a?0:~0u;}V nonzero(V a){return a?~0u:0;}
 V highest(V a){assert(a);return 31-__builtin_clz(a);}V select(V c,V a,V b){return c?a:b;}
};
using Wide=__uint128_t;
Wide value(std::array<uint32_t,3> a){return Wide(a[0])|(Wide(a[1])<<32)|(Wide(a[2])<<64);}
Wide divide(Wide a,Wide b,unsigned count){Wide q=0,r=0;while(count--){r=(r<<1)|((a>>count)&1);if(r>=b){r-=b;q|=Wide(1)<<count;}}return q;}
int main(){Ops ops;uint64_t rng=0x1c05eed;
 for(unsigned i=0;i<20000;++i){
  auto next=[&](){rng^=rng<<13;rng^=rng>>7;rng^=rng<<17;return uint32_t(rng);};
  std::array<uint32_t,3>a={next(),next(),next()},b={next(),next(),next()};
  if(i%2==0)a[1]=a[2]=0;if(i%3==0)b[1]=b[2]=0;if(i%17==0)b={0,0,0};if(i%29==0)a={0,0,0};
  auto n=kepler_line_division_start(ops,96u,a[0],a[1],a[2],b[0],b[1],b[2],0u);
  assert(n>=1&&n<=96);if(a[1]||a[2]||value(b)==0)assert(n==96);else assert(n<=32);
  assert(divide(value(a),value(b),96)==divide(value(a),value(b),n));
  assert(kepler_line_division_start(ops,97u,a[0],a[1],a[2],b[0],b[1],b[2],1u)==97);
 }
 for(unsigned bit=0;bit<32;++bit)for(uint32_t x:{uint32_t(1)<<bit,(uint32_t(1)<<bit)-1,~0u}){
  auto n=kepler_line_division_start(ops,96u,x,0u,0u,1u,0u,0u,0u);
  assert(divide(x,1,n)==x);
 }
}
'''.replace('HEADER',str(ROOT/'tools/kepler/line_division.h'))
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory(prefix='gt710-line-division-') as directory:
            p=Path(directory);(p/'check.cpp').write_text(source)
            subprocess.run(priority+['c++','-std=c++17','-Wall','-Wextra','-Werror','-O2',str(p/'check.cpp'),'-o',str(p/'check')],check=True)
            subprocess.run([str(p/'check')],check=True)

    def test_optimization_is_pinned_to_the_shared_line_input(self):
        spec=importlib.util.spec_from_file_location('kepler_assets',ROOT/'tools/kepler/assets.py')
        assets=importlib.util.module_from_spec(spec);spec.loader.exec_module(assets)
        path=ROOT/'kernel/src/gpu/virgl/pipelines/10.tgsi'
        self.assertEqual(assets.native_compiler_options(path,{'pipeline':10}),['--line-leading-zero'])
        self.assertEqual(assets.native_compiler_options(path,{'pipeline':11}),[])
        with tempfile.TemporaryDirectory() as directory:
            changed=Path(directory)/'10.tgsi';changed.write_bytes(path.read_bytes()+b'\n')
            with self.assertRaisesRegex(RuntimeError,'line.*source'):
                assets.native_compiler_options(changed,{'pipeline':10})


if __name__ == '__main__':
    unittest.main()
