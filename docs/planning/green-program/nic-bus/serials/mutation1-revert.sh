#!/bin/bash
set -e
cd /root/breenix-gbus-prove
git checkout -- docker/qemu/run-x86-boot-tests.sh
git diff --stat docker/qemu/run-x86-boot-tests.sh
echo "REVERT_DONE"
