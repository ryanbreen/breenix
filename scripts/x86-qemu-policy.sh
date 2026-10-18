#!/bin/bash
# Source before preparing an x86 QEMU boot; build-only paths do not call this guard.
if [ "${BREENIX_ALLOW_X86_QEMU:-}" != 1 ]; then
    printf '%s\n' "x86 QEMU refused by the 2026-10-18 bare-metal-only policy (S23); use the x86-metal boot loop (scripts/metal/metal-run); operator override: BREENIX_ALLOW_X86_QEMU=1." >&2
    exit 1
fi
