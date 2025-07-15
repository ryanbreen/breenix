#\!/bin/bash
# Test sys_get_time directly
timeout 15 cargo run -p xtask -- build-and-run --features testing 2>&1 | tail -1000 | grep -E "(RAX=0x4|sys_get_time|Current time|Hello from userspace)" | head -20
