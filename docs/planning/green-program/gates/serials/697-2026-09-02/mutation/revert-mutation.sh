#!/bin/bash
set -euo pipefail
cd /root/breenix-697-prove-branch
git checkout -- docker/qemu/run-x86-boot-tests.sh
echo "=== status after revert ==="
git status --porcelain docker/qemu/run-x86-boot-tests.sh
git diff --stat docker/qemu/run-x86-boot-tests.sh
echo "REVERT_CLEAN"
