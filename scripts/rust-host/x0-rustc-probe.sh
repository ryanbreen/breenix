#!/usr/bin/env bash
# Cargo's RUSTC_WRAPPER entry point for X0.9.
set -euo pipefail
exec python3 "$(dirname "$0")/x0_profile.py" "$@"
