#!/bin/bash
set -euo pipefail
cd /root/breenix-697-prove-branch
git checkout -- docker/qemu/run-x86-boot-tests.sh
sed -i '/^readonly PRODUCTION_REAPED_ROWS$/i\
PRODUCTION_REAPED_ROWS=$(( PRODUCTION_REAPED_ROWS + 1 ))  # MUTATION-697: redden the pin (prove slot)' docker/qemu/run-x86-boot-tests.sh
echo "=== diff ==="
git diff docker/qemu/run-x86-boot-tests.sh
