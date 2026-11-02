"""Exercise the stage's production observation-boundary match on the host."""
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
source = (ROOT / 'libs/libbreenix/src/suite_stage/checks.rs').read_text()
start = source.index('match (rec.backend, rec.flags, rec.boundary) {')
end = source.index('\n                }', start) + len('\n                }')
production_match = source[start:end]
fixture = r'''
struct Record { backend: u32, flags: u32, boundary: u32 }
fn accepts(rec: Record) -> Result<(), String> {
    PRODUCTION_MATCH
    Ok(())
}
#[test]
fn observation_boundary_contract() {
    for backend in [4, 5] {
        for flags in [1, 3] {
            assert!(accepts(Record { backend, flags, boundary: 4 }).is_ok());
            assert!(accepts(Record { backend, flags, boundary: 5 }).is_err());
        }
        assert!(accepts(Record { backend, flags: 2, boundary: 4 }).is_err());
    }
    for flags in [1, 3] {
        assert!(accepts(Record { backend: 6, flags, boundary: 5 }).is_ok());
    }
    for (backend, flags, boundary) in [(6, 1, 4), (4, 1, 5), (6, 2, 5), (7, 1, 5)] {
        assert!(accepts(Record { backend, flags, boundary }).is_err());
    }
}
'''.replace('PRODUCTION_MATCH', production_match)
with tempfile.TemporaryDirectory() as directory:
    path = Path(directory)
    (path / 'test.rs').write_text(fixture)
    priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
    subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                               str(path / 'test.rs'), '-o', str(path / 'test')], check=True)
    subprocess.run([str(path / 'test')], check=True, timeout=10)
