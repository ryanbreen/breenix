# CI Plan: Parallel ARM64 + x86_64 Boot Tests

## Status: Planned

## Goal

Run ARM64 and x86_64 boot tests in parallel on every push/PR to main via GitHub Actions, replacing the current x86_64-only `boot-stages.yml` and the outdated manual-dispatch `arm64-boot.yml`.

---

## Background

### Current CI State

| Workflow | Trigger | Architecture | Status |
|----------|---------|-------------|--------|
| `boot-stages.yml` | push/PR to main | x86_64 only | Active, working |
| `arm64-boot.yml` | manual dispatch | ARM64 (minimal) | Outdated: no userspace, no ext2, only checks "Hello from ARM64" |
| `kthread-stress.yml` | push/PR to main | x86_64 only | Active, working |

The ARM64 workflow is severely behind. It doesn't build userspace, doesn't create an ext2 disk image, and only validates a single boot message. Locally, ARM64 testing is mature (120+ userspace programs, full shell prompt, ext2 filesystem) but this isn't reflected in CI.

---

## Architecture Decision: Panel-of-Experts Reconciliation

Two independent investigation agents analyzed this problem in parallel and reached **strong consensus** on all major decisions.

### Topology: Single Workflow, Two Explicit Jobs

**Decision**: One `boot-tests.yml` file with two jobs: `x86_64-boot` and `arm64-boot`.

**Rejected alternatives**:
- **Matrix strategy**: The two architectures have fundamentally different build commands, QEMU invocations, test validation, and runner requirements. A matrix would require `if:` conditionals on nearly every step, negating the DRY benefit.
- **Two separate workflows**: Creates operational overhead (two check marks, duplicated trigger config, harder to enforce "both must pass").

### Runner Selection: Native ARM64

**Decision**: ARM64 job runs on `ubuntu-24.04-arm64` (native ARM64 runner).

**Rejected alternatives**:
- **Cross-compilation on x86_64**: QEMU aarch64 on x86_64 uses TCG software emulation, making the 30-second ARM64 boot test take 5-10x longer. Cross-compilation adds linker complexity for zero benefit.
- **Hybrid build-on-x86/test-on-arm**: Adds artifact passing, sequential dependency, and cross-compilation complexity. Over-engineering for a ~10 minute build.

### Failure Coupling: Both Architectures Gate Merges

**Decision**: Both jobs must pass for PR merge. A single workflow with two jobs and branch protection gives this automatically.

**Rationale**: The kernel's core subsystems (scheduler, memory manager, syscall layer, VFS, ext2) are shared code. Letting one architecture's PRs merge while the other is broken creates "second-class citizen drift" -- the broken architecture gets deprioritized and becomes permanently broken within weeks.

**Escape hatch**: If one architecture has a known issue requiring a longer fix, temporarily add `continue-on-error: true` to that job (with a TODO and issue link). Never as permanent state.

### ext2 Disk Creation: sudo mount (immediate), genext2fs (follow-up)

**Decision**: Use `sudo ./scripts/create_ext2_disk.sh --arch aarch64` initially. GitHub Actions runners provide passwordless sudo.

**Critical bug identified**: `create_ext2_disk.sh` exits 0 without populating the image when not root (line 200). CI MUST use `sudo`.

**Follow-up hardening**: Add `genext2fs` support to the script. `genext2fs` creates populated ext2 images without requiring `mount`, eliminating the sudo/loopback fragility. Standard in embedded Linux build systems (Buildroot, Yocto).

### Migration: Incremental Cutover

1. Create `boot-tests.yml` with `workflow_dispatch` trigger for testing
2. Validate manually over several runs
3. Switch to `push/PR` trigger, simultaneously move `boot-stages.yml` to `workflow_dispatch` only
4. Update branch protection rules
5. After 2 weeks of stability, archive old workflows

---

## Implementation Plan

### New Workflow: `.github/workflows/boot-tests.yml`

```yaml
name: Boot Tests

on:
  push:
    branches: [ main ]
  pull_request:
    branches: [ main ]
  workflow_dispatch:
```

### Job 1: x86_64-boot

**Runner**: `ubuntu-latest`
**Timeout**: 20 minutes

This job is a direct copy of the current `boot-stages.yml` job to minimize migration risk.

