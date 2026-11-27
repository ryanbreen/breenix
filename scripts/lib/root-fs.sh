#!/usr/bin/env bash
# Prepare the staged root and construct the existing QEMU root drive arguments.
root_fs_prepare() {
    if [ "$ROOT_FS" = breenfs ]; then
        if [ "$MODE" != suite ] || [[ ",$SUITE," != *,filesystem,* ]]; then
            run_build cargo build --release --manifest-path "$ROOT/tools/breenfs/Cargo.toml" --bin breenfs-extract
        fi
        local breenfs_root="${WRITABLE%/*}/breenfs-writable.img"
        run_build bash "$ROOT/scripts/create-breenfs-root.sh" "$WRITABLE" "$breenfs_root"
        WRITABLE="$breenfs_root"
    fi
    ROOT_DRIVE="if=none,id=ext2,format=raw,file=$WRITABLE"
    if [ "$MODE" != tests ]; then
        ROOT_DRIVE+=",cache=unsafe,aio=threads"
    fi
}

root_fs_scratch() {
    BREENFS_SCRATCH_ARGS=()
    local scratch
    if [ "$MODE" = suite ] && [[ "${MILESTONE:-}" = toolchain/busybox-on-box || "${MILESTONE:-}" = toolchain/kernel-assembly ]]; then
        scratch="${WRITABLE%/*}/toolchain-scratch.img"
    elif [ "$MODE" = suite ] && [[ ",$SUITE," == *,self-host,* ]]; then
        scratch="${WRITABLE%/*}/self-host-scratch.img"
    elif [ "$MODE" = suite ] && [[ ",$SUITE," == *,filesystem,* ]]; then
        scratch="${WRITABLE%/*}/breenfs-scratch.img"
    else
        return 0
    fi
    if [ -n "$scratch" ]; then
        # QEMU allocates MMIO downward, so discovery finds scratch after the other fixtures.
        BREENFS_SCRATCH_ARGS=(-device virtio-blk-device,drive=breenfs-scratch
            -drive "if=none,id=breenfs-scratch,format=raw,file=$scratch")
    fi
}
