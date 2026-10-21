#!/bin/sh
# Apply the single placement/ownership table to a populated image tree.
set -eu
root=$1
binaries=$2
table=$3
while read -r kind path mode uid gid; do
    case "$kind" in
        dir) mkdir -p "$root/$path"; chown "$uid:$gid" "$root/$path"; chmod "$mode" "$root/$path" ;;
    esac
done < "$table"
for elf in "$binaries"/*.elf; do
    [ -f "$elf" ] || continue
    name=${elf##*/}; name=${name%.elf}
    [ "$name" = libc-busybox ] && continue
    while read -r kind pattern destination mode gid; do
        [ "$kind" = bin ] || continue
        case "$name" in
            $pattern)
                # The old population step is retired below; copy straight from the ELF.
                rm -f "$root/bin/$name" "$root/sbin/$name" "$root/usr/local/test/bin/$name"
                cp "$elf" "$root/$destination/$name"
                chown "0:$gid" "$root/$destination/$name"
                chmod "$mode" "$root/$destination/$name"
                break ;;
        esac
    done < "$table"
done
for fixture in test deep; do
    if [ -d "$root/$fixture" ]; then
        rmdir "$root/usr/libexec/breenix/$fixture"
        mv "$root/$fixture" "$root/usr/libexec/breenix/$fixture"
    fi
done
if [ -d "$root/usr/local/libc" ]; then
    rmdir "$root/usr/libexec/breenix/libc/bin" "$root/usr/libexec/breenix/libc"
    mv "$root/usr/local/libc" "$root/usr/libexec/breenix/libc"
fi
if [ -d "$root/usr/local/test/bin" ]; then
    # Includes the Rust-std non-UTF-8 hard link, which is a fixture rather than an ELF.
    for file in "$root/usr/local/test/bin"/*; do
        [ -e "$file" ] || continue
        mv "$file" "$root/usr/libexec/breenix/"
    done
    rmdir "$root/usr/local/test/bin" "$root/usr/local/test"
fi
if [ -f "$root/usr/libexec/breenix/rust-std-child_test" ]; then
    ln "$root/usr/libexec/breenix/rust-std-child_test" "$root/usr/libexec/breenix/rust-std-child_test$(printf '\377')"
fi
printf 'root:!:::::::\nnobody:*:::::::\nsshtest:!:::::::\n' > "$root/etc/shadow"
printf 'permit %%wheel as root\n' > "$root/etc/sudo.policy"
printf 'UMASK 022\nROOT_LOGIN no\nFAIL_DELAY 2\n' > "$root/etc/login.defs"
printf '/bin/sh\n/bin/bsh\n' > "$root/etc/shells"
cat >> "$root/etc/group" <<'GROUPS'
adm:x:4:
tty:x:5:
wheel:x:10:
users:x:100:
nogroup:x:65534:
GROUPS
while read -r kind path mode uid gid; do
    case "$kind" in
        file) [ -e "$root/$path" ] || : > "$root/$path"
              chown "$uid:$gid" "$root/$path"; chmod "$mode" "$root/$path" ;;
    esac
done < "$table"
# Defaults contain no generated test credentials or per-boot intent.
cp -a "$root/etc/." "$root/usr/share/factory/etc/"
rm -rf "$root/usr/share/factory/etc/breenix" "$root/usr/share/factory/etc/ssh"
mkdir "$root/usr/share/factory/etc/ssh"
# PID 1 clears this fixture before the layout suite measures /run.
printf 'stale\n' > "$root/run/layout-stale"
