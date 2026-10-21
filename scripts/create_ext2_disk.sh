#!/bin/bash
# Create ext2 disk image for Breenix kernel testing
#
# This script creates an ext2 filesystem image (256MB default) with:
#   - Test files for filesystem testing
#   - Breenix-libc BusyBox in /bin/busybox with hard links for its applets
#   - hello_world binary for exec testing
#   - Test binaries in /usr/local/test/bin
#
# Requires Docker on macOS (or mke2fs on Linux).
#
# Usage:
#   ./scripts/create_ext2_disk.sh
#   ./scripts/create_ext2_disk.sh --arch aarch64
#   ./scripts/create_ext2_disk.sh --arch aarch64 --size 128
#
# --size below what the current userspace binaries + fonts payload needs is
# rejected up front with the computed shortfall (#850) rather than failing
# partway through the copy step with ENOSPC -- omit --size to get the
# 256MB default, which comfortably fits the payload on both architectures
# as of this writing.
#
# Or use xtask:
#   cargo run -p xtask -- create-ext2-disk

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TARGET_DIR="$PROJECT_ROOT/target"
SIZE_MB=256

DNS_SERVER="10.0.2.3"
ARCH="x86_64"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --arch)
            ARCH="$2"
            shift 2
            ;;
        --dns-server)
            DNS_SERVER="$2"
            shift 2
            ;;
        --size)
            SIZE_MB="$2"
            shift 2
            ;;
        *)
            echo "Usage: $0 [--arch x86_64|aarch64] [--size MB]"
            exit 1
            ;;
    esac
done

if [[ "$ARCH" == "aarch64" ]]; then
    USERSPACE_DIR="$PROJECT_ROOT/userspace/programs/aarch64"
    OUTPUT_FILE="$TARGET_DIR/ext2-aarch64.img"
    TESTDATA_FILE="$PROJECT_ROOT/testdata/ext2-aarch64.img"
    # ARM64 uses the same 256MB default as x86_64
else
    USERSPACE_DIR="$PROJECT_ROOT/userspace/programs"
    OUTPUT_FILE="$TARGET_DIR/ext2.img"
    TESTDATA_FILE="$PROJECT_ROOT/testdata/ext2.img"
fi

# Both native and gate disks use the exact output of the local libc build.
# Required-applet validation rejects an incomplete BusyBox build.
python3 "$SCRIPT_DIR/userspace-manifest.py" check --arch "$ARCH"
"$SCRIPT_DIR/install-libc-busybox.sh" --check "$USERSPACE_DIR" "$PROJECT_ROOT/vendor/busybox/required-applets"

echo "Creating ext2 disk image..."
echo "  Arch: $ARCH"
echo "  Output: $OUTPUT_FILE"
echo "  Size: ${SIZE_MB}MB"

# --- #850: refuse a --size too small for the real payload -----------------
#
# This script's copy step installs the *.elf files directly under
# $USERSPACE_DIR (top-level glob, matching the `for elf_file in .../*.elf`
# loops below) plus the *.ttf files under $PROJECT_ROOT/fonts, independent
# of --size. A requested size smaller than that payload used to fail
# partway through the Docker copy step with a bare "No space left on
# device" from `cp`, after the image had already been created and
# partially populated (#850). Compute the payload up front and fail
# loudly, before touching Docker/mke2fs at all, rather than truncating an
# image mid-copy.
file_size_bytes() {
    # Portable stat: BSD/macOS uses -f%z, GNU/Linux uses -c%s.
    stat -f%z "$1" 2>/dev/null || stat -c%s "$1"
}

