"""Check the noisy filesystem success records and their path formatting."""
from pathlib import Path
import re
source = (Path(__file__).resolve().parents[1] / 'kernel/src/syscall/fs.rs').read_text()
assert 'log::info!' not in source, 'per-call filesystem success logs still reach the screen'
for call in re.findall(r'log::\w+!\((.*?)\);', source, re.S):
    if any(name in call for name in ('sys_link:', 'sys_symlink:', 'sys_readlink:')):
        assert '{:?}' not in call, 'path printed as a byte array: ' + call
print('filesystem success logs are debug; link paths are text')
