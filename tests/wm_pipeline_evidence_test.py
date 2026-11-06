"""Failure pipeline evidence must survive the suite child's discarded stdout."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item
ROOT = Path(__file__).resolve().parents[1]
class PipelineEvidence(unittest.TestCase):
    def test_failure_trace_reaches_evidence_with_stdout_discarded(self):
        source=(ROOT/'userspace/programs/src/wm_suite/cases.rs').read_text()
        if 'fn failure_pipeline(' in source:
            body=item(source,'fn failure_pipeline(')
            if 'fn export_pipeline(' in source:
                body+='\n'+item(source,'fn export_pipeline(')
        else:
            body='fn failure_pipeline() { '+item(source,'if let Ok(trace) = std::fs::read_to_string("/proc/breenix/window-pipeline")')+' }'
        with tempfile.TemporaryDirectory(prefix='wm-pipeline-') as directory:
            p=Path(directory); trace=p/'trace'; evidence=p/'evidence'; trace.write_text('1 2 3 4 5\n');evidence.touch()
            body=body.replace('"/proc/breenix/window-pipeline"',f'"{trace}"').replace('"/dev/screen-evidence"',f'"{evidence}"')
            (p/'check.rs').write_text('fn value(_: &str, _: i64, _: &str, _: Option<(i64,i64)>) {panic!("unexpected export error");}\n'+body+'\nfn main(){failure_pipeline();}')
            compiler=(['taskpolicy','-b'] if sys.platform=='darwin' else [])+['rustc','--edition=2021','-Dwarnings']
            subprocess.run(compiler+[str(p/'check.rs'),'-o',str(p/'check')],check=True)
            with open('/dev/null','w') as sink:subprocess.run([str(p/'check')],stdout=sink,stderr=sink,check=True)
            self.assertEqual(evidence.read_text(),'VALUE window-pipeline 1 2 3 4 5\n')
if __name__=='__main__':unittest.main()
