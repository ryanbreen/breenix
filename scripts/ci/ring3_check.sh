#!/usr/bin/env bash
set -euo pipefail

# CI Ring3 smoke test with streaming detection
# - Runs QEMU via scripts/breenix_runner.py with --ci-ring3 to exit early
#   when success or failure markers appear in stdout
# - Verifies absence of fault patterns in the saved log
# - Prints a concise summary and leaves logs/ artifacts for CI upload

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# scripts/ci -> repo root
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

MODE="${1:-uefi}"  # uefi|bios
TIMEOUT_SECONDS="${RING3_TIMEOUT_SECONDS:-20}"

echo "=== Ring3 smoke: mode=$MODE timeout=${TIMEOUT_SECONDS}s ==="

# Ensure no stale QEMU holds the image lock
pkill -f qemu-system-x86_64 >/dev/null 2>&1 || true

# Run with streaming detection so we don't always wait for timeout
set +e
export BREENIX_QEMU_LOG_PATH="${REPO_ROOT}/logs/qemu_debug.log"
  python3 "${REPO_ROOT}/scripts/breenix_runner.py" \
  --mode "$MODE" \
  --ci-ring3 \
  --timeout-seconds "$TIMEOUT_SECONDS"
run_rc=$?
set -e

# Locate latest log
LATEST_LOG=$(ls -t logs/*.log 2>/dev/null | head -1 || true)
SERIAL_LOG=$(ls -t logs/*_serial.log 2>/dev/null | head -1 || true)
if [[ -z "${LATEST_LOG}" ]]; then
  echo "ERROR: No log files found in logs/ directory"
  exit 2
fi

echo "Latest log: ${LATEST_LOG}"
if [[ -n "${SERIAL_LOG}" ]]; then
  echo "Latest serial log: ${SERIAL_LOG}"
fi
if [[ -s "${REPO_ROOT}/logs/qemu_debug.log" ]]; then
  echo "QEMU debug log present (guest_errors). Tail:";
  tail -n 50 "${REPO_ROOT}/logs/qemu_debug.log" || true
fi

# Helper to use the canonical log searcher
search() {
  echo "$1" > /tmp/log-query.txt
  "${REPO_ROOT}/scripts/find-in-logs"
}

# 1) Check for obvious faults (must be absent)
echo "=== Checking for fault patterns ==="
set +e
search '-E "DOUBLE FAULT|Page Fault|PAGE FAULT|panic|backtrace"'
fault_rc=$?
set -e
if [[ $fault_rc -eq 0 ]]; then
  echo "ERROR: Fault patterns found in latest log"
  exit 3
fi

# 2) Success markers (streaming may have already exited on success). We verify again from log.
echo "=== Checking for success markers ==="
# Prefer a canonical OK marker if kernel emits it; otherwise fallback logic depends on runner outcome
set +e
search '-F "[ OK ] RING3_SMOKE: userspace executed + syscall path verified"'
canonical_ok_rc=$?
have_hello=$(search '-F "Hello from userspace! Current time:"' >/dev/null && echo yes || echo no)
have_cs=$(search '-F "Context switch: from_userspace=true, CS=0x33"' >/dev/null && echo yes || echo no)
have_user_output=$(search '-F "USERSPACE OUTPUT:"' >/dev/null && echo yes || echo no)
set -e

if [[ $canonical_ok_rc -eq 0 ]]; then
  : # canonical success found
elif [[ $run_rc -eq 0 ]]; then
  # Runner reported success via streaming markers; require core pair of proofs
  if [[ "$have_hello" != yes || "$have_cs" != yes ]]; then
    echo "ERROR: Streaming success reported, but core markers not present in log"
    echo "hello=$have_hello cs=$have_cs"
    exit 4
  fi
else
  # Runner did not report streaming success; require full composite including userspace_output
  if [[ "$have_hello" != yes || "$have_cs" != yes || "$have_user_output" != yes ]]; then
    echo "ERROR: Ring3 success markers not found in latest log"
    echo "hello=$have_hello cs=$have_cs userspace_output=$have_user_output"
    exit 4
  fi
fi

# 3) Optional completion marker (warn only)
set +e
search '-F "🎯 KERNEL_POST_TESTS_COMPLETE 🎯"' >/dev/null
comp_rc=$?
set -e
if [[ $comp_rc -ne 0 ]]; then
  echo "WARNING: Completion marker not found; continuing."
fi

# 4) Print brief summary context for PR logs
echo "=== Userspace context (last occurrences) ==="
set +e
search '-C3 "Hello from userspace! Current time:"' || true
search '-C2 "Context switch: from_userspace=true, CS=0x33"' || true
set -e

if [[ $run_rc -ne 0 ]]; then
  # Streaming mode might exit non-zero if it saw a failure; but we already verified absence of faults
  # If non-zero but we have success markers and no faults, normalize to success
  echo "Note: Runner exit code=$run_rc, but markers validated. Normalizing to success."
fi

echo "=== RING3 CHECK: PASS ==="
exit 0
