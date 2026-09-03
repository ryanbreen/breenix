#!/bin/bash
# #693 investigation patch, apply. Adds the poll_tcp_oracle launch site back to
# the x86 RING3_SMOKE roster in kernel/src/main.rs, which is what the #693 x86
# batteries (693-FIX-2026-09-02.md sections 3.2 and 3.3) were run on.
#
# It is NOT in the branch's bytes: with it applied,
# docker/qemu/run-x86-boot-tests.sh cannot satisfy its census assertion at :548,
# because each forked-and-reaped oracle peer adds a production reaped row
# against a frozen pin (removed=6 at :177-:178). The round-2 review measured that
# gate PASS on main @ 3d601400 on 2 of 2 runs and FAIL at eba15887 on 2 of 2
# runs. Making that gate compatible is #697.
# Use this patch only with the bespoke repeat driver in this directory
# (x86-693fix-driver-20260902.sh), never with the boot gate, and revert it with
# x86-launch-site-revert.sh before committing anything.
set -e
cd "$(git rev-parse --show-toplevel)"
git apply --check docs/planning/green-program/sockets/serials/693-fix/x86-launch-site.patch
git apply docs/planning/green-program/sockets/serials/693-fix/x86-launch-site.patch
echo "applied: poll_tcp_oracle launched on x86 RING3_SMOKE"
git diff --stat kernel/src/main.rs
