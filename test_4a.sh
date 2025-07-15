#\!/bin/bash
# Quick test to verify Phase 4A syscall gate
timeout 20 cargo run -p xtask -- build-and-run --features testing 2>&1 | grep -E "SYSCALL_ENTRY|Hello from|TEST_MARKER"
