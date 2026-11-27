#!/bin/sh
# Run on Breenix with the btest account and state directories provisioned.
set -eu

id=limits-check-$$
stage=/var/lib/btest/stage/$id
mkdir -m 0755 "$stage"
trap 'rm -r "$stage"' EXIT
mkdir -m 0755 "$stage/bin" "$stage/suite" "$stage/suite/golden" "$stage/suite/inputs"
cp "$1" "$stage/supervisor"
chmod 0555 "$stage/supervisor"
ln -s /bin/sh "$stage/bin/sh"

umask 022
cat > "$stage/suite/manifest.json" <<'EOF'
{"program":"sh","binary":"sh","format":1,"cases":[{"id":"inherited-limits","argv":["-s"],"source":"manual"}]}
EOF
cat > "$stage/suite/inputs/inherited-limits.in" <<'EOF'
set -e
for resource in t f u v; do
    ulimit -S "-$resource"
    ulimit -H "-$resource"
done
EOF
printf '%s\n' 2 2 32768 32768 16 16 65536 65536 > "$stage/suite/golden/inherited-limits.out"
: > "$stage/suite/golden/inherited-limits.err"
printf '0\n' > "$stage/suite/golden/inherited-limits.status"

result=$(sudo -n /usr/bin/sudo -n -u btest "$stage/supervisor" __case-supervisor "$stage" "$id")
printf '%s\n' "$result"
case "$result" in
    *'"failed":0,"passed":1}' ) ;;
    *) exit 1 ;;
esac
