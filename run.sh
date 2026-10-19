#!/bin/bash
# Breenix Interactive Runner
# ===========================
# Boots Breenix with a display or serial console; probe mode exits after its verdict.
#
# Usage:
#   ./run.sh              # ARM64 with native cocoa display (default)
#   ./run.sh --clean      # Full rebuild (userspace + ext2 disk + kernel), then run
#   ./run.sh --x86        # x86_64 with VNC display
#   ./run.sh --headless   # ARM64 with serial output only
#   ./run.sh --x86 --headless  # x86_64 with serial output only
#   ./run.sh --no-build        # Skip all builds, use existing artifacts
#   ./run.sh --resolution 1920x1080  # Custom resolution
#   ./run.sh --parallels       # Build and boot on Parallels Desktop
#   ./run.sh --parallels --no-build  # Deploy existing build to Parallels
#   ./run.sh --parallels --test      # Build, boot, wait, screenshot, exit
#   ./run.sh --parallels --test 60   # Same with custom wait (default: 35s)
#   ./run.sh --parallels --probe     # Production probe, stop after PROBE DONE
#   ./run.sh --vmware --probe        # Same probe on VMware Fusion
#   ./run.sh --parallels --tests     # Testing kernel, stop after its userspace report
#   ./run.sh --ahci             # ARM64 with AHCI (SATA) disk instead of virtio-blk
#   ./run.sh --ahci --headless # ARM64 AHCI with serial output only
#   ./run.sh --btrt            # ARM64 BTRT structured boot test
#   ./run.sh --btrt --x86      # x86_64 BTRT structured boot test
#   ./run.sh --no-audio        # Disable audio entirely (default: coreaudio output on)
#   ./run.sh --accel tcg       # Select ARM64 software emulation explicitly
#
# Display:
#   ARM64:  Native window (cocoa) - no VNC needed
#   x86_64: VNC at localhost:5900
#
# Both architectures run QEMU natively on the host.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$SCRIPT_DIR"
cd "$BREENIX_ROOT"
source "$BREENIX_ROOT/scripts/host-slots.sh"
RUN_ARGS=("$@")

# #826/#834/#865/R181: this script's native (non-Parallels, non-VMware)
# QEMU boot runs behind the host-wide lock in
# docker/qemu/lib/qemu-host-lock.sh -- sourced unconditionally since arch is
# runtime-selected, and both the arm64 and x86_64 legs below call
# qemu_host_lock_acquire with their own binary name (#865 keeps one lock
# domain per QEMU binary, so the two legs cooperate with a concurrent gate
# lane running the same binary without contending with each other). The
# Parallels and VMware code paths above this point boot a VM, not a native
# QEMU process, and exit before reaching here. #834 extended this lock's
# coverage from docker/qemu/*.sh (its original #826/R181 scope) to run.sh
# as well; #865 closed the x86_64-leg gap this comment used to disclose.

# Defaults: ARM64 with graphics
ARCH="arm64"
HEADLESS=false
CLEAN=false
NO_BUILD=false
DRY_RUN=false
DISK_OVERRIDE=""
BTRT=false
PARALLELS=false
PARALLELS_VM="breenix-dev"
PARALLELS_TEST=false
PARALLELS_TEST_WAIT=35
VMWARE=false
SUITE=""
ONLY=""
GPU_CORE=false
PROBE=false
TESTS=false
PROGRAM=""
PROGRAM_ARG=""
KERNEL_FEATURES=""
BOOT_MODE=default
GATE_TIMEOUT=1800
DEBUG=false
REBUILD_HOME=false
RESOLUTION=""
USE_AHCI=false
NO_AUDIO=false

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --x86|--x86_64|--amd64)
            ARCH="x86_64"
            shift
            ;;
        --arm64|--aarch64)
            ARCH="arm64"
            shift
            ;;
        --clean)
            CLEAN=true
            shift
            ;;
        --accel)
            export BREENIX_ARM64_QEMU_ACCEL="${2:?--accel needs hvf or tcg}"
            shift 2
            ;;
        --disk)
            DISK_OVERRIDE="${2:?--disk needs an ext2 image path}"
            shift 2
            ;;
        --no-build)
            NO_BUILD=true
            shift
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        --parallels)
            PARALLELS=true
            ARCH="arm64"
            shift
            ;;
        --vmware)
            VMWARE=true
            ARCH="arm64"
            shift
            ;;
        --test)
            PARALLELS_TEST=true
            # Optional: next arg is wait seconds if it's a number
            if [[ "${2:-}" =~ ^[0-9]+$ ]]; then
                PARALLELS_TEST_WAIT="$2"
                shift
            fi
            shift
            ;;
        --vm)
            PARALLELS_VM="$2"
            shift 2
            ;;
        --btrt)
            BTRT=true
            shift
            ;;
        --debug)
            DEBUG=true
            shift
            ;;
        --ahci)
            USE_AHCI=true
            shift
            ;;
        --no-audio)
            NO_AUDIO=true
            shift
            ;;
        --rebuild-home)
            REBUILD_HOME=true
            shift
            ;;
        --resolution)
            RESOLUTION="$2"
            shift 2
            ;;
        --serial-log)
            SERIAL_LOG_OVERRIDE="$2"
            shift 2
            ;;
        --gate-timeout)
            GATE_TIMEOUT="${2:-}"
            shift 2
            ;;
        --probe)
            PROBE=true
            shift
            ;;
        --tests)
            TESTS=true
            shift
            ;;
        --program)
            PROGRAM="${2:-}"
            shift 2
            ;;
        --program-arg)
            PROGRAM_ARG="${2:-}"
            shift 2
            ;;
        --kernel-features)
            KERNEL_FEATURES="${2:-}"
            shift 2
            ;;
        --case)
            [ -n "${2:-}" ] || { echo "--case needs a category id"; exit 2; }
            ONLY="${2:-}"
            shift 2
            ;;
        --suite)
            SUITE="${2:-}"
            shift 2
            ;;
        --gpu-core)
            GPU_CORE=true
            shift
            ;;
        --retina)
            RETINA=true
            shift
            ;;
        --headless|--serial)
            HEADLESS=true
            shift
            ;;
        --graphics|--vnc)
            HEADLESS=false
            shift
            ;;
        -h|--help)
            echo "Usage: ./run.sh [options]"
            echo ""
            echo "Options:"
            echo "  --clean                    Full rebuild: userspace, system ext2, kernel"
            echo "  --rebuild-home             Recreate home disk (destroys user data)"
            echo "  --no-build                 Skip all builds, use existing artifacts"
            echo "  --disk IMG                 Boot a supplied ext2 image, without rebuilding it"
            echo "  --x86, --x86_64, --amd64   Run x86_64 kernel (default: ARM64)"
            echo "  --arm64, --aarch64         Run ARM64 kernel (default)"
            echo "  --parallels                Build and boot on Parallels Desktop VM"
            echo "  --parallels --test [N]     Build, boot, wait N sec (default 35), screenshot, exit"
            echo "  --vmware                   Build and boot on VMware Fusion VM"
            echo "  --vm NAME                  Parallels VM name (default: breenix-dev)"
            echo "  --headless, --serial       Run without display (serial only)"
            echo "  --accel hvf|tcg            ARM64 accelerator (default hvf on Apple Silicon, tcg elsewhere)"
            echo "  --graphics, --vnc          Run with VNC display (default)"
            echo "  --btrt                     Run BTRT structured boot test"
            echo "  --ahci                     Use AHCI (SATA) disk instead of virtio-blk (ARM64)"
            echo "  --no-audio                 Disable audio entirely (default: coreaudio output on)"
            echo "  --debug                    Enable GDB stub (port 1234) for debugging"
            echo "  --serial-log PATH          Parallels/VMware: write the VM's serial output to PATH"
            echo "  --probe                    Parallels/VMware: run /sbin/probe as PID 1, wait for PROBE DONE, stop VM"
            echo "  --tests                    Parallels/VMware: boot the testing kernel, wait for its userspace report, stop VM"
            echo "  --program PATH             Parallels/VMware: /sbin/probe runs guest program PATH, wait for RUN DONE, stop VM"
            echo "  --program-arg WORD         The one argument --program's PATH gets"
            echo "  --kernel-features LIST     With --program: build the kernel with these cargo features (a measurement kernel)"
            echo "  --gate-timeout N           DONE deadline in seconds (default 1800)"
            echo "  --case CATEGORY[/CASE]     Run one suite category or case (not full-suite evidence)"
            echo "  --suite ID                 Parallels/VMware: run effort suite ID (docs/suites/ID.json) as PID 1"
            echo "  --suite ID,ID,...|all      Parallels/VMware: run several suites in one boot, in order"
            echo "  --gpu-core                 Parallels: the GPU core (kernel/src/gpu/) owns the virtio-gpu device"
            echo "                             instead of the legacy display driver; implied by --parallels --suite gpu"
            echo "  --dry-run                  Check target policy and exit before building or booting"
            echo "  BREENIX_CATCHUP=1          Allow effort catch-up on Parallels/VMware; recorded in metadata"
            echo "  BREENIX_PARALLELS_3D=off   Parallels: create the VM with 3D acceleration off (attestation control)"
            echo "  BREENIX_VMWARE_POINTER=relative  VMware: tablet disabled, VNC buttons and wheel go to the USB mouse"
            echo "  --retina                   Parallels/VMware: native Retina resolution (default: scaled 2x, readable)"
            echo "  --resolution WxH           Set display resolution (e.g. 1920x1080)"
            echo "                             Default: auto-detect from screen"
            echo "  -h, --help                 Show this help"
            echo ""
            echo "Debugging:"
            echo "  QMP socket always at: /tmp/breenix-qmp.sock"
            echo "  GDB (--debug):  target remote :1234"
            echo "  Forensics:      scripts/forensic-capture.sh"
            echo ""
            echo "Display:"
            echo "  ARM64:  Native window (cocoa)"
            echo "  x86_64: VNC at localhost:5900"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            echo "Use --help for usage information"
            exit 1
            ;;
    esac
done

# Refuse effort work on x86-64, Parallels and VMware before builds, VM setup or host-slot
# supervision. The supervised re-exec (BREENIX_SLOT_SESSION) inherits the normalized
# BREENIX_CATCHUP and the checked stamp, so the check and its launch line run once.
TARGET=arm64-qemu
if [ "$ARCH" = x86_64 ]; then TARGET=x86; fi
if [ "$PARALLELS" = true ]; then TARGET=parallels
elif [ "$VMWARE" = true ]; then TARGET=vmware
fi
if [ "$TARGET" = x86 ] && [ "$DRY_RUN" != true ]; then
    source "$BREENIX_ROOT/scripts/x86-qemu-policy.sh"
fi
if [ -z "${BREENIX_SLOT_SESSION:-}" ]; then
    case "$(printf %s "${BREENIX_CATCHUP:-}" | tr '[:upper:]' '[:lower:]')" in
        ''|0|false|no) BREENIX_CATCHUP=0 ;;
        1|true|yes) BREENIX_CATCHUP=1 ;;
        *)
            echo "BREENIX_CATCHUP must be 1, true or yes for the catch-up, or 0, false, no or unset; got '$BREENIX_CATCHUP'" >&2
            exit 1
            ;;
    esac
    export BREENIX_CATCHUP
    echo "Breenix target=$TARGET BREENIX_CATCHUP=$BREENIX_CATCHUP dry-run=$DRY_RUN"
    if [ "$TARGET" != arm64-qemu ]; then
        POLICY_SHA=$(python3 "$BREENIX_ROOT/scripts/effort-target-guard.py" --repo "$BREENIX_ROOT" \
            --target "$TARGET") || exit 1
        export BREENIX_TARGET_POLICY_CHECKED="$TARGET:$POLICY_SHA"
    fi
fi
if [ "$DRY_RUN" = true ]; then
    echo "Target policy accepted; no build or boot requested."
    exit 0
fi
# shellcheck source=docker/qemu/lib/qemu-host-lock.sh
source "$BREENIX_ROOT/docker/qemu/lib/qemu-host-lock.sh"

host_slots_start "$SCRIPT_DIR/$(basename "${BASH_SOURCE[0]}")" "${RUN_ARGS[@]}"

if [ -n "$DISK_OVERRIDE" ]; then
    DISK_OVERRIDE=$(python3 - "$DISK_OVERRIDE" <<'PYDISK'
from pathlib import Path
import sys
try:
    path = Path(sys.argv[1]).resolve(strict=True)
    if not path.is_file():
        raise ValueError('not a regular image file')
    with path.open('rb') as image:
        image.seek(1024 + 56)
        if image.read(2) != b'\x53\xef':
            raise ValueError('not an ext2 image (expected a raw filesystem, not a partitioned disk)')
    print(path)
except (OSError, ValueError) as error:
    sys.exit('--disk: ' + str(error))
PYDISK
    ) || exit 2
    python3 "$BREENIX_ROOT/scripts/ext2-machine.py" "$DISK_OVERRIDE" "$ARCH" || exit 2
    if [ "$BTRT" = true ]; then
        echo "--disk cannot be combined with --btrt"
        exit 2
    fi