payload_bytes=0
for f in "$USERSPACE_DIR"/*.elf "$PROJECT_ROOT/fonts"/*.ttf; do
    [[ -f "$f" ]] || continue
    payload_bytes=$(( payload_bytes + $(file_size_bytes "$f") ))
done

# Headroom arithmetic (measured empirically against a real Docker mke2fs
# build of the actual aarch64 payload -- see docs/planning/green-program/
# gates/ for the round that derived this):
#   - mke2fs/ext2 metadata (inode tables, block group descriptors, reserved
#     GDT blocks for future resize, block-size rounding per file) cost
#     ~30-40% of total image size at these sizes, not a fixed number of
#     bytes -- so the multiplier is applied to the payload, not added flat.
#   - +45% multiplier reproduced a comfortable ~15-20MB of genuine free
#     space left over on a 62MB real aarch64 payload (72MB requested left
#     only 0.8MB free -- too tight; 88-96MB requested left 14.7-21.8MB
#     free).
#   - +4MB flat on top covers /etc, /tmp, /home, /var/log and the small
#     built-in test text files each image also gets, which don't scale
#     with the binary/font payload above.
required_bytes=$(( payload_bytes * 29 / 20 + 4 * 1024 * 1024 ))
required_mb=$(( (required_bytes + 1024 * 1024 - 1) / (1024 * 1024) ))
payload_mb_display=$(( (payload_bytes + 1024 * 1024 - 1) / (1024 * 1024) ))

echo "  Payload: ${payload_mb_display}MB (userspace binaries + fonts)"
echo "  Minimum image size for this payload: ${required_mb}MB (incl. ext2 overhead + headroom)"

if (( SIZE_MB < required_mb )); then
    echo ""
    echo "ERROR: requested size ${SIZE_MB}MB is too small for the ${payload_mb_display}MB payload." >&2
    echo "  Needs at least ${required_mb}MB (shortfall: $((required_mb - SIZE_MB))MB)." >&2
    echo "  Refusing to build a disk image that would fail partway through the copy" >&2
    echo "  step with ENOSPC. Pass --size ${required_mb} or larger, or omit --size to" >&2
    echo "  use the 256MB default." >&2
    exit 1
fi

# Ensure target directory exists
mkdir -p "$TARGET_DIR"
mkdir -p "$PROJECT_ROOT/testdata"

# A test-only SSH key pair for the loopback SSH oracle (init's run_bssh_pubkey_oracle):
# made for each image build (or taken from BREENIX_TEST_SSH_KEY_DIR, so two builds that
# must be byte-identical share one), authorized in this image only, for its test account
# sshtest and for root (whose login bsshd refuses). It is not committed, and the
# persistent install leaves the key, the account and root's authorized keys behind.
TEST_SSH_DIR="${BREENIX_TEST_SSH_KEY_DIR:-}"
TEST_SSH_TEMP=
if [[ -z "$TEST_SSH_DIR" ]]; then
    TEST_SSH_DIR=$(mktemp -d "${TMPDIR:-/tmp}/breenix-test-ssh.XXXXXX")
    TEST_SSH_TEMP=$TEST_SSH_DIR
fi
remove_test_ssh_temp() {
    if [[ -n "$TEST_SSH_TEMP" ]]; then
        rm -rf "$TEST_SSH_TEMP"
    fi
}
trap remove_test_ssh_temp EXIT
if [[ ! -f "$TEST_SSH_DIR/id_ed25519" ]]; then
    if ! command -v ssh-keygen &> /dev/null; then
        echo "Error: ssh-keygen (OpenSSH) is required to make the image's test-only SSH key"
        exit 1
    fi
    ssh-keygen -q -t ed25519 -N '' -C breenix-test-image -f "$TEST_SSH_DIR/id_ed25519"
fi

# Check if we're on macOS or Linux
if [[ "$(uname)" == "Darwin" ]]; then
    # macOS - use Docker
    if ! command -v docker &> /dev/null; then
        echo "Error: Docker is required on macOS to create ext2 images"
        echo "Install Docker Desktop: https://docs.docker.com/desktop/mac/install/"
        exit 1
    fi

    if ! docker info &> /dev/null; then
        echo "Error: Docker daemon is not running"
        echo "Start Docker Desktop and try again"
        exit 1
    fi

    echo "  Using Docker to create ext2 filesystem..."

    # Extract just the filename from OUTPUT_FILE for use in Docker
    OUTPUT_FILENAME=$(basename "$OUTPUT_FILE")

    docker run --rm --privileged \
        -v "$TARGET_DIR:/work" \
        -v "$USERSPACE_DIR:/binaries:ro" \
        -v "$PROJECT_ROOT/fonts:/fonts:ro" \
        -v "$SCRIPT_DIR/install-libc-busybox.sh:/install-libc-busybox.sh:ro" \
        -v "$PROJECT_ROOT/vendor/busybox/required-applets:/required-applets:ro" \
        -v "$TEST_SSH_DIR:/test-ssh:ro" \
        -e "OUTPUT_FILENAME=$OUTPUT_FILENAME" \
        -e "BREENIX_WAIT_STRESS=${BREENIX_WAIT_STRESS:-0}" \
        -e "BREENIX_BSSH_AUTORUN=${BREENIX_BSSH_AUTORUN:-0}" \
        -e "BREENIX_ARCH=$ARCH" \
        -e "DNS_SERVER=$DNS_SERVER" \
        alpine:latest \
        sh -c '
            set -e
            apk add --no-cache e2fsprogs >/dev/null 2>&1

            # Create the empty disk image
            dd if=/dev/zero of=/work/$OUTPUT_FILENAME bs=1M count='"$SIZE_MB"' status=none

            # Create ext2 filesystem
            mke2fs -t ext2 -F /work/$OUTPUT_FILENAME >/dev/null 2>&1

            # Mount and populate
            mkdir -p /mnt/ext2
            mount /work/$OUTPUT_FILENAME /mnt/ext2

            # Create /bin, /sbin, /usr/local/test/bin, and /usr/local/cbin directories
            mkdir -p /mnt/ext2/bin
            mkdir -p /mnt/ext2/sbin
            mkdir -p /mnt/ext2/usr/local/test/bin
            mkdir -p /mnt/ext2/usr/local/cbin

            sh /install-libc-busybox.sh /binaries /required-applets /mnt/ext2

            # Copy remaining binaries from /binaries directory
            #          test binaries (*_test, test_*) -> /usr/local/test/bin
            #          system binaries (telnetd, init, probe, persistent-install, blogd, suite-*) -> /sbin
            #          everything else -> /bin
            echo "Installing other binaries..."
            bin_count=0
            sbin_count=0
            test_count=0

            for elf_file in /binaries/*.elf; do
                if [ -f "$elf_file" ]; then
                    bin_name=$(basename "$elf_file" .elf)
                    # Skip BusyBox (already installed above)
                    [ "$bin_name" = "libc-busybox" ] && continue
                    if echo "$bin_name" | grep -qE "_test$|^test_"; then
                        cp "$elf_file" /mnt/ext2/usr/local/test/bin/${bin_name}
                        chmod 755 /mnt/ext2/usr/local/test/bin/${bin_name}
                        test_count=$((test_count + 1))
                    elif [ "$bin_name" = "telnetd" ] || [ "$bin_name" = "init" ] || [ "$bin_name" = "probe" ] || [ "$bin_name" = "persistent-install" ] || [ "$bin_name" = "blogd" ] || echo "$bin_name" | grep -qE "^suite-"; then
                        cp "$elf_file" /mnt/ext2/sbin/${bin_name}
                        chmod 755 /mnt/ext2/sbin/${bin_name}
                        sbin_count=$((sbin_count + 1))
                    else
                        cp "$elf_file" /mnt/ext2/bin/${bin_name}
                        chmod 755 /mnt/ext2/bin/${bin_name}
                        bin_count=$((bin_count + 1))
                    fi
                fi
            done
            echo "  Installed $bin_count binaries in /bin"
            echo "  Installed $sbin_count binaries in /sbin"

            echo "  Installed $test_count test binaries in /usr/local/test/bin"

            # The Rust std suite starts its helper through a name that is not UTF-8.
            if [ -f /mnt/ext2/usr/local/test/bin/rust-std-child_test ]; then
                ln /mnt/ext2/usr/local/test/bin/rust-std-child_test "/mnt/ext2/usr/local/test/bin/rust-std-child_test$(printf "\377")"
            fi


            # Create /etc with passwd and group for libc getpwuid/getgrgid
            mkdir -p /mnt/ext2/etc
            printf "127.0.0.1 localhost\n" > /mnt/ext2/etc/hosts
            printf "nameserver %s\n" "$DNS_SERVER" > /mnt/ext2/etc/resolv.conf
            printf "http 80/tcp www\nhttps 443/tcp\ndomain 53/tcp\ndomain 53/udp\nssh 22/tcp\n" > /mnt/ext2/etc/services
            cat > /mnt/ext2/etc/passwd << PASSWD
root:x:0:0:root:/root:/bin/bsh
nobody:x:65534:65534:nobody:/nonexistent:/bin/false
sshtest:x:2000:2000:sshtest:/etc/breenix/test-ssh/home:/bin/sh
PASSWD
            cat > /mnt/ext2/etc/group << GROUP
root:x:0:
nobody:x:65534:
sshtest:x:2000:
GROUP

            # SSH: bsshd generates the host key into /etc/ssh on its first start. The
            # test-only client key logs in as sshtest, whose home is beside the key (/home
            # is a separate disk), and is listed for root so the oracle sees root refused.
            T=/mnt/ext2/etc/breenix/test-ssh
            mkdir -p /mnt/ext2/etc/ssh $T/home/.ssh /mnt/ext2/root/.ssh
            cp /test-ssh/id_ed25519 $T/id_ed25519
            cp /test-ssh/id_ed25519.pub $T/home/.ssh/authorized_keys
            cp /test-ssh/id_ed25519.pub /mnt/ext2/root/.ssh/authorized_keys
            chown -R 0:0 /mnt/ext2/etc/ssh $T /mnt/ext2/root
            chown -R 2000:2000 $T/home
            chmod 755 /mnt/ext2/etc/ssh $T
            chmod 700 $T/home $T/home/.ssh /mnt/ext2/root /mnt/ext2/root/.ssh
            chmod 600 $T/id_ed25519 $T/home/.ssh/authorized_keys /mnt/ext2/root/.ssh/authorized_keys

            # Create /etc/bshrc - startup config for the bsh ECMAScript shell
            cat > /mnt/ext2/etc/bshrc << 'BSHRC'
// Breenix Shell Configuration
// This file is evaluated as ECMAScript by bsh on startup.

// Global PATH - all standard binary directories
env("PATH", "/bin:/sbin:/usr/local/bin:/usr/local/cbin");

// Home directory
env("HOME", "/root");

// Shell identification
env("SHELL", "/bin/bsh");

// Hostname
env("HOSTNAME", "breenix");

// Greeting
console.log("Welcome to Breenix OS");
BSHRC

            # /tmp: owned by root, sticky and writable by other users (docs/users/design.md, section 4)
            mkdir -p /mnt/ext2/tmp
            chmod 1777 /mnt/ext2/tmp

            if [ "${BREENIX_WAIT_STRESS:-0}" = "1" ]; then
                touch /mnt/ext2/etc/wait_stress.enabled
                echo "  Enabled wait_stress init gate"
            fi
            if [ "${BREENIX_BSSH_AUTORUN:-0}" = "1" ]; then
                touch /mnt/ext2/etc/bssh_autorun.enabled
                echo "  Enabled bssh autorun init gate"
            fi

            # Create /home for user data (Gus Kit saves, etc.)
            mkdir -p /mnt/ext2/home

            # Create /var/log for blogd kernel log persistence
            mkdir -p /mnt/ext2/var/log

            # Create test files for filesystem testing
            echo "Hello from ext2!" > /mnt/ext2/hello.txt
            echo "Truncate test file" > /mnt/ext2/trunctest.txt
            touch /mnt/ext2/empty.txt  # Empty file for wc testing
            mkdir -p /mnt/ext2/test
            echo "Nested file content" > /mnt/ext2/test/nested.txt
            # Dense shrink fixtures start allocated before the timed suite cases.
            fixture_block_size=$(stat -f -c %S /mnt/ext2)
            for fixture in files-io-large-shrink files-io-large-open; do
                dd if=/dev/zero bs="$fixture_block_size" count=129 status=none |
                    tr "\000" Z > "/mnt/ext2/test/$fixture"
            done
            # A directory whose entries span at least three blocks, for the
            # directories suite: hard links to one empty file under 200-byte
            # names, which ext2 stores in 208-byte records.
            mkdir -p /mnt/ext2/test/dir-blocks
            i=0
            while [ $((i * 208)) -le $((3 * fixture_block_size)) ]; do
                name=$(printf "%0200d" "$i")
                if [ "$i" -eq 0 ]; then
                    first=$name
                    touch "/mnt/ext2/test/dir-blocks/$first"
                else
                    ln "/mnt/ext2/test/dir-blocks/$first" "/mnt/ext2/test/dir-blocks/$name"
                fi
                i=$((i + 1))
            done

            # Create additional test content
            mkdir -p /mnt/ext2/deep/path/to/file
            echo "Deep nested content" > /mnt/ext2/deep/path/to/file/data.txt

            # Create multi-line test file for head/tail/wc testing (15 lines)
            cat > /mnt/ext2/lines.txt << EOF
Line 1
Line 2
Line 3
Line 4
Line 5
Line 6
Line 7
Line 8
Line 9
Line 10
Line 11
Line 12
Line 13
Line 14
Line 15
EOF

            # Install TrueType fonts
            mkdir -p /mnt/ext2/usr/share/fonts
            font_count=0
            for font_file in /fonts/*.ttf; do
                if [ -f "$font_file" ]; then
                    cp "$font_file" /mnt/ext2/usr/share/fonts/
                    font_count=$((font_count + 1))
                fi
            done
            echo "  Installed $font_count fonts in /usr/share/fonts"

            # Create default system font configuration
            cat > /mnt/ext2/etc/fonts.conf << FONTSCONF
# Breenix System Font Configuration
# Apps read this file for default font settings.
# Use bfontpicker to change fonts interactively.
#
# Available fonts:
#   /usr/share/fonts/DejaVuSansMono.ttf
#   /usr/share/fonts/DejaVuSans.ttf
#   /usr/share/fonts/Inconsolata-Regular.ttf
#   /usr/share/fonts/JetBrainsMono-Regular.ttf
#
mono.font=/usr/share/fonts/DejaVuSansMono.ttf
mono.size=10
display.font=/usr/share/fonts/DejaVuSans.ttf
display.size=14
FONTSCONF
            echo "  Created /etc/fonts.conf"

            # Create /etc/hotkeys.conf - BWM hotkey configuration
            cat > /mnt/ext2/etc/hotkeys.conf << 'HOTKEYS'
# Breenix hotkey configuration
# Format: modifier+key = action
# Modifiers: Super (Cmd), Alt, Ctrl, Shift
# Actions: exec <path>, close, focus_next, focus_prev
# Double-tap: Super+Super = exec /bin/blauncher

Super+Super  = exec /bin/blauncher
Super+Return = exec /bin/bterm
Alt+q        = close
Alt+Tab      = focus_next
Alt+Shift+Tab = focus_prev
HOTKEYS
            echo "  Created /etc/hotkeys.conf"

            # Create /etc/init.js - bsh boot script executed by init
            if [ "$BREENIX_ARCH" = aarch64 ]; then
            cat > /mnt/ext2/etc/init.js << 'INITJS'
// Breenix boot script - executed by bsh on startup
// spawn() starts a process in the background and returns its PID.
// Keep launch non-blocking; timer sleeps during early single-CPU Parallels boot
// can delay the visible desktop behind background tasks.

// Network service
spawn("/sbin/telnetd");

// Window compositor (must start before GUI apps)
spawn("/bin/bwm");

// GUI applications
spawn("/bin/bterm");
spawn("/bin/blog");
spawn("/bin/bounce");
spawn("/bin/bcheck");

// Log daemon starts after the visible desktop. On single-CPU Parallels, blogd
// initial /proc/kmsg drain can otherwise starve the boot script before GUI apps
// get scheduled.
spawn("/sbin/blogd");
INITJS
            else
                echo "// The x86 boot script starts no network service: telnetd is not run on x86." > /mnt/ext2/etc/init.js
            fi
            echo "  Created /etc/init.js"

            # Show what was created
            echo ""
            echo "ext2 filesystem contents:"
            echo "  Binaries in /bin:"
            ls -la /mnt/ext2/bin/ 2>/dev/null || echo "    (none)"
            echo "  Test binaries in /usr/local/test/bin:"
            ls -la /mnt/ext2/usr/local/test/bin/ 2>/dev/null || echo "    (none)"
            echo "  Test files:"
            find /mnt/ext2 -type f -not -path "/mnt/ext2/bin/*" -not -path "/mnt/ext2/usr/local/test/bin/*" -exec ls -la {} \;

            # Unmount
            umount /mnt/ext2

            echo ""
            echo "ext2 image created successfully"
        '
else
    # Linux - use native tools
    if ! command -v mke2fs &> /dev/null; then
        echo "Error: mke2fs not found. Install e2fsprogs:"
        echo "  apt-get install e2fsprogs  # Debian/Ubuntu"
        echo "  yum install e2fsprogs      # RHEL/CentOS"
        exit 1
    fi

    # Create empty image
    dd if=/dev/zero of="$OUTPUT_FILE" bs=1M count=$SIZE_MB status=none

    # Mounted ext2 allocation depends on host process IDs. Gate cache builds
    # populate through libext2fs instead, giving clean builds identical placement.
    if [[ "${BREENIX_REPRODUCIBLE_EXT2:-0}" != 1 ]]; then
        mke2fs -t ext2 -F "$OUTPUT_FILE" >/dev/null 2>&1
    fi

    # Mount and populate (requires root)
    if [[ $EUID -ne 0 ]]; then
        echo "Warning: Need root to mount and populate image"
        echo "Run with sudo or populate manually"
        exit 1
    fi

    MOUNT_DIR=$(mktemp -d)
    MOUNTED=0
    cleanup_mount() {
        remove_test_ssh_temp
        if [[ "$MOUNTED" == 1 ]]; then
            umount "$MOUNT_DIR" || return
        fi
        rm -rf "$MOUNT_DIR"
    }
    trap cleanup_mount EXIT
    if [[ "${BREENIX_REPRODUCIBLE_EXT2:-0}" == 1 ]]; then
        chmod 755 "$MOUNT_DIR"
    else
        mount "$OUTPUT_FILE" "$MOUNT_DIR"
        MOUNTED=1
    fi

    # Create /bin, /sbin, /usr/local/test/bin, and /usr/local/cbin directories
    mkdir -p "$MOUNT_DIR/bin"
    mkdir -p "$MOUNT_DIR/sbin"
    mkdir -p "$MOUNT_DIR/usr/local/test/bin"
    mkdir -p "$MOUNT_DIR/usr/local/cbin"

    "$SCRIPT_DIR/install-libc-busybox.sh" "$USERSPACE_DIR" "$PROJECT_ROOT/vendor/busybox/required-applets" "$MOUNT_DIR"

    # Copy remaining binaries from userspace directory
    #          test binaries (*_test, test_*) -> /usr/local/test/bin
    #          system binaries (telnetd, init, probe, persistent-install, blogd, suite-*) -> /sbin
    #          everything else -> /bin
    echo "Installing other binaries..."
    bin_count=0
    sbin_count=0
    test_count=0

    for elf_file in "$USERSPACE_DIR"/*.elf; do
        if [ -f "$elf_file" ]; then
            bin_name=$(basename "$elf_file" .elf)
            # Skip BusyBox (already installed above)
            [ "$bin_name" = "libc-busybox" ] && continue
            if echo "$bin_name" | grep -qE '_test$|^test_'; then
                cp "$elf_file" "$MOUNT_DIR/usr/local/test/bin/${bin_name}"
                chmod 755 "$MOUNT_DIR/usr/local/test/bin/${bin_name}"
                test_count=$((test_count + 1))
            elif [ "$bin_name" = "telnetd" ] || [ "$bin_name" = "init" ] || [ "$bin_name" = "probe" ] || [ "$bin_name" = "persistent-install" ] || [ "$bin_name" = "blogd" ] || echo "$bin_name" | grep -qE '^suite-'; then
                cp "$elf_file" "$MOUNT_DIR/sbin/${bin_name}"
                chmod 755 "$MOUNT_DIR/sbin/${bin_name}"
                sbin_count=$((sbin_count + 1))
            else
                cp "$elf_file" "$MOUNT_DIR/bin/${bin_name}"
                chmod 755 "$MOUNT_DIR/bin/${bin_name}"
                bin_count=$((bin_count + 1))
            fi
        fi
    done
    echo "  Installed $bin_count binaries in /bin"
    echo "  Installed $sbin_count binaries in /sbin"

    echo "  Installed $test_count test binaries in /usr/local/test/bin"

    # The Rust std suite starts its helper through a name that is not UTF-8.
    if [ -f "$MOUNT_DIR/usr/local/test/bin/rust-std-child_test" ]; then
        ln "$MOUNT_DIR/usr/local/test/bin/rust-std-child_test" "$MOUNT_DIR/usr/local/test/bin/rust-std-child_test$(printf '\377')"
    fi


    # Create /etc with passwd and group for libc getpwuid/getgrgid
    mkdir -p "$MOUNT_DIR/etc"
    printf '127.0.0.1 localhost\n' > "$MOUNT_DIR/etc/hosts"
    printf 'nameserver %s\n' "$DNS_SERVER" > "$MOUNT_DIR/etc/resolv.conf"
    printf 'http 80/tcp www\nhttps 443/tcp\ndomain 53/tcp\ndomain 53/udp\nssh 22/tcp\n' > "$MOUNT_DIR/etc/services"
    cat > "$MOUNT_DIR/etc/passwd" << PASSWD
root:x:0:0:root:/root:/bin/bsh
nobody:x:65534:65534:nobody:/nonexistent:/bin/false
sshtest:x:2000:2000:sshtest:/etc/breenix/test-ssh/home:/bin/sh
PASSWD
    cat > "$MOUNT_DIR/etc/group" << GROUP
root:x:0:
nobody:x:65534:
sshtest:x:2000:
GROUP

    # SSH: bsshd generates the host key into /etc/ssh on its first start. The
    # test-only client key logs in as sshtest, whose home is beside the key (/home
    # is a separate disk), and is listed for root so the oracle sees root refused.
    TEST_SSH_ROOT="$MOUNT_DIR/etc/breenix/test-ssh"
    mkdir -p "$MOUNT_DIR/etc/ssh" "$TEST_SSH_ROOT/home/.ssh" "$MOUNT_DIR/root/.ssh"
    cp "$TEST_SSH_DIR/id_ed25519" "$TEST_SSH_ROOT/id_ed25519"
    cp "$TEST_SSH_DIR/id_ed25519.pub" "$TEST_SSH_ROOT/home/.ssh/authorized_keys"
    cp "$TEST_SSH_DIR/id_ed25519.pub" "$MOUNT_DIR/root/.ssh/authorized_keys"
    chown -R 0:0 "$MOUNT_DIR/etc/ssh" "$TEST_SSH_ROOT" "$MOUNT_DIR/root"
    chown -R 2000:2000 "$TEST_SSH_ROOT/home"
    chmod 755 "$MOUNT_DIR/etc/ssh" "$TEST_SSH_ROOT"
    chmod 700 "$TEST_SSH_ROOT/home" "$TEST_SSH_ROOT/home/.ssh" "$MOUNT_DIR/root" "$MOUNT_DIR/root/.ssh"
    chmod 600 "$TEST_SSH_ROOT/id_ed25519" "$TEST_SSH_ROOT/home/.ssh/authorized_keys" "$MOUNT_DIR/root/.ssh/authorized_keys"

    # Create /etc/bshrc - startup config for the bsh ECMAScript shell
    cat > "$MOUNT_DIR/etc/bshrc" << 'BSHRC'
// Breenix Shell Configuration
// This file is evaluated as ECMAScript by bsh on startup.

// Global PATH - all standard binary directories
env("PATH", "/bin:/sbin:/usr/local/bin:/usr/local/cbin");

// Home directory
env("HOME", "/root");

// Shell identification
env("SHELL", "/bin/bsh");

// Hostname
env("HOSTNAME", "breenix");

// Greeting
console.log("Welcome to Breenix OS");
BSHRC

    # /tmp: owned by root, sticky and writable by other users (docs/users/design.md, section 4)
    mkdir -p "$MOUNT_DIR/tmp"
    chmod 1777 "$MOUNT_DIR/tmp"

    if [ "${BREENIX_WAIT_STRESS:-0}" = "1" ]; then
        touch "$MOUNT_DIR/etc/wait_stress.enabled"
        echo "  Enabled wait_stress init gate"
    fi
    if [ "${BREENIX_BSSH_AUTORUN:-0}" = "1" ]; then
        touch "$MOUNT_DIR/etc/bssh_autorun.enabled"
        echo "  Enabled bssh autorun init gate"
    fi

    # Create /home for user data (Gus Kit saves, etc.)
    mkdir -p "$MOUNT_DIR/home"

    # Create /var/log for blogd kernel log persistence
    mkdir -p "$MOUNT_DIR/var/log"

    # Create test files
    echo "Hello from ext2!" > "$MOUNT_DIR/hello.txt"
    echo "Truncate test file" > "$MOUNT_DIR/trunctest.txt"
    touch "$MOUNT_DIR/empty.txt"  # Empty file for wc testing
    mkdir -p "$MOUNT_DIR/test"
    echo "Nested file content" > "$MOUNT_DIR/test/nested.txt"
    # Match the dense fixtures installed by the Docker path above.
    if [[ "${BREENIX_REPRODUCIBLE_EXT2:-0}" == 1 ]]; then
        fixture_block_size="${BREENIX_EXT2_BLOCK_SIZE:-4096}"
    else
        fixture_block_size=$(stat -f -c %S "$MOUNT_DIR")
    fi
    for fixture in files-io-large-shrink files-io-large-open; do
        dd if=/dev/zero bs="$fixture_block_size" count=129 status=none |
            tr "\000" Z > "$MOUNT_DIR/test/$fixture"
    done
    # Match the multi-block directory installed by the Docker path above.
    mkdir -p "$MOUNT_DIR/test/dir-blocks"
    i=0
    while [ $((i * 208)) -le $((3 * fixture_block_size)) ]; do
        name=$(printf "%0200d" "$i")
        if [ "$i" -eq 0 ]; then
            first=$name
            touch "$MOUNT_DIR/test/dir-blocks/$first"
        else
            ln "$MOUNT_DIR/test/dir-blocks/$first" "$MOUNT_DIR/test/dir-blocks/$name"
        fi
        i=$((i + 1))
    done
    mkdir -p "$MOUNT_DIR/deep/path/to/file"
    echo "Deep nested content" > "$MOUNT_DIR/deep/path/to/file/data.txt"

    # Create multi-line test file for head/tail/wc testing (15 lines)
    cat > "$MOUNT_DIR/lines.txt" << EOF
Line 1
Line 2
Line 3
Line 4
Line 5
Line 6
Line 7
Line 8
Line 9
Line 10
Line 11
Line 12
Line 13
Line 14
Line 15
EOF

    # Install TrueType fonts
    mkdir -p "$MOUNT_DIR/usr/share/fonts"
    font_count=0
    for font_file in "$PROJECT_ROOT/fonts"/*.ttf; do
        if [ -f "$font_file" ]; then
            cp "$font_file" "$MOUNT_DIR/usr/share/fonts/"
            font_count=$((font_count + 1))
        fi
    done
    echo "  Installed $font_count fonts in /usr/share/fonts"

    # Create default system font configuration
    cat > "$MOUNT_DIR/etc/fonts.conf" << FONTSCONF
# Breenix System Font Configuration
# Apps read this file for default font settings.
# Use bfontpicker to change fonts interactively.
#
# Available fonts:
#   /usr/share/fonts/DejaVuSansMono.ttf
#   /usr/share/fonts/DejaVuSans.ttf
#   /usr/share/fonts/Inconsolata-Regular.ttf
#   /usr/share/fonts/JetBrainsMono-Regular.ttf
#
mono.font=/usr/share/fonts/DejaVuSansMono.ttf
mono.size=10
display.font=/usr/share/fonts/DejaVuSans.ttf
display.size=14
FONTSCONF
    echo "  Created /etc/fonts.conf"

    # Create /etc/hotkeys.conf - BWM hotkey configuration
    cat > "$MOUNT_DIR/etc/hotkeys.conf" << 'HOTKEYS'
# Breenix hotkey configuration
# Format: modifier+key = action
# Modifiers: Super (Cmd), Alt, Ctrl, Shift
# Actions: exec <path>, close, focus_next, focus_prev
# Double-tap: Super+Super = exec /bin/blauncher

Super+Super  = exec /bin/blauncher
Super+Return = exec /bin/bterm
Alt+q        = close
Alt+Tab      = focus_next
Alt+Shift+Tab = focus_prev
HOTKEYS
    echo "  Created /etc/hotkeys.conf"

    # Create /etc/init.js - bsh boot script executed by init
    if [ "$ARCH" = aarch64 ]; then
    cat > "$MOUNT_DIR/etc/init.js" << 'INITJS'
// Breenix boot script - executed by bsh on startup
// spawn() starts a process in the background and returns its PID.
// Keep launch non-blocking; timer sleeps during early single-CPU Parallels boot
// can delay the visible desktop behind background tasks.

// Network service
spawn("/sbin/telnetd");

// Window compositor (must start before GUI apps)
spawn("/bin/bwm");

// GUI applications
spawn("/bin/bterm");
spawn("/bin/blog");
spawn("/bin/bounce");
spawn("/bin/bcheck");

// Log daemon starts after the visible desktop. On single-CPU Parallels, blogd
// initial /proc/kmsg drain can otherwise starve the boot script before GUI apps
// get scheduled.
spawn("/sbin/blogd");
INITJS
    else
        echo '// The x86 boot script starts no network service: telnetd is not run on x86.' > "$MOUNT_DIR/etc/init.js"
    fi
    echo "  Created /etc/init.js"

    # Show what was created
    echo ""
    echo "ext2 filesystem contents:"
    ls -la "$MOUNT_DIR/bin/"
    echo "  Test binaries in /usr/local/test/bin:"
    ls -la "$MOUNT_DIR/usr/local/test/bin/" 2>/dev/null || echo "    (none)"
    find "$MOUNT_DIR" -type f -not -path "$MOUNT_DIR/bin/*" -not -path "$MOUNT_DIR/usr/local/test/bin/*" -exec ls -la {} \;

    # Unmount and cleanup
    if [[ "${BREENIX_REPRODUCIBLE_EXT2:-0}" == 1 ]]; then
        E2FSPROGS_FAKE_TIME=946684800 mke2fs -t ext2 -b "${BREENIX_EXT2_BLOCK_SIZE:-4096}" -U 7fabead6-ba12-4c3b-835b-e076230994d9 -E hash_seed=00000000-0000-0000-0000-000000000000 -F -d "$MOUNT_DIR" "$OUTPUT_FILE" >/dev/null 2>&1
        rm -rf "$MOUNT_DIR"
        trap remove_test_ssh_temp EXIT
    else
        umount "$MOUNT_DIR"
        MOUNTED=0
        rmdir "$MOUNT_DIR"
    fi

    echo "ext2 image created successfully"
fi

# Copy to testdata/ for version control
if [[ -f "$OUTPUT_FILE" ]]; then
    cp "$OUTPUT_FILE" "$TESTDATA_FILE"
    SIZE=$(ls -lh "$OUTPUT_FILE" | awk '{print $5}')
    echo ""
    echo "ext2 disk created and copied to testdata/:"
    echo "  $OUTPUT_FILE"
    echo "  $TESTDATA_FILE"
    echo "  Size: $SIZE"
    echo ""
    echo "Contents:"
    echo "  /bin/busybox - BusyBox multi-call binary"
    echo "  /bin/ls - BusyBox ls applet"
    echo "  /bin/{cat,head,tail,...} - BusyBox hardlinks"
    echo "  /sbin/{true,false} - BusyBox hardlinks"
    echo "  /bin/* - Other userspace binaries (demos)"
    echo "  /usr/local/test/bin/* - Test binaries (*_test, test_*)"
    echo "  /sbin/telnetd - telnet daemon (started on ARM64 only)"
    echo "  /etc/breenix/test-ssh/id_ed25519 - this image's test-only SSH key"
    echo "  /hello.txt - test file (1 line)"
    echo "  /lines.txt - multi-line test file (15 lines) for head/tail/wc"
    echo "  /test/nested.txt - nested test file"
    echo "  /deep/path/to/file/data.txt - deep nested test file"
else
    echo "Error: Failed to create ext2 image"
    exit 1
fi
