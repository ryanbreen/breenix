"""Full pipeline records fit a bounded bulk request without UART expansion."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item
ROOT=Path(__file__).resolve().parents[1]


class PipelineBlob(unittest.TestCase):
    def test_fixed_width_rows_preserve_values_within_one_upload(self):
        source=(ROOT/'userspace/programs/src/wm_suite/cases.rs').read_text()
        encoder=item(source,'fn pipeline_blob(')
        main=r'''
fn main(){
 let row="1 2 3 4 18446744073709551615\n";
 let bytes=pipeline_blob(row).unwrap();assert_eq!(&bytes[..8],b"WMPIPE01");
 assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()),1);
 let values:Vec<_>=bytes[12..].chunks_exact(8).map(|b|u64::from_le_bytes(b.try_into().unwrap())).collect();
 assert_eq!(values,[1,2,3,4,u64::MAX]);
 let full=row.repeat(32768);let bytes=pipeline_blob(&full).unwrap();
 assert_eq!(bytes.len(),12+32768*40);assert!(bytes.len()<=2<<20);
 assert!(pipeline_blob(&(full+row)).is_err());
 for bad in ["","1 2 3 4\n","1 2 3 4 5 6\n","1 2 nope 4 5\n"]{assert!(pipeline_blob(bad).is_err());}
}
'''
        with tempfile.TemporaryDirectory(prefix='pipeline-blob-')as directory:
            p=Path(directory);(p/'check.rs').write_text(encoder+'\n'+main)
            compiler=(['taskpolicy','-b']if sys.platform=='darwin'else[])+['rustc','--edition=2021','-Dwarnings']
            subprocess.run(compiler+[str(p/'check.rs'),'-o',str(p/'check')],check=True)
            subprocess.run([str(p/'check')],check=True)


if __name__=='__main__':unittest.main()
