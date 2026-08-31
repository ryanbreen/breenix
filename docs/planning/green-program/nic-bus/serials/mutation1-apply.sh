#!/bin/bash
set -e
cd /root/breenix-gbus-prove
sed -i '419i\    CENSUS_VIRTIO_BLOCK=$((CENSUS_VIRTIO_BLOCK + 99))' docker/qemu/run-x86-boot-tests.sh
git diff docker/qemu/run-x86-boot-tests.sh
