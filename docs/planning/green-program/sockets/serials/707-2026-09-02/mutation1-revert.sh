#!/bin/bash
set -e
cd "$(git rev-parse --show-toplevel)"
python3 - << 'PYEOF'
p = "kernel/src/ipc/fd.rs"
s = open(p).read()
old = '                            let _ = port; // #707 mutation: close_cloexec() no longer decrements TcpListener ref_count\n'
assert s.count(old) == 1, "mutation marker not found -- was the apply step run first?"
new = '                            crate::net::tcp::tcp_listener_ref_dec(*port);\n'
s2 = s.replace(old, new, 1)
open(p, "w").write(s2)
print("reverted")
PYEOF
git diff --stat kernel/src/ipc/fd.rs
