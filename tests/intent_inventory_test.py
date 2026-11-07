"""Recompute U11 panel geometry by executing the production Rust layout."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT/'scripts/metal'))
import intent_manifest as manifest
spec = importlib.util.spec_from_file_location('verify_expect', ROOT/'scripts/screens/verify_expect.py')
v = importlib.util.module_from_spec(spec)
spec.loader.exec_module(v)


class IntentInventoryTest(unittest.TestCase):
    def test_inventory_covers_manifest_pictures_and_counts_pages(self):
        rows = manifest.inventory()
        self.assertEqual(len({(r['suite'],r['case']) for r in rows}),len(rows))
        # Expect also describes end-only pictures; these cases publish no judged handoff.
        ends_only = {
            'gpu': {'isolation/irq-masked-bound','isolation/no-addresses'},
            'breengel': {'composition/present-rate','isolation/texture-map','scanout/window-commits',
                'composition/damage-screen','composition/irq-bound','composition/no-change-commit',
                'composition/inflight-bound','composition/fbinfo-geometry','scanout/root-only',
                'scanout/completion','scanout/boundary-stated','scanout/device-path',
                'composition/composite-record','composition/damage-work','windows/distinct-content','scanout/mode-set'},
        }
        for suite in ('gpu','breengel'):
            doc=json.loads((ROOT/f'docs/suites/{suite}.json').read_text())
            expected={f"{c['id']}/{case['id']}" for c in doc['categories'] for case in c['cases'] if 'expect' in case}
            selected={r['case'] for r in rows if r['suite']==suite}
            self.assertFalse(selected & ends_only[suite])
            self.assertEqual(selected | ends_only[suite],expected)
        # Page inventories are additive; approvals remain separate for each suffix.
        self.assertEqual(manifest.counts(),{'gpu':122,'breengel':165})
        paged={r['case'] for r in rows if len(manifest.row_plans(r))>1}
        self.assertEqual(paged,{'performance/upload-bandwidth','png-decode/interlace-small','png-decode/ancillary-ignored'})

    def test_panel_dimensions_and_cells_match_production_rust(self):
        text=(ROOT/'userspace/programs/src/suite_breengel/images.rs').read_text()
        production=text[text.index('const PANEL_PIXELS'):text.index('fn handoff_images')]
        harness='''
mod checks {
    pub struct Image { pub width:u32, pub height:u32 }
    pub struct JudgedImage { pub actual:Image, pub expected:Image, pub label:String }
}
'''+production+'\nfn main() {\n'
        panels=[]
        for row in manifest.inventory():
            if not row['xs']:
                continue
            fixtures,_=v.panel_fixtures(row['case'])
            panels.append((row,fixtures))
            items=','.join('checks::JudgedImage { actual:checks::Image { width:%d,height:%d }, expected:checks::Image { width:%d,height:%d }, label:%s.into() }' %
                           (f['width'],f['height'],f['width'],f['height'],json.dumps(f['label'])) for f in fixtures)
            harness+='let plan=plan_panel(&['+items+']);\n'
            # Read the geometry and drawing fields so this isolated host build is warning-free.
            harness+='let p=&plan.pages[0]; println!("{} {} {}",plan.target,p.width,p.height);\n'
            harness+='for t in &p.tiles { println!("{} {} {} {} {} {} {} {} {} {}",t.x,t.y,t.width,t.height,t.picture_w,t.picture_h,t.scale,t.divisor,t.title.len(),t.title_height); }\n'
        harness+='}\n'
        with tempfile.TemporaryDirectory() as tmp:
            src=Path(tmp)/'panel.rs'
            exe=Path(tmp)/'panel'
            src.write_text(harness)
            subprocess.run(['taskpolicy','-b','rustc','--edition=2021','-Dwarnings',str(src),'-o',str(exe)],check=True,capture_output=True,text=True)
            output=iter(subprocess.check_output([str(exe)],text=True).splitlines())
        for row,fixtures in panels:
            _,w,h=map(int,next(output).split())
            self.assertEqual((w,h),row['size'],row['case'])
            size,tiles,_,_=v.panel_plan(fixtures)
            actual=[]
            for tile in tiles:
                values=list(map(int,next(output).split()))
                self.assertEqual(values[:8],[tile[k] for k in ('x','y','width','height','picture_w','picture_h','scale','divisor')],row['case'])
                actual.append(values)
            self.assertEqual(row['xs'],sorted({t[0] for t in actual}|{w}))
            self.assertEqual(row['ys'],sorted({t[1] for t in actual}|{h}))
        self.assertEqual(list(output),[])


if __name__ == '__main__':
    unittest.main()
