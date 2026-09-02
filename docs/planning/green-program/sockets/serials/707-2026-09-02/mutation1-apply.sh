#!/bin/bash
set -e
cd "$(git rev-parse --show-toplevel)"
python3 - << 'PYEOF'
p = "kernel/src/ipc/fd.rs"
s = open(p).read()
old = '                            crate::net::tcp::tcp_listener_ref_dec(*port);\n'
assert s.count(old) == 1, "anchor line not found or not unique -- check kernel/src/ipc/fd.rs by hand"
new = '                            let _ = port; // #707 mutation: close_cloexec() no longer decrements TcpListener ref_count\n'
s2 = s.replace(old, new, 1)
open(p, "w").write(s2)
print("mutated")
PYEOF
git diff kernel/src/ipc/fd.rs
