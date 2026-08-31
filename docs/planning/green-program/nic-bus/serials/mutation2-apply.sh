#!/bin/bash
set -e
cd /root/breenix-gbus-prove
sed -i '427i\    CENSUS_NETWORK=0' docker/qemu/run-x86-boot-tests.sh
git diff docker/qemu/run-x86-boot-tests.sh