**Steps**:
1. Checkout code
2. Init rust-fork submodule (shallow): `git submodule update --init --depth 1 rust-fork` + inner submodules
3. Install Rust `nightly-2025-06-24` with `rust-src`, `llvm-tools-preview`
4. Install system deps: `qemu-system-x86`, `qemu-utils`, `ovmf`, `nasm`
5. Cache cargo registry (key: `Linux-x86_64-cargo-registry-{Cargo.lock hash}`)
6. Cache cargo build (key: `Linux-x86_64-cargo-build-{Cargo.lock hash}`)
7. Build userspace: `cd userspace/programs && ./build.sh`
8. Pre-build kernel: `cargo build --release -p breenix --features testing,external_test_bins --bin qemu-uefi`
9. Run test: `cargo run -p xtask -- boot-stages`
10. Upload artifacts on failure: `target/xtask_boot_stages_output.txt`, `target/xtask_user_output.txt`

### Job 2: arm64-boot

**Runner**: `ubuntu-24.04-arm64`
**Timeout**: 20 minutes

**Steps**:

1. **Checkout code**
   ```bash
   uses: actions/checkout@v4
   ```

2. **Init rust-fork submodule (shallow)**
   ```bash
   git submodule update --init --depth 1 rust-fork
   cd rust-fork && git submodule update --init --depth 1 library/stdarch library/backtrace
   ```

3. **Install Rust nightly-2025-06-24**
   ```yaml
   uses: dtolnay/rust-toolchain@master
   with:
     toolchain: nightly-2025-06-24
     components: rust-src, llvm-tools-preview
   ```

4. **Install system dependencies**
   ```bash
   sudo apt-get update
   sudo apt-get install -y qemu-system-arm e2fsprogs
   ```

5. **Cache cargo registry**
   ```yaml
   key: ${{ runner.os }}-${{ runner.arch }}-cargo-registry-${{ hashFiles('**/Cargo.lock') }}
   ```

6. **Cache cargo build**
   ```yaml
   key: ${{ runner.os }}-${{ runner.arch }}-cargo-build-${{ hashFiles('**/Cargo.lock') }}
   ```

7. **Build userspace for aarch64**
   ```bash
   export PATH="$PATH:$(rustc --print sysroot)/lib/rustlib/aarch64-unknown-linux-gnu/bin"
   cd userspace/programs && ./build.sh --arch aarch64
   ```
   Note: `build.sh` handles building `libbreenix-libc` as step [1/3] internally.

8. **Create ext2 disk image**
   ```bash
   sudo ./scripts/create_ext2_disk.sh --arch aarch64
   ```
   Creates `target/ext2-aarch64.img` (48MB) with all userspace binaries populated.

9. **Build ARM64 kernel**
   ```bash
   cargo build --release \
     --target aarch64-breenix.json \
     -Z build-std=core,alloc \
     -Z build-std-features=compiler-builtins-mem \
     -p kernel --bin kernel-aarch64
   ```

10. **Boot ARM64 kernel in QEMU**
    ```bash
    OUTPUT_DIR="/tmp/breenix_aarch64_ci"
    mkdir -p "$OUTPUT_DIR"
    cp target/ext2-aarch64.img "$OUTPUT_DIR/ext2-writable.img"

    timeout 60 qemu-system-aarch64 \
      -M virt -cpu cortex-a72 -m 512 -smp 4 \
      -kernel target/aarch64-breenix/release/kernel-aarch64 \
      -display none -no-reboot \
      -device virtio-gpu-device \
      -device virtio-keyboard-device \
      -device virtio-blk-device,drive=ext2 \
      -drive if=none,id=ext2,format=raw,file="$OUTPUT_DIR/ext2-writable.img" \
      -device virtio-net-device,netdev=net0 \
      -netdev user,id=net0 \
      -serial file:"$OUTPUT_DIR/serial.txt" &
    QEMU_PID=$!

    # Wait for userspace shell prompt (45s timeout)
    BOOT_OK=false
    for i in $(seq 1 45); do
      if [ -f "$OUTPUT_DIR/serial.txt" ]; then
        if grep -q "breenix>" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
          BOOT_OK=true
          break
        fi
        if grep -qiE "(KERNEL PANIC|panic!)" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
          break
        fi
      fi
      sleep 1
    done

    kill $QEMU_PID 2>/dev/null || true
    wait $QEMU_PID 2>/dev/null || true

    if $BOOT_OK; then
      SHELL_COUNT=$(grep -o "init_shell" "$OUTPUT_DIR/serial.txt" 2>/dev/null | wc -l | tr -d ' ')
      if [ "${SHELL_COUNT:-0}" -le 5 ]; then
        echo "SUCCESS: ARM64 boot test passed ($SHELL_COUNT init_shell mentions)"
        exit 0
      else
        echo "FAIL: Too many init_shell mentions: $SHELL_COUNT"
        cat "$OUTPUT_DIR/serial.txt"
        exit 1
      fi
    else
      echo "FAIL: ARM64 boot test failed"
      echo "=== Serial output ==="
      cat "$OUTPUT_DIR/serial.txt" 2>/dev/null || echo "(no output)"
      exit 1
    fi
    ```

    **Success criteria**:
    - `breenix>` prompt appears (actual userspace shell, NOT kernel fallback)
    - `init_shell` count <= 5 (no excessive respawning)
    - No `KERNEL PANIC` or `panic!` in output

