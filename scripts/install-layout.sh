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
    rmdir "$root/usr/local/test/bin"
fi
if [ -d "$root/usr/local/test/share" ]; then
    mv "$root/usr/local/test/share" "$root/usr/libexec/breenix/share"
fi
if [ -d "$root/usr/local/test" ]; then
    rmdir "$root/usr/local/test"
fi
if [ -f "$root/usr/libexec/breenix/rust-std-child_test" ]; then
    ln "$root/usr/libexec/breenix/rust-std-child_test" "$root/usr/libexec/breenix/rust-std-child_test$(printf '\377')"
fi
printf 'root:!:::::::\nnobody:*:::::::\nsshtest:!:::::::\n' > "$root/etc/shadow"
# The users suite's fixture accounts (docs/users/design.md, section 6): test images
# only, with locked passwords. Their homes sit beside the other test credentials in
# /etc/breenix, which the persistent install leaves behind with these lines.
for fixture in ua:2001 ub:2002 uw:2003; do
    name=${fixture%%:*}; id=${fixture#*:}
    printf '%s:x:%s:%s:Users suite fixture:/etc/breenix/test-users/%s:/bin/bsh\n' "$name" "$id" "$id" "$name" >> "$root/etc/passwd"
    printf '%s:x:%s:\n' "$name" "$id" >> "$root/etc/group"
    printf '%s:!:::::::\n' "$name" >> "$root/etc/shadow"
    mkdir -p "$root/etc/breenix/test-users/$name"
    chown "$id:$id" "$root/etc/breenix/test-users/$name"
    chmod 0700 "$root/etc/breenix/test-users/$name"
done
printf 'permit %%wheel as root\n' > "$root/etc/sudo.policy"
printf 'UMASK 022\nROOT_LOGIN no\nFAIL_DELAY 2\n' > "$root/etc/login.defs"
printf '/bin/sh\n/bin/bsh\n' > "$root/etc/shells"
cat >> "$root/etc/group" <<'GROUPS'
adm:x:4:
tty:x:5:
wheel:x:10:uw
users:x:100:
ug:x:2100:ua
GROUPS
# The account records the test fixtures added, verbatim, and their memberships of
# system groups. persistent-install leaves out exactly these records, not an
# account matched by its name.
{
    for file in passwd group shadow; do
        grep -E '^(sshtest|ua|ub|uw|ug):' "$root/etc/$file" | sed "s/^/$file /"
    done
    printf 'member wheel uw\n'
} > "$root/etc/breenix/test-fixtures"
chmod 0644 "$root/etc/breenix/test-fixtures"
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
# The service table is a factory default that reconciliation installs and updates.
if [ -f "$root/etc/breenix/services" ]; then
    mkdir "$root/usr/share/factory/etc/breenix"
    cp -p "$root/etc/breenix/services" "$root/usr/share/factory/etc/breenix/services"
fi
# Init clears both fixtures before the suite measures their absence.
for volatile in run tmp; do
    printf 'stale\n' > "$root/$volatile/layout-stale"
done