fi

# Opt in for constrained build hosts; VM execution retains normal priority.
run_build() {
    if [ "${BREENIX_BUILD_BACKGROUND:-0}" = 1 ] && [ "$(uname)" = Darwin ]; then
        "$BREENIX_ROOT/scripts/build-background.sh" "$@"
    else
        "$@"
    fi
}

[[ "$GATE_TIMEOUT" =~ ^[1-9][0-9]*$ ]] || { echo "--gate-timeout needs positive seconds"; exit 2; }
if [ "$VMWARE" = true ] && [ "$PARALLELS_TEST" = true ]; then
    echo "--test screenshots are supported only on Parallels"
    exit 2
fi

# --suite ID: the VM's ext2 disk gets /etc/breenix/boot-target ("suite ID"), so the
# production kernel runs /sbin/suite-ID as PID 1 (docs/boot-path.md, "Boot modes").
if [ -n "$SUITE" ]; then
    if [ "$PARALLELS" != true ] && [ "$VMWARE" != true ]; then
        echo "--suite is for --parallels or --vmware (ARM64 QEMU: scripts/boot-interactive.sh --mode suite --suite ID)"
        exit 1
    fi
    # A comma-separated list runs several suites in one boot (docs/suites/README.md, "Several suites in one boot").
    if [ "$PARALLELS" = true ]; then SUITE_TARGET=parallels; else SUITE_TARGET=vmware; fi
    SUITE=$(python3 "$BREENIX_ROOT/scripts/suite-sequence.py" resolve "$SUITE" --target "$SUITE_TARGET") || exit 1
    if [[ "$SUITE" == *,* ]] && [ -n "$ONLY" ]; then
        echo "--case watches one case of one suite, not a list"; exit 2
    fi
fi

# --gpu-core: the ESP carries \EFI\BREENIX\GPUCORE, which the loader reports to the kernel
# (arm64_boot_contract::BOOT_FLAG_GPU_CORE), so the GPU core drives the virtio-gpu device and
# shows its kernel console. The gpu suite needs it; other boots keep the legacy display.
if [ "$PARALLELS" = true ] && [[ ",$SUITE," == *",gpu,"* ]]; then GPU_CORE=true; fi
if [ "$GPU_CORE" = true ] && [ "$PARALLELS" != true ]; then
    echo "--gpu-core is for --parallels"
    exit 2
fi

if [ -n "$ONLY" ]; then
    [ -n "$SUITE" ] && [[ "$ONLY" =~ ^[a-zA-Z0-9_./-]+$ ]] || {
        echo "--case needs --suite ID and a category id"; exit 2;
    }
fi

if [ "$PROBE" = true ]; then
    if [ "$PARALLELS" != true ] && [ "$VMWARE" != true ]; then
        echo "--probe is for --parallels or --vmware (ARM64 QEMU: scripts/boot-interactive.sh --mode probe)"
        exit 1
    fi
    if [ -n "$SUITE" ] || [ "$PARALLELS_TEST" = true ] || [ "$BTRT" = true ]; then
        echo "--probe cannot be combined with --suite, --test or --btrt"
        exit 1
    fi
    BOOT_MODE=probe
elif [ -n "$SUITE" ]; then
    BOOT_MODE=suite
fi
if [ -n "$PROGRAM" ]; then
    if [ "$PARALLELS" != true ] && [ "$VMWARE" != true ]; then
        echo "--program is for --parallels or --vmware (ARM64 QEMU: scripts/boot-interactive.sh --mode program)"
        exit 1
    fi
    if [ "$PROBE" = true ] || [ -n "$SUITE" ] || [ "$TESTS" = true ] || [ "$PARALLELS_TEST" = true ] || [ "$BTRT" = true ]; then
        echo "--program cannot be combined with --probe, --suite, --tests, --test or --btrt"
        exit 1
    fi
    [[ "$PROGRAM" =~ ^/[A-Za-z0-9_./-]+$ ]] || { echo "--program needs an absolute guest path"; exit 2; }
    [ -z "$PROGRAM_ARG" ] || [[ "$PROGRAM_ARG" =~ ^[a-z0-9]+(-[a-z0-9]+)*$ ]] ||
        { echo "--program-arg needs lowercase words joined by '-'"; exit 2; }
    BOOT_MODE=program
elif [ -n "$PROGRAM_ARG" ] || [ -n "$KERNEL_FEATURES" ]; then
    echo "--program-arg and --kernel-features need --program"
    exit 2
fi
# The manifests of the suites this boot runs, for the quiet-host check.
SUITE_MANIFESTS=()
IFS=',' read -r -a SUITE_IDS <<< "$SUITE"
for id in ${SUITE_IDS[@]+"${SUITE_IDS[@]}"}; do SUITE_MANIFESTS+=("$BREENIX_ROOT/docs/suites/$id.json"); done

# --tests: the testing kernel (--features testing) and its test loader, as
# scripts/boot-interactive.sh --mode tests runs it on ARM64 QEMU.
KERNEL_FEATURE_ARGS=()
if [ "$TESTS" = true ]; then
    if [ "$PARALLELS" != true ] && [ "$VMWARE" != true ]; then
        echo "--tests is for --parallels or --vmware (ARM64 QEMU: scripts/boot-interactive.sh --mode tests)"
        exit 1
    fi
    if [ "$PROBE" = true ] || [ -n "$SUITE" ] || [ "$PARALLELS_TEST" = true ] || [ "$BTRT" = true ]; then
        echo "--tests cannot be combined with --probe, --suite, --test or --btrt"
        exit 1
    fi
    if [ "$NO_BUILD" = true ]; then
        echo "--tests builds the testing kernel; it cannot be combined with --no-build"
        exit 1
    fi
    BOOT_MODE=tests
    KERNEL_FEATURE_ARGS=(--features testing)
fi
if [ -n "$KERNEL_FEATURES" ]; then
    [[ "$KERNEL_FEATURES" =~ ^[a-z0-9_]+(,[a-z0-9_]+)*$ ]] || { echo "--kernel-features needs comma-separated feature names"; exit 2; }
    [ "$NO_BUILD" != true ] || { echo "--kernel-features builds the kernel; it cannot be combined with --no-build"; exit 1; }
    KERNEL_FEATURE_ARGS=(--features "$KERNEL_FEATURES")
fi

# Keep host launch policy beside this run's serial, including successful catch-up overrides.
record_launch_metadata() {
    python3 - "$SERIAL_LOG.launch.json" "$TARGET" "${BREENIX_CATCHUP:-0}" "$(git rev-parse HEAD)" <<'PYLAUNCH'
import json, sys
from pathlib import Path
Path(sys.argv[1]).write_text(json.dumps({'target': sys.argv[2], 'catchup': sys.argv[3] == '1',
                                       'commit': sys.argv[4]}) + '\n')
PYLAUNCH
}

# Whether this boot writes a boot target onto a copy of the ext2 disk.
writes_boot_target() {
    [ "$BOOT_MODE" = probe ] || [ "$BOOT_MODE" = suite ] || [ "$BOOT_MODE" = program ]
}

# The nameserver this target's /etc/resolv.conf names. VMware's adapter is bridged
# to the host's network, whose nameserver only the boot-time DHCP lease knows, so
# its resolv.conf is the kernel's /proc/net/pnp.
target_nameserver() {
    if [ "$PARALLELS" = true ]; then echo 10.211.55.1;
    elif [ "$VMWARE" = true ]; then echo dhcp;
    else echo 10.0.2.3; fi
}

# The ext2 disk a Parallels or VMware VM boots: the built disk itself, or with
# --suite or --probe a copy carrying the boot target (the built disk never gets one, so later
# ordinary boots keep using the built disk). Sets BOOT_EXT2_DISK.
stage_boot_ext2_disk() {
    local built="$1" staging_dir="$2"
    BOOT_EXT2_DISK="$built"
    if [ ! -f "$built" ]; then
        echo "ERROR: $BOOT_MODE needs the ext2 disk at $built (run without --no-build to create it)"
        exit 1
    fi
    mkdir -p "$staging_dir"
    BOOT_EXT2_DISK="$staging_dir/ext2-boot-target.img"
    rm -f "$BOOT_EXT2_DISK"
    cp "$built" "$BOOT_EXT2_DISK" || { echo "ERROR: could not copy $built for the boot target"; exit 1; }
    "$BREENIX_ROOT/scripts/write-resolv-conf.sh" "$BOOT_EXT2_DISK" "$(target_nameserver)" || exit 1
    writes_boot_target || return 0
    if [ "$PROBE" = true ]; then
        "$BREENIX_ROOT/scripts/write-boot-target.sh" "$BOOT_EXT2_DISK" --probe \
            "$BREENIX_ROOT/userspace/programs/aarch64/probe.elf" || exit 1
    elif [ "$BOOT_MODE" = program ]; then
        "$BREENIX_ROOT/scripts/write-boot-target.sh" "$BOOT_EXT2_DISK" --program "$PROGRAM" \
            ${PROGRAM_ARG:+"$PROGRAM_ARG"} || exit 1
    else
        local hostkeys_backend=none
        if [ "$PARALLELS" = true ]; then
            hostkeys_backend=prlsdk:declaration
        elif [ -n "${BREENIX_VMWARE_VNC_PORT:-}" ]; then
            hostkeys_backend="vnc:127.0.0.1:$BREENIX_VMWARE_VNC_PORT"
        fi
        local sequence=
        [[ "$SUITE" != *,* ]] || sequence=$SUITE
        BREENIX_HOSTKEYS="$(python3 "$BREENIX_ROOT/scripts/hostkeys.py" declare --backend "$hostkeys_backend")" \
        "$BREENIX_ROOT/scripts/write-boot-target.sh" "$BOOT_EXT2_DISK" "${SUITE%%,*}" \
            "$BREENIX_ROOT/userspace/programs/aarch64/suite-${SUITE%%,*}.elf" "$sequence" "$ONLY" || exit 1
        if [[ ",$SUITE," == *",sdk,"* ]]; then
            # The Developer SDK suite's host side stages the SDK's results onto the copy.
            python3 "$BREENIX_ROOT/scripts/sdk-suite.py" --arch aarch64 --image "$BOOT_EXT2_DISK" || exit 1
        fi
    fi
}

ext2_source_identity() {
    (set -o pipefail; shasum -a 256 "$EXT2_DISK" | awk '{print $1}')
}

# Whether the VM's wrapped data disk (whose boot-target marker file is $1) must
# be made again: when the built disk changed, this run selects a boot target,
# an earlier run's boot target must be cleared, or the disk names another
# nameserver. Otherwise the wrapped disk, and any changes the guest made to it, is kept.
ext2_rewrap_needed() {
    local rebuilt="$1" marker="$2" identity
    if [ -n "$DISK_OVERRIDE" ] || [ "$rebuilt" = true ] || writes_boot_target || [ -f "$marker" ] ||
        [ "$(cat "$marker.dns" 2>/dev/null)" != "$(target_nameserver)" ]; then
        return 0
    fi
    identity=$(ext2_source_identity) || return 0
    [ "$(cat "$marker.source.sha256" 2>/dev/null)" != "$identity" ]
}

# Record whether the data disk just wrapped carries a boot target.
mark_wrapped_ext2() {
    local marker="$1"
    if writes_boot_target; then echo "$BOOT_MODE $SUITE$PROGRAM${PROGRAM_ARG:+ $PROGRAM_ARG}" > "$marker"; else rm -f "$marker"; fi
    # Staging verifies the nameserver before the wrapped disk is published.
    target_nameserver > "$marker.dns"
    ext2_source_identity > "$marker.source.sha256"
}

# Cargo can restore a cached profile with an older mtime. Compare the exact
# kernel and loader contents, rather than dates, before reusing an EFI image.
efi_artifact_identity() {
    (set -o pipefail; shasum -a 256 "$KERNEL_ELF" "$LOADER_EFI" | awk '{print $1}') && echo "gpu-core=$GPU_CORE"
}

efi_artifacts_match() {
    local identity
    [ -f "$1" ] || return 1
    identity=$(efi_artifact_identity) || return 1
    [ "$(cat "$1")" = "$identity" ]
}

# Wait for a userspace completion record and reject missing or failing verdicts.
wait_boot_done() {
    python3 "$BREENIX_ROOT/scripts/wait-boot-done.py" "$SERIAL_LOG" "$BOOT_MODE" \
        --suite "$SUITE" --case "$ONLY" --timeout "$GATE_TIMEOUT" --started "$CLOCK_STARTED"
}

