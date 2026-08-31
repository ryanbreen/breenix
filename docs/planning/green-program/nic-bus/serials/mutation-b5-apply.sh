#!/bin/bash
set -e
cd /root/breenix-gbus-prove
sed -i '183a\  expected_virtio_block=$((expected_virtio_block + 99))' docker/qemu/run-x86-gate.sh
git diff docker/qemu/run-x86-gate.sh