11. **Upload serial log on failure**
    ```yaml
    if: failure()
    uses: actions/upload-artifact@v4
    with:
      name: arm64-boot-log-${{ github.run_number }}
      path: /tmp/breenix_aarch64_ci/serial.txt
      retention-days: 7
    ```

---

## Caching Strategy

Cache keys MUST include `runner.arch` to prevent cross-architecture cache pollution (both runners report `runner.os` as `Linux`).

| Cache | Key Pattern | Path |
|-------|-------------|------|
| Cargo registry | `${{ runner.os }}-${{ runner.arch }}-cargo-registry-{Cargo.lock}` | `~/.cargo/registry` |
| Cargo build | `${{ runner.os }}-${{ runner.arch }}-cargo-build-{Cargo.lock}` | `target` |

**Not cached**: rust-fork submodule (shallow clone is fast), ext2 images (depend on userspace binaries), serial output (test artifacts).

---

## Timeout Budget

| Phase | x86_64 | ARM64 |
|-------|--------|-------|
| Checkout + submodule | ~30s | ~30s |
| Rust install | ~30s | ~30s |
| APT packages | ~30s | ~30s |
| Cache restore | ~15s | ~15s |
| Userspace build (cached) | ~2-3min | ~2-3min |
| ext2 creation | N/A (xtask handles) | ~30s |
| Kernel build (cached) | ~1-2min | ~2-3min |
| Kernel build (cold) | ~5-8min | ~5-10min |
| Test execution | ~5-8min | ~30-60s |
| **Total (cached)** | **~10-14min** | **~7-10min** |
| **Total (cold)** | **~15-20min** | **~10-15min** |

Wall-clock time is `max(x86_64, arm64)` since they run in parallel = x86_64 time.

---

## Branch Protection Configuration

After the workflow is stable, update repository settings:

**Required status checks for main**:
- `Boot Tests / x86_64-boot`
- `Boot Tests / arm64-boot`

**Remove**:
- `Boot Stages Test / Validate Boot Stages` (from old `boot-stages.yml`)

---

## Files Modified

| File | Change |
|------|--------|
| `.github/workflows/boot-tests.yml` | **New**: Combined workflow with both jobs |
| `.github/workflows/boot-stages.yml` | Change trigger to `workflow_dispatch` only (deprecate) |
| `.github/workflows/arm64-boot.yml` | Delete (fully superseded) |

---

## Follow-up Work

1. **genext2fs support**: Add `--method genext2fs` flag to `scripts/create_ext2_disk.sh` to eliminate `sudo mount` dependency in CI
2. **ARM64 xtask subcommand**: Create `cargo run -p xtask -- arm64-boot` to match x86_64 pattern, moving QEMU lifecycle and validation logic out of workflow YAML
3. **ARM64 kthread stress test**: Add `arm64-kthread` job to `kthread-stress.yml`
4. **Reusable setup action**: If ARM64 jobs appear in 3+ workflows, extract `.github/actions/setup-breenix-arm64/action.yml`

---

## Verification Checklist

- [ ] New workflow passes on `workflow_dispatch` (manual trigger)
- [ ] x86_64 job is functionally identical to old `boot-stages.yml`
- [ ] ARM64 job reaches `breenix>` prompt consistently
- [ ] Cache keys don't collide between architectures
- [ ] Failure in one job doesn't prevent the other from running
- [ ] Artifacts uploaded on failure contain useful debugging info
- [ ] Branch protection rules updated after 2-week stability period
