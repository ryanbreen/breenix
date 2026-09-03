#!/bin/bash
# #693 investigation patch, revert. Undoes x86-launch-site-apply.sh, returning
# kernel/src/main.rs's executable code to the branch's bytes (which are
# byte-identical to main @ 3d601400 for this file; only the comment differs).
set -e
cd "$(git rev-parse --show-toplevel)"
git apply -R --check docs/planning/green-program/sockets/serials/693-fix/x86-launch-site.patch
git apply -R docs/planning/green-program/sockets/serials/693-fix/x86-launch-site.patch
echo "reverted: poll_tcp_oracle off the x86 RING3_SMOKE roster"
git diff --stat kernel/src/main.rs