# Install before starting a VM. Ignore further signals during cleanup so Vigil
# receives the boot verdict even if stopping the VM fails or Ctrl-C is repeated.
bounded_vm_command() {
    python3 "$BREENIX_ROOT/scripts/bounded-vm-command.py" 30 "$@"
}

# F21: a name is not ownership. Keep crashed-run records until deletion succeeds.
vm_run_owner() {
    python3 "$BREENIX_ROOT/scripts/vm-run-owner.py" "$@"
}

delete_parallels_vm() {
    local vm="$1" info disk
    info=$(bounded_vm_command prlctl list -i "$vm") || return 1
    # These disks are reusable artifacts outside the VM bundle. Detach hdd0
    # and hdd1 when present before deleting our own VM or a dead owner's VM.
    for disk in hdd0 hdd1; do
        if grep -q "^[[:space:]]*$disk[[:space:]]" <<< "$info"; then
            bounded_vm_command prlctl set "$vm" --device-del "$disk" || return 1
        fi
    done
    bounded_vm_command prlctl delete "$vm"
}

sweep_parallels_vms() {
    local marker vm status
    for marker in "$PARALLELS_OWNER_DIR"/breenix-*.json; do
        [ -f "$marker" ] && [ ! -L "$marker" ] || continue
        vm_run_owner dead "$marker" || continue
        vm=${marker##*/}; vm=${vm%.json}
        status=$(bounded_vm_command prlctl status "$vm") || continue
        [[ "$status" == *" stopped" ]] || continue
        if delete_parallels_vm "$vm"; then rm -f "$marker"; fi
    done
}

sweep_vmware_bundles() {
    local bundle
    for bundle in "$VM_MACHINES"/breenix-*.vmwarevm; do
        [ -d "$bundle" ] && [ ! -L "$bundle" ] || continue
        vm_run_owner dead "$bundle/.breenix-run-process" || continue
        # Give the existing deletion path only the dead runner's exact bundle;
        # it separately checks the name marker and that no VMX inside is running.
        ( VM_BUNDLE="$bundle"; VM_NAME=${bundle##*/}; VM_NAME=${VM_NAME%.vmwarevm}
          VM_BUNDLE_OWNED=true; delete_vmware_bundle "$bundle" )
    done
}

# Read the VM's written disk only after stop has succeeded. VMware's deployed
# bundle, rather than the pristine staging VMDK, holds the guest's writes.
# A boot of several suites scores each of them here, the last with its disk checks.
score_vm_suite_disk() {
    (
        # Whole-suite disk checks depend on other cases and are not one-case evidence.
        [ -z "${ONLY:-}" ] || exit 0
        local manifest="$BREENIX_ROOT/docs/suites/${SUITE##*,}.json" checks disk="" temporary
        checks=$(python3 -c 'import json,sys; print(len(json.load(open(sys.argv[1])).get("diskChecks", [])))' "$manifest") || exit $?
        if [ "$checks" = 0 ]; then
            [[ "$SUITE" == *,* ]] || exit 0
            python3 "$BREENIX_ROOT/scripts/suite-sequence.py" score "$SUITE" "$SERIAL_LOG"
            exit $?
        fi
        if [ "$PARALLELS" = true ]; then
            disk=$(find "$EXT2_HDD_DIR" -name '*.hds' -type f) || exit $?
            if [ ! -f "$disk" ]; then
                echo "FAIL: expected one written Parallels ext2 image"
                exit 1
            fi
        else
            temporary=$(mktemp -d) || exit $?
            trap 'rm -rf "$temporary"' EXIT
            disk="$temporary/ext2.raw"
            bounded_vm_command qemu-img convert -f vmdk -O raw "$EXT2_VMDK_ARG" "$disk" || exit $?
        fi
        if [[ "$SUITE" == *,* ]]; then
            python3 "$BREENIX_ROOT/scripts/suite-sequence.py" score "$SUITE" "$SERIAL_LOG" --disk "$disk"
        else
            python3 "$BREENIX_ROOT/scripts/suite-verdict.py" "$manifest" "$SERIAL_LOG" --disk "$disk"
        fi
    )
}

# Only this runner's exclusively-created bundle may be deleted. Query all
# running VMX paths inside it, rather than guessing a filename.
vmware_bundle_stopped() {
    local bundle="$1" running
    running=$(bounded_vm_command "$VMRUN" list 2>/dev/null) || return 1
    while IFS= read -r vmx; do
        case "$vmx" in "$bundle"/*) return 1 ;; esac
    done <<< "$running"
}

delete_vmware_bundle() {
    local bundle="$1" vmx scratch lock output
    [ -d "$bundle" ] && [ ! -L "$bundle" ] || return 0
    [ "$bundle" = "${VM_BUNDLE:-}" ] && [ "${VM_BUNDLE_OWNED:-false}" = true ] || return 0
    [ -f "$bundle/.breenix-run-owner" ] || return 0
    [ "$(cat "$bundle/.breenix-run-owner")" = "$VM_NAME" ] || return 0
    vmware_bundle_stopped "$bundle" || { echo "WARNING: keeping running or unqueryable VM bundle $bundle"; return 0; }
    vmx="$bundle/$VM_NAME.vmx"
    scratch=$(mktemp -d /tmp/breenix-vmware-delete.XXXXXX) || return 0
    if [ -f "$vmx" ]; then
        for lock in "$bundle"/*.lck; do
            [ ! -e "$lock" ] || mv "$lock" "$scratch/" || true
        done
        if output=$(bounded_vm_command "$VMRUN" deleteVM "$vmx" 2>&1); then
            :
        else
            echo "WARNING: vmrun deleteVM kept $bundle: $output"
        fi
    else
        # An interrupted setup may have copied disks without writing a VMX.
        # Leave any other contents in place so reuse fails safely.
        rm -f "$bundle/boot.vmdk" "$bundle/ext2-data.vmdk"
    fi
    # Fusion removes its known VM files but preserves our ownership marker.
    # Remove only that marker, the window frame we wrote and an empty
    # directory, never unknown contents.
    if [ -d "$bundle" ] && [ ! -e "$vmx" ]; then
        rm -f "$bundle/.breenix-run-owner" "$bundle/.breenix-run-process" "$bundle/$VM_NAME.plist"
        rmdir "$bundle" 2>/dev/null || echo "WARNING: keeping extra files in $bundle"
    fi
    rm -rf "$scratch"
}

vmware_boot_name() {
    local identity
    if [ "${BREENIX_BOOT_NO_QUEUE:-0}" = 1 ]; then
        # An unqueued boot owns no slot identity and must not reuse its bundle.
        identity=$(uuidgen) || return $?
        printf 'breenix-unqueued-%s\n' "$identity"
    else
        identity=$(host_slot_number mac-boot) || return $?
        printf 'breenix-mac-boot-%s\n' "$identity"
    fi
}

prune_vmware_inventory() {
    python3 "$BREENIX_ROOT/scripts/vmware-inventory.py" \
        || echo "WARNING: could not prune VMware library"
}

cleanup_vmware_bundle() {
    delete_vmware_bundle "$VM_BUNDLE"
    prune_vmware_inventory
}

# Fusion opens a VM's window at the frame saved in <name>.plist in its bundle and, unless
# the VMX says otherwise, resizes the window to whatever display size the guest sets: a
# guest that sets a 256x256 screen gets a 256-point window. Write a frame whose content
# area shows WIDTH x HEIGHT points (shrunk to fit the screen, aspect kept), centred on the
# main screen; the VMX's stretch mode then scales any guest display into it.
write_vmware_window_frame() {
    local plist="$1" width="$2" height="$3" frame
    frame=$(/usr/bin/osascript -l JavaScript - "$width" "$height" <<'JS'
ObjC.import('AppKit');
function run(argv) {
    const chrome = 52;
    const screen = $.NSScreen.mainScreen.visibleFrame;
    const width = Number(argv[0]), height = Number(argv[1]);
    const scale = Math.min(1, screen.size.width / width, (screen.size.height - chrome) / height);
    const w = Math.floor(width * scale), h = Math.floor(height * scale) + chrome;
    const x = Math.floor(screen.origin.x + (screen.size.width - w) / 2);
    const y = Math.floor(screen.origin.y + (screen.size.height - h) / 2);
    return '{{' + x + ', ' + y + '}, {' + w + ', ' + h + '}}';
}
JS
    ) || return 1
    plutil -create xml1 "$plist" \
        && plutil -insert singleWindowFrame -string "$frame" "$plist" \
        && plutil -insert singleWindowIsZoomed -bool NO "$plist" \
        && echo "  Window: $frame"
}

finish_vm_boot() {
    local status=$?
    local stopped=true
    trap '' INT HUP TERM
    trap - EXIT
    if [ -n "${LOGMON_PID:-}" ]; then
        kill "$LOGMON_PID" 2>/dev/null || true
        wait "$LOGMON_PID" 2>/dev/null || true
        LOGMON_PID=""
    fi
    if [ -n "${GPU_TIME_PID:-}" ]; then
        kill "$GPU_TIME_PID" 2>/dev/null || true
        wait "$GPU_TIME_PID" 2>/dev/null || true
        GPU_TIME_PID=""
    fi
    if [ "$PARALLELS" = true ]; then
        prlctl stop "$PARALLELS_VM" --kill >/dev/null 2>&1 || stopped=false
    else
        bounded_vm_command "$VMRUN" stop "$VMX_FILE" hard >/dev/null 2>&1 || {
            vmware_bundle_stopped "$VM_BUNDLE" || stopped=false
        }
        vmware_check_network_log "$VM_BUNDLE/vmware.log"
        vmware_network_summary
    fi
    if [ "${BOOT_MODE:-}" = suite ] && { [ "$status" = 0 ] || [[ "$SUITE" == *,* ]]; }; then
        # Each suite of a sequence is scored even when the boot was stopped part way.
        if [ "$stopped" != true ]; then
            echo "FAIL: could not stop VM for suite disk checks"
            [ "$status" != 0 ] || status=1
        else
            score_vm_suite_disk || { [ "$status" != 0 ] || status=1; }
        fi
    fi
    host_slot_header "$SERIAL_LOG"
    "$BREENIX_ROOT/scripts/vigil-record.sh" finish "${VIGIL_ID:-}" "$status"
    if [ "$PARALLELS" = true ] && [ "$stopped" = true ]; then
        if delete_parallels_vm "$PARALLELS_VM"; then
            rm -f "${PARALLELS_OWNER_FILE:-}"
        else
            echo "WARNING: could not delete owned VM $PARALLELS_VM" >&2
        fi
        host_slot_release mac-boot
    elif [ "$PARALLELS" != true ] && [ "$stopped" = true ]; then
        # Delete and prune while the slot is still ours. The supervisor keeps
        # the lease through its final VM stop, so it cannot stop the next boot.
        cp "$VM_BUNDLE/vmware.log" "$SERIAL_LOG.vmware.log" 2>/dev/null || true
        delete_vmware_bundle "$VM_BUNDLE"
    fi
    [ "$PARALLELS" = true ] || prune_vmware_inventory
    exit "$status"
}

install_vm_boot_traps() {
    trap finish_vm_boot EXIT
    trap 'exit 130' INT
    trap 'exit 129' HUP
    trap 'exit 143' TERM
}

vm_is_running() {
    if [ "$PARALLELS" = true ]; then
        prlctl status "$PARALLELS_VM" 2>/dev/null | grep -q 'running'
    else
        "$VMRUN" list 2>/dev/null | grep -Fqx "$VMX_FILE"
    fi
}

follow_vm_serial() {
    local offset=0 size
    while vm_is_running; do
        if [ "$VMWARE" = true ]; then
            vmware_check_network_log "$VM_BUNDLE/vmware.log"
        fi
        size=$(wc -c < "$SERIAL_LOG" | tr -d ' ')
        if [ "$size" -gt "$offset" ]; then
            tail -c +$((offset + 1)) "$SERIAL_LOG"
            offset=$size
        fi
        sleep 1
    done
}

# BTRT mode: delegate to xtask and exit
if [ "$BTRT" = true ]; then
    host_slot_acquire mac-boot
    host_slot_quiet
    if [ "$ARCH" = "arm64" ]; then
        BTRT_ARCH="arm64"
    else
        BTRT_ARCH="x86_64"
    fi
    echo ""
    echo "========================================="
    echo "Breenix BTRT Boot Test ($BTRT_ARCH)"
    echo "========================================="
    echo ""
    exec cargo run -p xtask -- boot-test-btrt --arch "$BTRT_ARCH"
fi

# Parallels mode: build, deploy, boot on Parallels Desktop
if [ "$PARALLELS" = true ]; then
    echo ""
    echo "========================================="
    echo "Breenix on Parallels Desktop"
    echo "========================================="
    echo ""

    if ! command -v prlctl &>/dev/null; then
        echo "ERROR: prlctl not found. Is Parallels Desktop installed?"
        exit 1
    fi
    if ! command -v prl_disk_tool &>/dev/null; then
        echo "ERROR: prl_disk_tool not found. Is Parallels Desktop installed?"
        exit 1
    fi

    PARALLELS_DIR="$BREENIX_ROOT/target/parallels"
    SERIAL_LOG="${SERIAL_LOG_OVERRIDE:-/tmp/breenix-parallels-serial-$(date -u +%Y%m%dT%H%M%SZ)-$$.log}"
    HDD_DIR="$PARALLELS_DIR/breenix-efi.hdd"
    EXT2_HDD_DIR="$PARALLELS_DIR/breenix-ext2.hdd"
    EXT2_DISK="${DISK_OVERRIDE:-$BREENIX_ROOT/target/ext2-aarch64.img}"

    LOADER_EFI="$BREENIX_ROOT/target/aarch64-unknown-uefi/release/parallels-loader.efi"
    KERNEL_ELF="$BREENIX_ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"

    if [ "$NO_BUILD" = true ]; then
        echo "Skipping compilation (--no-build)"
        if [ ! -f "$LOADER_EFI" ] || [ ! -f "$KERNEL_ELF" ]; then
            echo "ERROR: No prebuilt loader/kernel found:"
            echo "  loader: $LOADER_EFI"
            echo "  kernel: $KERNEL_ELF"
            echo "Run without --no-build first to build them."
            exit 1
        fi
    else
        # Clean build caches if --clean was specified
        if [ "$CLEAN" = true ]; then
            echo "[0/4] Cleaning build caches..."
            cargo clean -p kernel 2>/dev/null || true
            cargo clean -p parallels-loader 2>/dev/null || true
        fi

        # Build the UEFI loader
        echo "[1/4] Building UEFI loader..."
        run_build cargo build --release --target aarch64-unknown-uefi -p parallels-loader

        # Build the kernel
        echo ""
        echo "[2/4] Building kernel..."
        run_build cargo build --release --target aarch64-breenix-kernel.json \
            -Z build-std=core,alloc \
            -Z build-std-features=compiler-builtins-mem \
            -p kernel --bin kernel-aarch64 ${KERNEL_FEATURE_ARGS[@]+"${KERNEL_FEATURE_ARGS[@]}"}

        if [ ! -f "$LOADER_EFI" ]; then
            echo "ERROR: UEFI loader not found at $LOADER_EFI"
            exit 1
        fi
        if [ ! -f "$KERNEL_ELF" ]; then
            echo "ERROR: Kernel not found at $KERNEL_ELF"
            exit 1
        fi

        if [ -z "$DISK_OVERRIDE" ]; then
            # Build userspace binaries and create ext2 data disk
            echo ""
            echo "[3/4] Building userspace binaries (aarch64)..."
            # A failure here is fatal: packing older ELFs under a new kernel is never a boot of this tree.
            if ! run_build "$BREENIX_ROOT/userspace/programs/build.sh" --arch aarch64; then
                echo "ERROR: Userspace build failed"
                exit 1
            fi

            echo ""
            echo "[4/4] Creating ext2 data disk image..."
            if ! run_build "$BREENIX_ROOT/scripts/create_ext2_disk.sh" --arch aarch64; then
                echo "ERROR: ext2 disk creation failed"
                exit 1
            fi
        fi
    fi

    host_slot_acquire mac-boot

    # -------------------------------------------------------------------
    # Deploy/package step: wraps the loader+kernel into the FAT32 ESP,
    # converts to a raw disk, and copies it into the Parallels .hdd.
    #
    # --no-build only skips COMPILATION above — it must NOT skip this
    # deploy step whenever a newer prebuilt image exists than what's
    # currently on disk, otherwise Parallels silently boots a stale .hds
    # (this voided a validation run: a freshly built kernel was never
    # copied in because --no-build short-circuited the whole pipeline).
    # Skip only when the existing EFI image contains these exact artifacts;
    # a cached production kernel may be older than the deployed testing one.
    # -------------------------------------------------------------------
    EXISTING_HDS=""
    if [ -d "$HDD_DIR" ]; then
        EXISTING_HDS=$(find "$HDD_DIR" -name "*.hds" 2>/dev/null | head -1)
    fi

    DEPLOY_NEEDED=true
    EXT2_SUITE_MARKER="$EXT2_HDD_DIR.suite"
    if [ "$NO_BUILD" = true ] && [ -n "$EXISTING_HDS" ] && [ -f "$EXISTING_HDS" ] \
        && efi_artifacts_match "$PARALLELS_DIR/efi-artifacts.sha256"; then
        DEPLOY_NEEDED=false
    fi

    if [ "$DEPLOY_NEEDED" = false ]; then
        echo "Existing Parallels disk ($EXISTING_HDS) already up to date with loader/kernel; skipping repackage"
        HDS_FILE="$EXISTING_HDS"
    else
        efi_artifact_identity >/dev/null || { echo "ERROR: selected kernel or loader is unreadable"; exit 1; }
        if [ "$NO_BUILD" = true ]; then
            echo "--no-build: deploying selected kernel and loader (artifact contents changed or no image exists)"
        fi

        echo ""
        echo "Creating FAT32 EFI disk image..."
        mkdir -p "$PARALLELS_DIR"

        # Create GPT+FAT32 disk image using hdiutil (native macOS, no mtools needed)
        DMG_PATH="$PARALLELS_DIR/efi-temp.dmg"
        rm -f "$DMG_PATH"
        hdiutil create -size 64m -fs FAT32 -volname BREENIX -layout GPTSPUD "$DMG_PATH" >/dev/null 2>&1

        # Mount the DMG, copy loader + kernel to ESP layout
        VOLUME=$(hdiutil attach "$DMG_PATH" 2>/dev/null | grep -o '/Volumes/[^ ]*' | head -1)
        if [ -z "$VOLUME" ] || [ ! -d "$VOLUME" ]; then
            echo "ERROR: Failed to mount FAT32 disk image"
            rm -f "$DMG_PATH"
            exit 1
        fi

        mkdir -p "$VOLUME/EFI/BOOT"
        mkdir -p "$VOLUME/EFI/BREENIX"
        cp "$LOADER_EFI" "$VOLUME/EFI/BOOT/BOOTAA64.EFI"
        cp "$KERNEL_ELF" "$VOLUME/EFI/BREENIX/KERNEL"
        [ "$GPU_CORE" = true ] && echo gpu-core > "$VOLUME/EFI/BREENIX/GPUCORE"
        hdiutil detach "$VOLUME" >/dev/null 2>&1

        echo "  Loader: $(stat -f%z "$LOADER_EFI") bytes"
        echo "  Kernel: $(stat -f%z "$KERNEL_ELF") bytes"
        echo "  GPU core: $GPU_CORE"

        # Convert DMG to raw disk image
        echo ""
        echo "Creating Parallels disks..."
        RAW_IMG="$PARALLELS_DIR/efi-raw.img"
        rm -f "$RAW_IMG" "${RAW_IMG}.cdr"
        hdiutil convert "$DMG_PATH" -format UDTO -o "$RAW_IMG" >/dev/null 2>&1
        mv "${RAW_IMG}.cdr" "$RAW_IMG"
        rm -f "$DMG_PATH"

        # Patch GPT partition type from "Microsoft Basic Data" to "EFI System Partition"
        # so UEFI firmware recognizes the ESP and auto-boots BOOTAA64.EFI
        python3 "$BREENIX_ROOT/scripts/parallels/patch-gpt-esp.py" "$RAW_IMG"

        # Wrap EFI disk in Parallels .hdd format
        rm -rf "$HDD_DIR"
        prl_disk_tool create --hdd "$HDD_DIR" --size 64M >/dev/null 2>&1
        HDS_FILE=$(find "$HDD_DIR" -name "*.hds" | head -1)
        if [ -z "$HDS_FILE" ]; then
            echo "ERROR: No .hds file found in $HDD_DIR"
            rm -f "$RAW_IMG"
            exit 1
        fi
        cp "$RAW_IMG" "$HDS_FILE"
        rm -f "$RAW_IMG"
        efi_artifact_identity > "$PARALLELS_DIR/efi-artifacts.sha256"
        echo "  EFI disk: $HDD_DIR"
    fi

    # Wrap the ext2 data disk in Parallels .hdd format (see ext2_rewrap_needed).
    if ! ext2_rewrap_needed "$DEPLOY_NEEDED" "$EXT2_SUITE_MARKER"; then
        echo "  ext2 disk: keeping $EXT2_HDD_DIR"
    elif stage_boot_ext2_disk "$EXT2_DISK" "$PARALLELS_DIR" && [ -f "$BOOT_EXT2_DISK" ]; then
        EXT2_SIZE_MB=$(( $(stat -f%z "$BOOT_EXT2_DISK") / 1048576 ))
        rm -rf "$EXT2_HDD_DIR"
        prl_disk_tool create --hdd "$EXT2_HDD_DIR" --size "${EXT2_SIZE_MB}M" >/dev/null 2>&1
        HDS_FILE=$(find "$EXT2_HDD_DIR" -name "*.hds" | head -1)
        if [ -z "$HDS_FILE" ]; then
            echo "WARNING: No .hds file in $EXT2_HDD_DIR, ext2 disk won't be attached"
        else
            cp "$BOOT_EXT2_DISK" "$HDS_FILE"
            mark_wrapped_ext2 "$EXT2_SUITE_MARKER"
            echo "  ext2 disk: $EXT2_HDD_DIR (${EXT2_SIZE_MB}MB)"
        fi
    else
        echo "WARNING: ext2 disk not found at $EXT2_DISK"
    fi

    echo ""
    echo "--- Configuring Parallels VM ---"

    # Use a unique VM name each time to avoid Parallels stale state issues.
    # Parallels can get stuck in "stopping"/"suspended" states that require
    # sudo service restarts. Using unique names sidesteps this entirely.
    PARALLELS_OWNER_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/breenix/vm-owners/parallels"
    sweep_parallels_vms
    PARALLELS_VM="breenix-$(date +%s)-$$"
    echo "VM name: $PARALLELS_VM"

    echo "Creating fresh VM '$PARALLELS_VM'..."
    prlctl create "$PARALLELS_VM" --ostype linux --distribution linux --no-hdd
    PARALLELS_OWNER_FILE="$PARALLELS_OWNER_DIR/$PARALLELS_VM.json"
    vm_run_owner record "$PARALLELS_OWNER_FILE" "$$"
    prlctl set "$PARALLELS_VM" --memsize 8192
    prlctl set "$PARALLELS_VM" --cpus 8
    # Forward modifier transitions for console chords (Parallels KB 119070).
    prlctl set "$PARALLELS_VM" --keyboard-optimize on
    # Run the vCPU threads in macOS's user-interactive QoS class, as the ARM64 QEMU launcher
    # does (scripts/qemu-ui-launch.sh). Parallels otherwise starts them in the default class,
    # priority 31, below the host's interactive work, and on a loaded Mac guest timer ticks
    # were taken milliseconds after they were due on most vCPUs at once (#1562). prl_vm_app
    # reads vm.vcpuprio as it starts each vCPU thread and maps 0-6 to a QoS class, 6 to
    # user-interactive (priority 46); with the flag set it also declines later requests to
    # change the class.
    PARALLELS_SYSTEM_FLAGS="vm.vcpuprio=6"
    prlctl set "$PARALLELS_VM" --system-flags "$PARALLELS_SYSTEM_FLAGS"
    # Parallels UEFI supplies UTC by default with an unspecified TimeZone.
    # Tools synchronization settings do not determine that firmware default.
    prlctl set "$PARALLELS_VM" --time-sync on --disable-timezone-sync on
    # Disable auto-suspend: default is "suspend" which halts the VM when Parallels
    # detects no display activity. Breenix runs headless/serial; we want it to
    # stay running until explicitly stopped.
    prlctl set "$PARALLELS_VM" --autostop shutdown 2>/dev/null || true
    # Disable app sharing; this is expected to reduce host app indexing activity.
    if ! prlctl set "$PARALLELS_VM" --sh-app-guest-to-host off --sh-app-host-to-guest off; then
        echo "WARNING: could not disable Parallels application sharing" >&2
    fi

    # Configure VM: EFI boot, attach our disks
    prlctl set "$PARALLELS_VM" --efi-boot on 2>/dev/null || true

    # VirtIO GPU with 3D acceleration (required for VirGL). BREENIX_PARALLELS_3D=off is the
    # host GPU time attestation's negative control (docs/gui/gpu.md section 1.6.5).
    PARALLELS_3D="${BREENIX_PARALLELS_3D:-highest}"
    case "$PARALLELS_3D" in
        highest|off) ;;
        *) echo "BREENIX_PARALLELS_3D must be highest or off, got '$PARALLELS_3D'"; exit 2 ;;
    esac
    prlctl set "$PARALLELS_VM" --3d-accelerate "$PARALLELS_3D"
    prlctl set "$PARALLELS_VM" --videosize 2048
    # The GPU core presents 2560x1600 scanouts (docs/gui/gpu.md section 1.7): with the
    # high-resolution policy they fill a 1280x800-point window at full resolution, as the
    # GL QEMU's window shows them.
    [ "$GPU_CORE" = true ] && RETINA=true
    # Select host and guest Retina policy explicitly before starting the VM. A new VM
    # defaults to high-resolution-in-guest=on, which makes Parallels show the guest at
    # one guest pixel per Retina pixel even with host high resolution off: the 1280x960
    # guest then opens in a 640x480-point window. The scaled default needs both off.
    if [ "${RETINA:-false}" = true ]; then
        RETINA_POLICY="high-resolution=on high-resolution-in-guest=on"
        prlctl set "$PARALLELS_VM" --high-resolution on --high-resolution-in-guest on
    else
        RETINA_POLICY="high-resolution=off high-resolution-in-guest=off"
        prlctl set "$PARALLELS_VM" --high-resolution off --high-resolution-in-guest off
    fi
    # With vertical sync on, Parallels answers RESOURCE_FLUSH on the host display's refresh,
    # and the GPU core's PRESENT returns that answer: alternating 2560x1600 presents ran at
    # 10 to 94 a second across four boots, with a 101.9 ms median flush answer in one of
    # them. With it off the median answer was 0.22 ms. GPU-core boots run with it off;
    # other boots keep it on.
    if [ "$GPU_CORE" = true ]; then
        prlctl set "$PARALLELS_VM" --vertical-sync off
    else
        prlctl set "$PARALLELS_VM" --vertical-sync on
    fi

    # Remove any default devices Parallels created (cdrom, hdd, serial) to avoid conflicts
    for dev in hdd0 hdd1 hdd2 cdrom0 cdrom1 serial0 serial1; do
        prlctl set "$PARALLELS_VM" --device-del "$dev" 2>/dev/null || true
    done

    # Attach EFI boot disk as hdd0 (sata:0)
    prlctl set "$PARALLELS_VM" --device-add hdd --image "$HDD_DIR" --type plain --position 0
    # Attach ext2 data disk as hdd1 (sata:1) — kernel probes all AHCI devices for ext2 magic
    if [ -d "$EXT2_HDD_DIR" ]; then
        prlctl set "$PARALLELS_VM" --device-add hdd --image "$EXT2_HDD_DIR" --type plain --position 1
        echo "  hdd0: EFI boot disk (FAT32) at sata:0"
        echo "  hdd1: ext2 data disk at sata:1"
    else
        echo "  hdd0: EFI boot disk (FAT32) at sata:0"
        echo "  WARNING: No ext2 disk to attach (run without --no-build to create it)"
    fi

    # Set boot order: hard disk first (default boots from cdrom which hangs)
    prlctl set "$PARALLELS_VM" --device-bootorder "hdd0" 2>/dev/null || true

    # Configure serial port output to file
    prlctl set "$PARALLELS_VM" --device-add serial --output "$SERIAL_LOG" 2>/dev/null || true
    # Serial port must be explicitly connected or it stays disconnected
    prlctl set "$PARALLELS_VM" --device-set serial0 --connect 2>/dev/null || true

    # Records the boot's start; a timing-sensitive suite first waits for a quiet host
    # (docs/boot-path.md, "Shared host slots").
    host_slot_quiet ${SUITE_MANIFESTS[@]+"${SUITE_MANIFESTS[@]}"}
    echo ""
    echo "--- Starting VM ---"
    rm -f "$SERIAL_LOG"  # Remove so VMware creates fresh (avoids append/replace prompt)
    VIGIL_ID=""
    install_vm_boot_traps
    record_launch_metadata
    # A one-case run is not registered: Vigil would score it as a suite boot.
    [ -n "$ONLY" ] || VIGIL_ID=$("$BREENIX_ROOT/scripts/vigil-record.sh" start parallels "$BOOT_MODE" "$SUITE" "$SERIAL_LOG")
    host_slot_vm "$SERIAL_LOG" prlctl stop "$PARALLELS_VM" --kill
    # Open the VM in its own window, as VMware's `gui` start does, so a boot can be watched;
    # BREENIX_PARALLELS_NOGUI=1 keeps unattended automation off the operator's desktop.
    if [ -n "${BREENIX_PARALLELS_NOGUI:-}" ]; then
        prlctl set "$PARALLELS_VM" --startup-view headless
    else
        prlctl set "$PARALLELS_VM" --startup-view window
    fi
    VM_INFO=$(prlctl list -i "$PARALLELS_VM") || { echo "ERROR: cannot query configured Parallels VM"; exit 1; }
    VM_HOME=$(sed -n 's/^Home: //p' <<< "$VM_INFO")
    [ -d "$VM_HOME" ] || { echo "ERROR: cannot locate configured Parallels VM"; exit 1; }
    grep -q "$RETINA_POLICY" <<< "$VM_INFO" || { echo "ERROR: Parallels VM does not report $RETINA_POLICY"; exit 1; }
    grep -q "<SystemFlags>$PARALLELS_SYSTEM_FLAGS</SystemFlags>" "$VM_HOME/config.pvs" ||
        { echo "ERROR: Parallels VM config does not carry system flags $PARALLELS_SYSTEM_FLAGS"; exit 1; }
    CLOCK_STARTED=$(python3 -c 'import time; print(time.time())')
    prlctl start "$PARALLELS_VM"
    # A full gpu suite's hardware claim rests on the VM process's host GPU time
    # (docs/gui/gpu.md section 1.6.5): sample it beside the serial log.
    GPU_TIME_RECORD=""
    if [[ ",$SUITE," == *",gpu,"* ]] && [ -z "$ONLY" ]; then
        GPU_TIME_RECORD="$SERIAL_LOG.gpu-time.jsonl"
        export BREENIX_GPU_TIME_JSONL="$GPU_TIME_RECORD"
        python3 "$BREENIX_ROOT/scripts/parallels/gpu-time.py" sample --vm "$PARALLELS_VM" \
            --serial "$SERIAL_LOG" --out "$GPU_TIME_RECORD" &
        GPU_TIME_PID=$!
    fi
    if [ "$BOOT_MODE" = suite ] || [ "$BOOT_MODE" = program ]; then
        # A suite case that needs a key asks on serial; the watcher presses it (scripts/hostkeys.py).
        python3 "$BREENIX_ROOT/scripts/hostkeys.py" watch --serial "$SERIAL_LOG" --backend "prlsdk:$PARALLELS_VM" \
            --log "$SERIAL_LOG.hostkeys" --parent "$$" > /dev/null 2>&1 &
    fi
    echo ""
    echo "========================================="
    echo "Breenix running on Parallels"
    echo "========================================="
    echo "VM:     $PARALLELS_VM"
    echo "Serial: $SERIAL_LOG"
    echo "Stop:   prlctl stop $PARALLELS_VM --kill"
    echo ""

    if [ "$BOOT_MODE" = probe ] || [ "$BOOT_MODE" = suite ] || [ "$BOOT_MODE" = tests ] || [ "$BOOT_MODE" = program ]; then
        wait_boot_done || exit $?
        if [ -n "$GPU_TIME_RECORD" ]; then
            python3 "$BREENIX_ROOT/scripts/parallels/gpu-time.py" judge "$GPU_TIME_RECORD" || exit 1
        fi
        if [ "$BOOT_MODE" = probe ] || [ "$BOOT_MODE" = tests ] || [ "$BOOT_MODE" = program ]; then exit 0; fi
        if [ "$PARALLELS_TEST" != true ]; then exit 0; fi
    fi

    if [ "$PARALLELS_TEST" = true ]; then
        # Test mode: wait, screenshot, exit
        SCREENSHOT="/tmp/breenix-screenshot.png"
        # #917 fix-pass C-5: propagate the capture outcome to our exit status.
        CAPTURE_OK=true
        echo "Test mode: waiting ${PARALLELS_TEST_WAIT}s for boot..."
        sleep "$PARALLELS_TEST_WAIT"

        echo ""
        echo "=== Screenshot ==="
        # capture-display.sh replaces the old screenshot-vm.sh + bare
        # `prlctl capture` fallback here: it tries `prlctl capture` (works
        # with no GUI window open, which is how this VM was started) then a
        # Core Graphics window capture as a best-effort secondary, retries
        # past a black VirGL-warmup frame, and writes OUTPUT from exactly one
        # verified-frame code path (7/7 tests in
        # tests/parallels_capture_structure.rs) -- on failure it writes
        # nothing and exits non-zero. See scripts/parallels/capture-display.sh
        # and docs/planning/green-program/gui/PARALLELS-CAPTURE-2026-09-07.md
        # (#917: the old fallback matched window titles that Parallels does
        # not set -- 0/12 non-empty in
        # docs/planning/green-program/gui/evidence/windowlist-no-vm.txt --
        # failed in 6/6 historical runs that reached this step, then
        # reported "Screenshot: ..."
        # success regardless of whether the fallback captured a black or
        # blank frame).
        CAPTURE_SCRIPT="$BREENIX_ROOT/scripts/parallels/capture-display.sh"
        # The caller already waited PARALLELS_TEST_WAIT for boot, so use a
        # short retry schedule here (only guarding late VirGL warmup) unless
        # the caller set its own.
        CAPTURE_STDOUT="$(mktemp "${TMPDIR:-/tmp}/breenix-capture-stdout.XXXXXX")"
        if BREENIX_CAPTURE_RETRY_SCHEDULE="${BREENIX_CAPTURE_RETRY_SCHEDULE:-5 10 20}" \
            "$CAPTURE_SCRIPT" "$PARALLELS_VM" "$SCREENSHOT" >"$CAPTURE_STDOUT"; then
            cat "$CAPTURE_STDOUT"
            CAPTURE_METHOD="$(grep -o 'method=[a-z]*' "$CAPTURE_STDOUT" | head -1 | cut -d= -f2)"
            echo "Screenshot: $SCREENSHOT (capture=${CAPTURE_METHOD:-unknown})"
        else
            CAPTURE_OK=false
            cat "$CAPTURE_STDOUT"
            echo "Screenshot: capture=none (no PNG written; see the [PARALLELS_CAPTURE:...] line above for why)"
        fi
        rm -f "$CAPTURE_STDOUT"

        echo ""
        echo "Serial log: $SERIAL_LOG"
    else
        # Interactive mode: tail serial forever
        echo "Tailing serial output (Ctrl+C stops the VM)..."
        echo ""

        # Monitor Parallels VM log for VCPU exceptions in background
        VM_LOG="$VM_HOME/parallels.log"
        if [ -f "$VM_LOG" ]; then
            python3 -u "$BREENIX_ROOT/scripts/vm-log-monitor.py" parallels "$VM_LOG" "$$" &
            LOGMON_PID=$!
        fi

        sleep 1
        follow_vm_serial
    fi

    # #917 fix-pass C-5: capture=none must also fail for exit-status callers.
    # claim-lint:ok: #917; tests/parallels_capture_structure.rs
    if [ "$PARALLELS_TEST" = true ] && [ "$CAPTURE_OK" != true ]; then
        exit 1
    fi
    exit 0
fi

# VMware Fusion mode: build, convert to VMDK, launch
if [ "$VMWARE" = true ]; then
    host_slot_acquire mac-boot
    echo ""
    echo "========================================="
    echo "Breenix on VMware Fusion"
    echo "========================================="
    echo ""

    VMRUN="/Applications/VMware Fusion.app/Contents/Public/vmrun"
    if [ ! -x "$VMRUN" ]; then
        echo "ERROR: vmrun not found. Is VMware Fusion installed?"
        exit 1
    fi
    prune_vmware_inventory

    # msg.autoAnswer dismisses Fusion's adapter errors and boots with Ethernet0
    # disconnected, even when the VMX requests a connected adapter.
    source "$BREENIX_ROOT/scripts/vmware-network.sh"

    VMWARE_DIR="$BREENIX_ROOT/target/vmware"
    SERIAL_LOG="${SERIAL_LOG_OVERRIDE:-/tmp/breenix-vmware-serial-$(date -u +%Y%m%dT%H%M%SZ)-$$.log}"
    # Fusion's "Use full resolution for Retina display": off by default so the guest is scaled 2x.
    if [ "${RETINA:-false}" = true ]; then VMWARE_NATIVE_RESOLUTION=TRUE; else VMWARE_NATIVE_RESOLUTION=FALSE; fi
    EXT2_DISK="${DISK_OVERRIDE:-$BREENIX_ROOT/target/ext2-aarch64.img}"
    VM_MACHINES="$HOME/Virtual Machines.localized"
    LOADER_EFI="$BREENIX_ROOT/target/aarch64-unknown-uefi/release/parallels-loader.efi"
    KERNEL_ELF="$BREENIX_ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"

    if [ "$NO_BUILD" = true ]; then
        echo "Skipping builds (--no-build)"
        if [ ! -f "$LOADER_EFI" ] || [ ! -f "$KERNEL_ELF" ]; then
            echo "ERROR: No prebuilt loader/kernel found:"
            echo "  loader: $LOADER_EFI"
            echo "  kernel: $KERNEL_ELF"
            echo "Run without --no-build first to build them."
            exit 1
        fi
    else
        # Clean build caches if --clean
        if [ "$CLEAN" = true ]; then
            echo "[0/6] Cleaning build caches..."
            cargo clean -p kernel 2>/dev/null || true
            cargo clean -p parallels-loader 2>/dev/null || true
        fi

        # Build UEFI loader
        echo "[1/6] Building UEFI loader..."
        run_build cargo build --release --target aarch64-unknown-uefi -p parallels-loader

        # Build kernel
        echo ""
        echo "[2/6] Building kernel..."
        run_build cargo build --release --target aarch64-breenix-kernel.json \
            -Z build-std=core,alloc \
            -Z build-std-features=compiler-builtins-mem \
            -p kernel --bin kernel-aarch64 ${KERNEL_FEATURE_ARGS[@]+"${KERNEL_FEATURE_ARGS[@]}"}

        if [ ! -f "$LOADER_EFI" ]; then
            echo "ERROR: UEFI loader not found at $LOADER_EFI"
            exit 1
        fi
        if [ ! -f "$KERNEL_ELF" ]; then
            echo "ERROR: Kernel not found at $KERNEL_ELF"
            exit 1
        fi

        if [ -z "$DISK_OVERRIDE" ]; then
            # Build userspace
            echo ""
            echo "[3/6] Building userspace binaries (aarch64)..."
            # A failure here is fatal: packing older ELFs under a new kernel is never a boot of this tree.
            if ! run_build "$BREENIX_ROOT/userspace/programs/build.sh" --arch aarch64; then
                echo "ERROR: Userspace build failed"
                exit 1
            fi

            # Create ext2 data disk
            echo ""
            echo "[4/6] Creating ext2 data disk image..."
            if ! run_build "$BREENIX_ROOT/scripts/create_ext2_disk.sh" --arch aarch64; then
                echo "ERROR: ext2 disk creation failed"
                exit 1
            fi
        fi

    fi

    if [ "$NO_BUILD" = false ] || [ ! -f "$VMWARE_DIR/boot.vmdk" ] \
        || ! efi_artifacts_match "$VMWARE_DIR/efi-artifacts.sha256"; then
        # Deploy the selected artifacts even when compilation was skipped.
        efi_artifact_identity >/dev/null || { echo "ERROR: selected kernel or loader is unreadable"; exit 1; }
        echo ""
        echo "[5/6] Creating FAT32 EFI disk image..."
        mkdir -p "$VMWARE_DIR"

        DMG_PATH="$VMWARE_DIR/efi-temp.dmg"
        rm -f "$DMG_PATH"
        hdiutil create -size 64m -fs FAT32 -volname BREENIX -layout GPTSPUD "$DMG_PATH" >/dev/null 2>&1

        VOLUME=$(hdiutil attach "$DMG_PATH" 2>/dev/null | grep -o '/Volumes/[^ ]*' | head -1)
        if [ -z "$VOLUME" ] || [ ! -d "$VOLUME" ]; then
            echo "ERROR: Failed to mount FAT32 disk image"
            rm -f "$DMG_PATH"
            exit 1
        fi

        mkdir -p "$VOLUME/EFI/BOOT"
        mkdir -p "$VOLUME/EFI/BREENIX"
        cp "$LOADER_EFI" "$VOLUME/EFI/BOOT/BOOTAA64.EFI"
        cp "$KERNEL_ELF" "$VOLUME/EFI/BREENIX/KERNEL"
        hdiutil detach "$VOLUME" >/dev/null 2>&1

        echo "  Loader: $(stat -f%z "$LOADER_EFI") bytes"
        echo "  Kernel: $(stat -f%z "$KERNEL_ELF") bytes"

        # Convert to raw then to VMDK
        echo ""
        echo "[6/6] Converting to VMDK..."
        RAW_IMG="$VMWARE_DIR/efi-raw.img"
        rm -f "$RAW_IMG" "${RAW_IMG}.cdr"
        hdiutil convert "$DMG_PATH" -format UDTO -o "$RAW_IMG" >/dev/null 2>&1
        mv "${RAW_IMG}.cdr" "$RAW_IMG"
        rm -f "$DMG_PATH"

        # Patch GPT partition type to EFI System Partition
        python3 "$BREENIX_ROOT/scripts/parallels/patch-gpt-esp.py" "$RAW_IMG"

        # Convert raw to VMDK
        qemu-img convert -f raw -O vmdk "$RAW_IMG" "$VMWARE_DIR/boot.vmdk"
        rm -f "$RAW_IMG"
        efi_artifact_identity > "$VMWARE_DIR/efi-artifacts.sha256"
        echo "  Boot VMDK: $VMWARE_DIR/boot.vmdk"
    fi

    # Convert the ext2 data disk to VMDK (see ext2_rewrap_needed).
    EXT2_SUITE_MARKER="$VMWARE_DIR/ext2-data.vmdk.suite"
    VMWARE_REBUILT=true
    [ "$NO_BUILD" = true ] && VMWARE_REBUILT=false
    if ! ext2_rewrap_needed "$VMWARE_REBUILT" "$EXT2_SUITE_MARKER"; then
        echo "  Data VMDK: keeping $VMWARE_DIR/ext2-data.vmdk"
    elif stage_boot_ext2_disk "$EXT2_DISK" "$VMWARE_DIR" && [ -f "$BOOT_EXT2_DISK" ]; then
        mkdir -p "$VMWARE_DIR"
        qemu-img convert -f raw -O vmdk "$BOOT_EXT2_DISK" "$VMWARE_DIR/ext2-data.vmdk"
        mark_wrapped_ext2 "$EXT2_SUITE_MARKER"
        echo "  Data VMDK: $VMWARE_DIR/ext2-data.vmdk"
    fi

    echo ""
    echo "--- Configuring VMware VM ---"

    # One library identity per leased slot, including after a crashed run.
    # The supervisor refuses a slot number unless this session holds its lease.
    VM_NAME="$(vmware_boot_name)"
    VM_BUNDLE="$VM_MACHINES/$VM_NAME.vmwarevm"
    VM_BUNDLE_OWNED=false
    sweep_vmware_bundles
    # Exclusive creation fails if a running, unowned or unknown-content bundle
    # survived deletion; cleanup ownership begins only after mkdir succeeds.
    mkdir "$VM_BUNDLE"
    printf '%s\n' "$VM_NAME" > "$VM_BUNDLE/.breenix-run-owner"
    VM_BUNDLE_OWNED=true
    vm_run_owner record "$VM_BUNDLE/.breenix-run-process" "$$"
    echo "VM name: $VM_NAME"
    trap cleanup_vmware_bundle EXIT
    trap 'exit 130' INT
    trap 'exit 129' HUP
    trap 'exit 143' TERM

    # Copy VMDKs into the bundle
    cp "$VMWARE_DIR/boot.vmdk" "$VM_BUNDLE/boot.vmdk"
    EXT2_VMDK_ARG=""
    if [ -f "$VMWARE_DIR/ext2-data.vmdk" ]; then
        cp "$VMWARE_DIR/ext2-data.vmdk" "$VM_BUNDLE/ext2-data.vmdk"
        EXT2_VMDK_ARG="$VM_BUNDLE/ext2-data.vmdk"
    fi

    # Generate VMX config inline (avoid subshell issues with command substitution)
    VMX_FILE="$VM_BUNDLE/$VM_NAME.vmx"
    cat > "$VMX_FILE" <<VMXEOF
.encoding = "UTF-8"
config.version = "8"
virtualHW.version = "22"

displayName = "$VM_NAME"
guestOS = "arm-ubuntu-64"
firmware = "efi"

numvcpus = "4"
memsize = "2048"
cpuid.coresPerSocket = "4"

pciBridge0.present = "TRUE"
pciBridge4.present = "TRUE"
pciBridge4.virtualDev = "pcieRootPort"
pciBridge4.functions = "8"
pciBridge5.present = "TRUE"
pciBridge5.virtualDev = "pcieRootPort"
pciBridge5.functions = "8"
pciBridge6.present = "TRUE"
pciBridge6.virtualDev = "pcieRootPort"
pciBridge6.functions = "8"
pciBridge7.present = "TRUE"
pciBridge7.virtualDev = "pcieRootPort"
pciBridge7.functions = "8"

nvme0.present = "TRUE"
nvme0:0.present = "TRUE"
nvme0:0.fileName = "$VM_BUNDLE/boot.vmdk"

sata0.present = "TRUE"
VMXEOF

    if [ -n "$EXT2_VMDK_ARG" ]; then
        cat >> "$VMX_FILE" <<VMXEOF

sata0:0.present = "TRUE"
sata0:0.fileName = "$EXT2_VMDK_ARG"
VMXEOF
    fi

    # Ethernet0 is bridged to the host's network, and the guest takes its address from that network's DHCP
    # server. Fusion's "nat" runs vmnet8 as a host-only segment that reaches outside only through Fusion's
    # vmnet-natd, a root service this host does not run (#1202). Its MAC is the host's static one, so the
    # server knows the client (scripts/vmware/static-mac.sh).
    VMWARE_MAC="$("$BREENIX_ROOT/scripts/vmware/static-mac.sh")" || { echo "ERROR: no static VMware MAC"; exit 1; }
    cat >> "$VMX_FILE" <<VMXEOF

serial0.present = "TRUE"
serial0.fileType = "file"
serial0.fileName = "$SERIAL_LOG"
serial0.yieldOnMsrRead = "TRUE"

ethernet0.present = "TRUE"
ethernet0.connectionType = "bridged"
ethernet0.virtualDev = "e1000e"
ethernet0.addressType = "static"
ethernet0.address = "$VMWARE_MAC"
ethernet0.linkStatePropagation.enable = "TRUE"

usb.present = "TRUE"
usb_xhci.present = "TRUE"
usb_xhci:4.present = "TRUE"
usb_xhci:4.deviceType = "hid"
usb_xhci:4.port = "4"
usb_xhci:6.present = "TRUE"
usb_xhci:6.deviceType = "mouse"
usb_xhci:6.port = "6"

svga.vramSize = "268435456"
VMXEOF

    # Host keys for a suite (scripts/hostkeys.py): Fusion's own VNC server takes them as RFB key events.
    if [ -n "${BREENIX_VMWARE_VNC_PORT:-}" ]; then
        [[ "$BREENIX_VMWARE_VNC_PORT" =~ ^[0-9]{4,5}$ ]] || { echo "BREENIX_VMWARE_VNC_PORT needs a port number"; exit 2; }
        cat >> "$VMX_FILE" <<VMXEOF
RemoteDisplay.vnc.enabled = "TRUE"
RemoteDisplay.vnc.port = "$BREENIX_VMWARE_VNC_PORT"
RemoteDisplay.vnc.ip = "127.0.0.1"
VMXEOF
    fi
    # A pointer measurement of the relative interface: with Fusion's absolute USB tablet
    # disabled, its VNC server sends the USB mouse the pointer's buttons and wheel. It sends
    # that mouse no motion (two measurement boots, with and without the gaming-mouse policy).
    if [ "${BREENIX_VMWARE_POINTER:-}" = relative ]; then
        cat >> "$VMX_FILE" <<VMXEOF
mouse.vusb.absDisabled = "TRUE"
VMXEOF
    elif [ -n "${BREENIX_VMWARE_POINTER:-}" ]; then
        echo "BREENIX_VMWARE_POINTER takes only 'relative'"; exit 2
    fi

    # Keep the default scaled viewport bounded; --retina retains GOP autodetection.
    if [ "${RETINA:-false}" != true ]; then
        cat >> "$VMX_FILE" <<VMXEOF
svga.autodetect = "FALSE"
svga.maxWidth = "1280"
svga.maxHeight = "960"
VMXEOF
    fi

    cat >> "$VMX_FILE" <<VMXEOF
gui.fitGuestUsingNativeDisplayResolution = "${VMWARE_NATIVE_RESOLUTION}"
gui.perVMWindowAutofitMode = "stretch"
gui.enableStretchGuest = "TRUE"
gui.stretchGuestMode = "aspect"
mks.enable3d = "TRUE"

vmci0.present = "TRUE"

# The guest treats an unspecified UEFI TimeZone as a UTC RTC contract.
# Fusion ARM's firmware clock needs startInUTC; diffFromUTC controls CMOS.
rtc.startInUTC = "TRUE"
rtc.diffFromUTC = "0"
tools.syncTime = "FALSE"
floppy0.present = "FALSE"
hpet0.present = "TRUE"
msg.autoAnswer = "TRUE"
VMXEOF

    echo "  VMX: $VMX_FILE ($(wc -c < "$VMX_FILE") bytes)"
    # The window shows the 1280x960 guest at two Retina pixels per guest pixel; the VMX's
    # stretch mode keeps it that size whatever display the guest sets.
    write_vmware_window_frame "$VM_BUNDLE/$VM_NAME.plist" 1280 960 \
        || { echo "ERROR: could not write the VM window frame"; exit 1; }

    # Records the boot's start; a timing-sensitive suite first waits for a quiet host
    # (docs/boot-path.md, "Shared host slots").
    host_slot_quiet ${SUITE_MANIFESTS[@]+"${SUITE_MANIFESTS[@]}"}
    echo ""
    echo "--- Starting VM ---"
    vmware_network_summary
    rm -f "$SERIAL_LOG"  # Remove so VMware creates fresh (avoids append/replace prompt)
    VIGIL_ID=""
    install_vm_boot_traps
    record_launch_metadata
    # A one-case run is not registered: Vigil would score it as a suite boot.
    [ -n "$ONLY" ] || VIGIL_ID=$("$BREENIX_ROOT/scripts/vigil-record.sh" start vmware "$BOOT_MODE" "$SUITE" "$SERIAL_LOG")
    host_slot_vm "$SERIAL_LOG" "$VMRUN" stop "$VMX_FILE" hard
    if [ -n "${BREENIX_VMWARE_NOGUI:-}" ]; then
        # An explicit headless request selects nogui for unattended callers.
        # The default branch below requires a successful GUI start.
        echo "BREENIX_VMWARE_NOGUI set: starting headless (nogui)"
        CLOCK_STARTED=$(python3 -c 'import time; print(time.time())')
        "$VMRUN" start "$VMX_FILE" nogui 2>&1 || true
    else
        CLOCK_STARTED=$(python3 -c 'import time; print(time.time())')
        "$VMRUN" start "$VMX_FILE" gui 2>&1 || {
            echo "ERROR: VMware GUI start failed"
            exit 1
        }
    fi

    if [ "$BOOT_MODE" = suite ] || [ "$BOOT_MODE" = program ]; then
        if [ -n "${BREENIX_VMWARE_VNC_PORT:-}" ]; then
            HOSTKEYS_BACKEND="vnc:127.0.0.1:$BREENIX_VMWARE_VNC_PORT"
        else
            HOSTKEYS_BACKEND=none
        fi
        python3 "$BREENIX_ROOT/scripts/hostkeys.py" watch --serial "$SERIAL_LOG" --backend "$HOSTKEYS_BACKEND" \
            --log "$SERIAL_LOG.hostkeys" --parent "$$" > /dev/null 2>&1 &
    fi
    echo ""
    echo "========================================="
    echo "Breenix running on VMware Fusion"
    echo "========================================="
    echo "VM:     $VM_NAME"
    echo "Serial: $SERIAL_LOG"
    echo "VMX:    $VMX_FILE"
    echo "Stop:   \"$VMRUN\" stop \"$VMX_FILE\" hard"
    echo ""
    if [ "$BOOT_MODE" = probe ] || [ "$BOOT_MODE" = suite ] || [ "$BOOT_MODE" = tests ] || [ "$BOOT_MODE" = program ]; then
        wait_boot_done || exit $?
        vmware_check_network_log "$VM_BUNDLE/vmware.log"
        vmware_network_summary
        if [ "$BOOT_MODE" = probe ] || [ "$BOOT_MODE" = tests ] || [ "$BOOT_MODE" = program ]; then exit 0; fi
        exit 0
    fi

    echo "Tailing serial output (Ctrl+C stops the VM)..."
    echo ""

    # Monitor vmware.log for CPU exceptions in background
    VM_LOG="$VM_BUNDLE/vmware.log"
    python3 -u "$BREENIX_ROOT/scripts/vm-log-monitor.py" vmware "$VM_LOG" "$$" &
    LOGMON_PID=$!
    # Keep VM cleanup installed while the serial tail runs.
    sleep 1
    follow_vm_serial

    exit 0
fi

# Route to architecture-specific runner
if [ "$ARCH" = "arm64" ]; then
    # ARM64 path - direct kernel boot
    KERNEL="$BREENIX_ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"
    EXT2_DISK="${DISK_OVERRIDE:-$BREENIX_ROOT/target/ext2-aarch64.img}"
    HOME_DISK="$BREENIX_ROOT/target/ext2-home-aarch64.img"

    # Build command for ARM64
    BUILD_CMD="cargo build --release --features boot_tests --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64"
else
    # x86_64 path - uses UEFI boot
    EXT2_DISK="${DISK_OVERRIDE:-$BREENIX_ROOT/target/ext2.img}"
    HOME_DISK="$BREENIX_ROOT/target/ext2-home.img"
    VNC_PORT=5900

    # Build command for x86_64: the production kernel, with the screen console
    # the other x86 profiles draw too; it boots into the shell (fw_cfg below).
    BUILD_CMD="cargo build --release --bin qemu-uefi"
fi

# Resolve display resolution
if [ -z "$RESOLUTION" ] && [ "$HEADLESS" = false ]; then
    # Auto-detect: use macOS screen size minus menu bar (37px)
    if [ "$(uname)" = "Darwin" ]; then
        SCREEN_INFO=$(system_profiler SPDisplaysDataType 2>/dev/null | grep "Resolution:" | head -1)
        if [[ "$SCREEN_INFO" =~ ([0-9]+)\ x\ ([0-9]+) ]]; then
            NATIVE_W="${BASH_REMATCH[1]}"
            NATIVE_H="${BASH_REMATCH[2]}"
            # Retina displays: divide by 2 for effective resolution
            if echo "$SCREEN_INFO" | grep -q "Retina"; then
                NATIVE_W=$((NATIVE_W / 2))
                NATIVE_H=$((NATIVE_H / 2))
            fi
            # Subtract menu bar height
            RES_W="$NATIVE_W"
            RES_H=$((NATIVE_H - 37))
            RESOLUTION="${RES_W}x${RES_H}"
        fi
    fi
fi
# Fallback default
if [ -z "$RESOLUTION" ]; then
    RESOLUTION="1280x800"
fi

# Parse WxH
RES_W="${RESOLUTION%%x*}"
RES_H="${RESOLUTION##*x}"

echo ""
echo "========================================="
echo "Breenix Interactive Mode"
echo "========================================="
echo ""
echo "Architecture: $ARCH"
echo "Resolution: ${RES_W}x${RES_H}"
echo "Mode: $([ "$HEADLESS" = true ] && echo "headless (serial only)" || echo "graphics")"

if [ "$NO_BUILD" = true ]; then
    echo "Skipping all builds (--no-build)"
elif [ -n "$DISK_OVERRIDE" ]; then
    echo "Using supplied disk: $EXT2_DISK"
    echo "Building kernel..."
    run_build /bin/bash -c "$BUILD_CMD"
elif [ "$CLEAN" = true ]; then
    # --clean: full rebuild of userspace, system ext2 disk, and kernel
    echo ""
    echo "Clean build: rebuilding everything..."
    echo ""

    if [ "$ARCH" = "arm64" ]; then
        echo "[1/3] Building userspace binaries (aarch64)..."
        run_build "$BREENIX_ROOT/userspace/programs/build.sh" --arch aarch64

        echo ""
        echo "[2/3] Creating system ext2 disk image..."
        run_build "$BREENIX_ROOT/scripts/create_ext2_disk.sh" --arch aarch64

        echo ""
        echo "[3/3] Building kernel..."
    else
        # create-test-disk builds userspace, then packs target/test_binaries.img.
        echo "[1/3] Building userspace binaries and test disk (x86_64)..."
        (cd "$BREENIX_ROOT" && run_build cargo run -p xtask -- create-test-disk)

        echo ""
        echo "[2/3] Creating system ext2 disk image..."
        run_build "$BREENIX_ROOT/scripts/create_ext2_disk.sh"

        echo ""
        echo "[3/3] Building kernel..."
    fi
    run_build /bin/bash -c "$BUILD_CMD"
    echo ""
else
    # Userspace preparation belongs to the launcher, never kernel/build.rs.
    if [ "$ARCH" = "arm64" ]; then
        echo "Building userspace binaries (aarch64)..."
        run_build "$BREENIX_ROOT/userspace/programs/build.sh" --arch aarch64
        USERSPACE_MANIFEST="$BREENIX_ROOT/userspace/programs/aarch64/.userspace-manifest.json"
        EXT2_ARGS=(--arch aarch64)
    else
        # create-test-disk builds userspace, then packs target/test_binaries.img.
        echo "Building userspace binaries and test disk (x86_64)..."
        (cd "$BREENIX_ROOT" && run_build cargo run -p xtask -- create-test-disk)
        USERSPACE_MANIFEST="$BREENIX_ROOT/userspace/programs/.userspace-manifest.json"
        EXT2_ARGS=()
    fi
    # build.sh rewrites its manifest only when the installed ELFs or their
    # sources change, so an unchanged system disk (and the session disk cloned
    # from it below) survives an incremental run.
    if [ ! -f "$EXT2_DISK" ] || [ -n "$(find "$USERSPACE_MANIFEST" \
            "$BREENIX_ROOT/scripts/create_ext2_disk.sh" "$BREENIX_ROOT/scripts/install-libc-busybox.sh" \
            "$(dirname "$USERSPACE_MANIFEST")/libc-busybox.elf" \
            "$(dirname "$USERSPACE_MANIFEST")/libc-busybox.links" \
            "$BREENIX_ROOT/fonts" "$BREENIX_ROOT/vendor/busybox" -newer "$EXT2_DISK" -print -quit)" ]; then
        echo "Creating system ext2 disk image..."
        run_build "$BREENIX_ROOT/scripts/create_ext2_disk.sh" ${EXT2_ARGS[@]+"${EXT2_ARGS[@]}"}
    else
        echo "System ext2 disk image is current"
    fi
    # Always rebuild to ensure correct features (boot_tests, etc.)
    # Cargo is incremental — this is fast if nothing changed
    echo "Building kernel..."
    run_build /bin/bash -c "$BUILD_CMD"
fi

# Create home disk if it doesn't exist (never recreated unless --rebuild-home)
if [ "$REBUILD_HOME" = true ] || [ ! -f "$HOME_DISK" ]; then
    if [ "$REBUILD_HOME" = true ] && [ -f "$HOME_DISK" ]; then
        echo ""
        echo "Rebuilding home disk (--rebuild-home)..."
    elif [ ! -f "$HOME_DISK" ]; then
        echo ""
        echo "Creating home disk (first run)..."
    fi
    if [ "$ARCH" = "arm64" ]; then
        "$BREENIX_ROOT/scripts/create_home_disk.sh" --arch aarch64
    else
        "$BREENIX_ROOT/scripts/create_home_disk.sh"
    fi
fi

if [ "$ARCH" != "arm64" ]; then
    # The UEFI image is a build product, so look for it only once the build has run.
    UEFI_IMG=$(ls -t "$BREENIX_ROOT/target/release/build/breenix-"*/out/breenix-uefi.img 2>/dev/null | head -1)
    KERNEL="$UEFI_IMG"  # Reuse KERNEL var for existence check
fi

if [ ! -f "$KERNEL" ]; then
    echo "Error: Kernel not found after build"
    exit 1
fi

echo "Kernel: $KERNEL"

# Create output directory
OUTPUT_DIR=$(mktemp -d)
echo "Serial output: $OUTPUT_DIR/serial.txt"

# Add ext2 disk if it exists (writable copy to allow filesystem writes)
# Use a known path so extraction scripts can find the session disk
DISK_OPTS=""
EXT2_SESSION="$BREENIX_ROOT/target/ext2-session.img"
if [ -f "$EXT2_DISK" ]; then
    echo "Disk image: $EXT2_DISK"
    EXT2_WRITABLE="$EXT2_SESSION"
    # The stamp names the system disk the session was cloned from. A session from
    # the other architecture, or from a system disk rebuilt since, is replaced.
    EXT2_SESSION_BASE="$EXT2_SESSION.base"
    if [ -n "$DISK_OVERRIDE" ] || [ "$CLEAN" = true ] || [ ! -f "$EXT2_WRITABLE" ] || [ ! -f "$EXT2_SESSION_BASE" ] \
            || [ "$(cat "$EXT2_SESSION_BASE")" != "$EXT2_DISK" ] || [ "$EXT2_DISK" -nt "$EXT2_SESSION_BASE" ]; then
        echo "Creating fresh session disk from $EXT2_DISK"
        cp "$EXT2_DISK" "$EXT2_WRITABLE"
        echo "$EXT2_DISK" > "$EXT2_SESSION_BASE"
    else
        echo "Reusing existing session disk (use --clean to reset)"
    fi
    if [ "$ARCH" = "arm64" ]; then
        if [ "$USE_AHCI" = true ]; then
            # AHCI (SATA) disk — reproduces Parallels AHCI interrupt behavior on QEMU
            DISK_OPTS="-device ahci,id=ahci0 -device ide-hd,drive=ext2disk,bus=ahci0.0 -drive id=ext2disk,file=$EXT2_WRITABLE,format=raw,if=none"
            echo "  Using AHCI controller for ext2 disk"
        else
            DISK_OPTS="-device virtio-blk-device,drive=ext2disk -drive if=none,id=ext2disk,format=raw,file=$EXT2_WRITABLE"
        fi
    else
        # x86_64 uses virtio-blk-pci for UEFI compatibility
        DISK_OPTS="-drive if=none,id=ext2disk,format=raw,file=$EXT2_WRITABLE -device virtio-blk-pci,drive=ext2disk,disable-modern=on,disable-legacy=off"
    fi
else
    echo "Disk image: (none - shell commands will be limited)"
    if [ "$ARCH" = "arm64" ]; then
        if [ "$USE_AHCI" = true ]; then
            DISK_OPTS="-device ahci,id=ahci0 -device ide-hd,drive=hd0,bus=ahci0.0 -drive id=hd0,file=/dev/null,format=raw,if=none"
        else
            DISK_OPTS="-device virtio-blk-device,drive=hd0 -drive if=none,id=hd0,format=raw,file=/dev/null"
        fi
    fi
fi

# Add home disk if it exists (writable copy for user data persistence)
HOME_DISK_OPTS=""
HOME_SESSION="$BREENIX_ROOT/target/ext2-home-session.img"
if [ -f "$HOME_DISK" ]; then
    echo "Home disk: $HOME_DISK"
    if [ "$REBUILD_HOME" = true ] || [ ! -f "$HOME_SESSION" ]; then
        echo "Creating fresh home session disk from $HOME_DISK"
        cp "$HOME_DISK" "$HOME_SESSION"
    else
        echo "Reusing existing home session disk (use --rebuild-home to reset)"
    fi
    if [ "$ARCH" = "arm64" ]; then
        if [ "$USE_AHCI" = true ]; then
            # Home disk on AHCI port 1 (same controller as ext2 disk on port 0)
            HOME_DISK_OPTS="-device ide-hd,drive=homedisk,bus=ahci0.1 -drive id=homedisk,file=$HOME_SESSION,format=raw,if=none"
        else
            HOME_DISK_OPTS="-device virtio-blk-device,drive=homedisk -drive if=none,id=homedisk,format=raw,file=$HOME_SESSION"
        fi
    else
        HOME_DISK_OPTS="-drive if=none,id=homedisk,format=raw,file=$HOME_SESSION -device virtio-blk-pci,drive=homedisk,disable-modern=on,disable-legacy=off"
    fi
fi

# Build display options based on architecture and headless mode
if [ "$ARCH" = "arm64" ]; then
    # ARM64: Always add GPU and keyboard devices (needed for VirtIO enumeration)
    # The -display option only controls whether a window appears
    # Resolution is configured by the kernel's virtio-gpu driver via fw_cfg
    if [ "$HEADLESS" = true ]; then
        DISPLAY_OPTS="-display none -device virtio-gpu-device -device virtio-keyboard-device -device virtio-tablet-device"
    else
        DISPLAY_OPTS="-display cocoa -device virtio-gpu-device -device virtio-keyboard-device -device virtio-tablet-device"
    fi
else
    # x86_64 uses virtio-vga
    if [ "$HEADLESS" = true ]; then
        DISPLAY_OPTS="-display none"
    else
        DISPLAY_OPTS="-device virtio-vga -vnc :0 -k en-us"
    fi
fi

if [ "$HEADLESS" = true ]; then
    echo ""
    echo "Running in headless mode. Serial output will appear below."
    echo "Press Ctrl+C to stop."
    echo ""
else
    echo ""
    if [ "$ARCH" = "arm64" ]; then
        echo "Opening native window (cocoa display)..."
    else
        echo "VNC available at: localhost:$VNC_PORT"
    fi
    echo "Press Ctrl+C to stop."
    echo ""
fi

# Audio options (VirtIO sound device)
# NOTE: virtio-sound normally creates BOTH a playback (out) and a capture (in)
# PCM stream. macOS coreaudio can open the playback stream fine, but cannot
# open a capture stream in this environment ("Can not open `virtio-sound.in'
# (no host audio driver)"), which aborts device realization and QEMU never
# boots. streams=1 limits the device to just the playback stream, keeping
# audio OUTPUT working while dropping the failing capture stream.
if [ "$NO_AUDIO" = true ]; then
    AUDIO_OPTS="-audiodev none,id=audio0"
else
    AUDIO_OPTS="-audiodev coreaudio,id=audio0"
    if [ "$ARCH" = "arm64" ]; then
        AUDIO_OPTS="$AUDIO_OPTS -device virtio-sound-device,audiodev=audio0,streams=1"
    else
        AUDIO_OPTS="$AUDIO_OPTS -device virtio-sound-pci,audiodev=audio0,streams=1"
    fi
fi

# QMP socket for programmatic VM control (always enabled)
QMP_SOCK="/tmp/breenix-qmp.sock"
rm -f "$QMP_SOCK"
QMP_OPTS="-qmp unix:${QMP_SOCK},server,nowait"

# GDB stub (--debug flag)
# -s = GDB server on localhost:1234
# -S = halt CPU at startup (wait for GDB to continue)
GDB_OPTS=""
if [ "$DEBUG" = true ]; then
    GDB_OPTS="-s -S"
    echo "GDB stub: target remote :1234 (CPU halted, waiting for GDB)"
fi

# Pass resolution to kernel via fw_cfg
FW_CFG_OPTS="-fw_cfg name=opt/breenix/resolution,string=${RES_W}x${RES_H}"

# Build the full QEMU command based on architecture
if [ "$ARCH" = "arm64" ]; then
    # ARM64 QEMU invocation (native)
    # NOTE: Home disk must come BEFORE system disk because QEMU virt assigns
    # MMIO addresses in reverse command-line order. The kernel discovers devices
    # by scanning MMIO addresses low-to-high, so the LAST device here gets the
    # lowest address and becomes device 0 (the system/root disk).
    #
    # #834: this is an interactive display session (Ctrl+C stops QEMU below),
    # but it still takes the host-wide lock -- not exempt just because a
    # human, not a gate, is driving it. qemu_host_lock_acquire prints its own
    # one-line notice (host count, then a wait message if contended) before
    # blocking.
    host_slot_acquire mac-boot
    qemu_host_lock_acquire
    host_slot_quiet
    python3 "$BREENIX_ROOT/scripts/qemu-arm64.py" qemu-system-aarch64 \
        -M virt,gic-version=3 -cpu max -smp 4 \
        -m 512M \
        -kernel "$KERNEL" \
        $DISPLAY_OPTS \
        $HOME_DISK_OPTS \
        $DISK_OPTS \
        -device virtio-net-device,netdev=net0 \
        -netdev user,id=net0,hostfwd=tcp::2323-:2323,hostfwd=tcp::7890-:7890 \
        $AUDIO_OPTS \
        -monitor tcp:127.0.0.1:4444,server,nowait \
        $QMP_OPTS \
        $GDB_OPTS \
        $FW_CFG_OPTS \
        -serial mon:stdio \
        -no-reboot \
        &
else
    # x86_64 QEMU invocation (UEFI boot, native)
    # Fetch the pinned OVMF firmware on first use (a fresh checkout has none cached).
    if [ ! -f "$BREENIX_ROOT/target/ovmf/x64/code.fd" ] || [ ! -f "$BREENIX_ROOT/target/ovmf/x64/vars.fd" ]; then
        if [ ! -x "$BREENIX_ROOT/target/release/qemu-uefi" ]; then
            echo "Error: OVMF firmware is not cached and target/release/qemu-uefi, which fetches it, is not built"
            exit 1
        fi
        echo "Fetching OVMF firmware into target/ovmf..."
        (cd "$BREENIX_ROOT" && BREENIX_FETCH_OVMF=1 "$BREENIX_ROOT/target/release/qemu-uefi")
    fi
    # Copy OVMF firmware to output dir (pflash needs writable files)
    cp "$BREENIX_ROOT/target/ovmf/x64/code.fd" "$OUTPUT_DIR/OVMF_CODE.fd"
    cp "$BREENIX_ROOT/target/ovmf/x64/vars.fd" "$OUTPUT_DIR/OVMF_VARS.fd"

    # Build test binaries options if it exists
    TEST_BIN_OPTS=""
    if [ -f "$BREENIX_ROOT/target/test_binaries.img" ]; then
        TEST_BIN_OPTS="-drive if=none,id=testdisk,format=raw,readonly=on,file=$BREENIX_ROOT/target/test_binaries.img -device virtio-blk-pci,drive=testdisk,disable-modern=on,disable-legacy=off"
    fi

    # #865: this is an interactive display session (Ctrl+C stops QEMU
    # below) like the arm64 branch above, and takes the same host-wide
    # lock -- one lock domain per QEMU binary, so this qemu-system-x86_64
    # boot cooperates with a concurrent x86 gate lane without contending
    # with a concurrent arm64 session.
    host_slot_acquire mac-boot
    qemu_host_lock_acquire qemu-system-x86_64
    host_slot_quiet
    qemu-system-x86_64 \
        -pflash "$OUTPUT_DIR/OVMF_CODE.fd" \
        -pflash "$OUTPUT_DIR/OVMF_VARS.fd" \
        -drive if=none,id=hd,format=raw,media=disk,readonly=on,file="$UEFI_IMG" \
        -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
        $TEST_BIN_OPTS \
        $DISK_OPTS \
        $HOME_DISK_OPTS \
        -machine pc,accel=tcg \
        -cpu qemu64 \
        -smp 4 \
        -m 512 \
        $DISPLAY_OPTS \
        -no-reboot \
        -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
        -netdev user,id=net0 \
        -device e1000,netdev=net0,mac=52:54:00:12:34:56 \
        $AUDIO_OPTS \
        -monitor tcp:127.0.0.1:4444,server,nowait \
        $QMP_OPTS \
        $GDB_OPTS \
        -fw_cfg name=opt/breenix/mode,string=shell \
        -serial mon:stdio \
        -serial "file:$OUTPUT_DIR/serial.txt" \
        &
fi

QEMU_PID=$!
# F2 (#835 idiom): registers QEMU with the lock's own EXIT trap (see
# docker/qemu/lib/qemu-host-lock.sh) so a SIGTERM/SIGINT delivered to just
# this script's own PID still kills QEMU instead of orphaning it with the
# lock free. Both branches above now acquire a lock domain (#865), so this
# is no longer arm64-only.
qemu_host_lock_track_pid "$QEMU_PID"

echo "Paste:     echo 'code' | ./scripts/paste.sh"
echo "Monitor:   tcp://127.0.0.1:4444"
echo "QMP:       $QMP_SOCK"
if [ "$DEBUG" = true ]; then
    echo "GDB:       target remote :1234"
fi
echo "Forensics: scripts/forensic-capture.sh"

# If x86_64 graphics mode, try to open VNC viewer
if [ "$ARCH" = "x86_64" ] && [ "$HEADLESS" = false ] && [ "$(uname)" = "Darwin" ]; then
    sleep 2  # Give QEMU time to start
    if [ -d "/Applications/TigerVNC Viewer 1.15.0.app" ]; then
        echo "Opening TigerVNC..."
        open "/Applications/TigerVNC Viewer 1.15.0.app" --args "localhost:$VNC_PORT"
    else
        echo ""
        echo "TigerVNC not found. Connect manually to localhost:$VNC_PORT"
        echo "Install from: https://github.com/TigerVNC/tigervnc/releases"
    fi
fi

# Wait for QEMU to finish
QEMU_STATUS=0
wait "$QEMU_PID" 2>/dev/null || QEMU_STATUS=$?

echo ""
echo "========================================="
echo "Breenix stopped"
echo "========================================="
echo "Serial output saved to: $OUTPUT_DIR/serial.txt"
if [ -f "$EXT2_SESSION" ]; then
    echo "System session disk: $EXT2_SESSION"
fi
if [ -f "$HOME_SESSION" ]; then
    echo "Home session disk: $HOME_SESSION"
    echo "Extract saved files: ./scripts/extract-saves.sh"
fi

exit "$QEMU_STATUS"
