#!/bin/sh
# Build the pinned 25.11d2 workload using the staged bundle.
# Usage: sh busybox.sh [PHASE] FIXTURE BUILD-DIRECTORY x86_64|aarch64
# FIXTURE contains vendor/busybox and bundle/{bin,include,include-support,lib}.
# The guest uses BUSYBOX_BUNDLE to name the sealed versioned bundle.
# HOSTCC defaults to the bundle's cc. The Mac check supplies native Clang.
# BUSYBOX_JOBS selects the bmake job budget and defaults to four.
set -eu
export LC_ALL=C PATH=/bin:/usr/bin:/sbin:/usr/sbin

fail() {
    printf 'busybox: %s\n' "$*" >&2
    exit 1
}

case "$#" in
    3) phase=all ;;
    4) phase=$1; shift ;;
    *) fail 'usage: busybox.sh [PHASE] FIXTURE BUILD-DIRECTORY x86_64|aarch64' ;;
esac
case "$3" in x86_64|aarch64) arch=$3 ;; *) fail "unknown architecture: $3" ;; esac
case "$2" in /*) output=$2 ;; *) output=$PWD/$2 ;; esac
fixture=$(CDPATH='' cd -- "$1" && pwd -P)
vendor=$fixture/vendor/busybox
archive=busybox-1.37.0.tar.bz2
fragment=breenix-libc.config

verify() {
    digest=$(sha256sum < "$vendor/$1") || fail "$1: cannot calculate SHA-256"
    [ "${digest%% *}" = "$2" ] || fail "$1: SHA-256 mismatch"
}

report_time() {
    printf '%s_seconds=%s\n' "$1" "$(( $(date +%s) - $start ))"
}

verify_inputs() {
# These pins match toolchain/docs/busybox-c-inventory.json.
verify "$archive" 3311dff32e746499f4df0d5df04d7eb396382d7e108bb9250e7b519b837043a4
verify patches/series 5d0e09b0c8dc1da1f1a1f1877d56971dba00f5636995bd54809d8270f5856da4
verify "$fragment" c7761f177c6c5b83e54e9050dcdbc1caea41aa6bcefa0a038fcdbdd0d3674d04
while read -r name checksum; do
    verify "patches/$name" "$checksum"
done <<'PATCH_PINS'
diff-normal-format.patch 29cfd1ddc368de2c2a61bf4ccf2901f69a39189bc92b1a16e6477aa155f991ef
find-exec-plus-status.patch e236ee2e655db034d7cc87d2425607580c92c59ad8d3e8f2e928014c0a0c51e8
ps-pid-list.patch 993fe8e971853627ee204708385e0d60814e408a88317906544d011eaf0b2819
timeout-status.patch f902bc7f5dee913ee598f5b063572f9b24622ea4764a2669444f7941a49c2478
time-posix.patch 9c41902a8d44cf5d79440cfad4d891f302876f6b424868a99f721c0b1ee16154
getopt32-dead-usage-flag.patch 92c7f4d4687bce06de5e31d1464d163c9c9a7a82507d05d8fb222ba522b9f4c0
sha-drop-sha512-rotate.patch d993c4f4847c86107675554245f0f6d2ad0a7c7c5bb46db99a8ba683763f06ad
list-drop-installer-paths.patch 656b5604783b43ca795b763ddc7a90d170853eec1c00f1888dadd852840e4f7c
host-drop-docproc.patch 0ae2b34beda59f0e1f0d27461d5ab457fa729b9d3ae2a032ddf5094e995341a8
free-breenix-sysinfo.patch c16f3a7a97f30df90d4ccdef5dc73355899f0ede2d7671e1932ff58521df5553
PATCH_PINS
}

bundle=${BUSYBOX_BUNDLE:-$fixture/bundle}
bin=$bundle/bin
hostcc=${HOSTCC:-$bin/cc --target=$arch -std=gnu11 -I$bundle/include -I$bundle/include-support}
jobs=${BUSYBOX_JOBS:-4}
case "$jobs" in ''|*[!0-9]*) fail 'BUSYBOX_JOBS must be a positive integer' ;; esac
[ "$jobs" -gt 0 ] || fail 'BUSYBOX_JOBS must be a positive integer'
export PATH="$fixture/bin:$PATH"
# Kbuild stores commands in recipes and whitespace-delimited variables.
case "$fixture:$output:$bundle" in
    *[!a-zA-Z0-9_./:-]*) fail 'fixture and output paths must be safe to expand in Kbuild recipes' ;;
esac
case "$hostcc" in
    *[!a-zA-Z0-9_./=@\ -]*) fail 'HOSTCC must be safe to expand in Kbuild recipes' ;;
esac

# A phase only publishes its receipt after its complete output is available.
# Receipts use paths relative to the nonce directory and include the inputs
# that must still be current when the next phase begins.
source=$output/busybox-1.37.0
exec 3>&1
mark() { printf 'BUSYBOX_PHASE %s %s\n' "$(date +%s)" "$*" >&3; }

receipts() {
    (cd "$output" && sha256sum "$@") > "$receipt.new" || fail "cannot record $step"
    mv "$receipt.new" "$receipt"
}

begin() {
    step=$1
    receipt=$output/$step.sha256
    mark "$step start"
    start=$(date +%s)
    [ ! -e "$receipt" ] || fail "$step already completed"
}

needs() {
    predecessor=$output/$1.sha256
    [ -s "$predecessor" ] &&
        (cd "$output" && sha256sum -c "$predecessor") > /dev/null ||
        fail "$step: predecessor $1 missing or altered"
}

source_receipt() {
    (cd "$output" && find busybox-1.37.0 -type f -exec sha256sum {} +) > "$receipt.new"
    [ -s "$receipt.new" ] || fail "cannot record $step source tree"
    mv "$receipt.new" "$receipt"
}
make_busybox() {
    # Clear environment variables that could inject options or host inputs.
    env -i PATH="$PATH:$bin" LC_ALL=C TMPDIR="$output/tmp" \
        BMAKE_INHERIT_PROCESS_GROUP="${BMAKE_INHERIT_PROCESS_GROUP:-0}" \
        "$bin/make" -f Makefile "$@" "ARCH=$arch" "srctree=$source" "objtree=$source" \
        "CC=$bin/cc --target=$arch" "HOSTCC=$output/hostcc" \
        "LD=$bin/ld" "AR=$bin/ar" "NM=$bin/nm" \
        'HOSTCFLAGS=-Wall -Wstrict-prototypes -O0' \
        "EXTRA_CFLAGS=-std=gnu11 -O2 -ffunction-sections -Werror -DBB_GLOBAL_CONST= -D__breenix__=1 -I$bundle/include -I$bundle/include-support" \
        SKIP_STRIP=y
}

logged_make() {
    log=$1
    shift
    if ! make_busybox "$@" >> "$output/$log" 2>&1; then
        tail -n 12 "$output/$log" >&2
        fail "bmake $* failed, see $output/$log"
    fi
}

run_unpack() {
    begin unpack
    verify_inputs
    for tool in cc as ld ar nm make; do
        [ -x "$bin/$tool" ] || fail "missing bundle executable: $tool"
    done
    mark "extract archive"
    mkdir -- "$output" || fail 'build directory must be new'
    # Record decompressor success separately because the shell returns the
    # last pipeline command's status. Drain the tar padding before closing.
    (bunzip2 -c "$vendor/$archive" && : > "$output/.decompressed") |
        (cd "$output" && tar -xf - && cat > /dev/null)
    [ -f "$output/.decompressed" ] || fail 'bunzip2 failed'
    rm -- "$output/.decompressed"
    mark "extract archive done"
    printf 'unpack_tree_kib=%s\n' "$(du -sk "$output" | awk '{print $1}')"
    report_time unpack

    mkdir "$output/tmp"
    cat > "$output/hostcc" <<'HOSTCC_TRACE'
#!/bin/sh
trace_hostcc() {
    while [ "$#" -ge 2 ]; do
        if [ "$1" = -o ]; then
            case "$2" in
                /dev/null) ;;
                *) printf 'BUSYBOX_PHASE %s HOSTCC %s\n' "$(date +%s)" "$2" >&3 ;;
            esac
            return
        fi
        shift
    done
}
trace_hostcc "$@"
HOSTCC_TRACE
    printf 'exec %s "$@"\n' "$hostcc" >> "$output/hostcc"
    chmod +x "$output/hostcc"
    printf '%s\n' "$fixture" "$bundle" "$arch" "$hostcc" > "$output/settings"
    # Keep the pins available during the phases whose inputs are consumed later.
    sha256sum "$vendor/$fragment" "$vendor/patches/series" > "$output/config-inputs.sha256"
    source_receipt
}

check_settings() {
    [ -f "$output/settings" ] &&
        [ "$(cat "$output/settings")" = "$(printf '%s
' "$fixture" "$bundle" "$arch" "$hostcc")" ] ||
        fail "$step: driver inputs differ from unpack"
}

run_patch() {
    begin patch
    needs unpack
    check_settings
    verify_inputs
    (
        cd "$source" || exit 1
        while IFS= read -r name || [ -n "$name" ]; do
            case "$name" in ''|'#'*) continue ;; esac
            mark "patch $name" >&3
            patch -p1 < "$vendor/patches/$name" || exit 1
        done < "$vendor/patches/series"
    ) > "$output/patch.log" 2>&1 || { cat "$output/patch.log" >&2; fail 'patch failed'; }


    source_receipt
    report_time patch
}

run_generators() {
    begin generators
    needs patch
    check_settings
    cd "$source"
    mark "Kbuild allnoconfig and native generators"
    logged_make generators.log allnoconfig V=1
    receipts busybox-1.37.0/scripts/basic/fixdep busybox-1.37.0/scripts/basic/split-include \
        busybox-1.37.0/scripts/kconfig/conf busybox-1.37.0/.config
    report_time generators
}

run_configure() {
    begin configure
    needs generators
    check_settings
    sha256sum -c "$output/config-inputs.sha256" > /dev/null
    cd "$source"
    (
        awk '
            NR == FNR {
                if (/^CONFIG_[A-Z0-9_]+=/) {
                    key = $0
                    sub(/=.*/, "", key)
                    setting[key] = ($0 == key "=n" ? "# " key " is not set" : $0)
                }
                next
            }
            {
                key = $0
                sub(/^# /, "", key)
                sub(/=.*/, "", key)
                sub(/ is not set$/, "", key)
                if (key in setting) {
                    print setting[key]
                    delete setting[key]
                } else {
                    print
                }
            }
            END { for (key in setting) print setting[key] }
        ' "$vendor/$fragment" .config > config.new || exit 1
        mv config.new .config || exit 1
        # The fragment sets values on top of allnoconfig, so take defaults for any
        # newly visible options with the oldconfig recipe's native configurator.
        mark "oldconfig" >&3
        awk 'BEGIN { for (i = 0; i < 4096; i++) print "" }' |
            env -i PATH="$PATH" LC_ALL=C TMPDIR="$output/tmp" scripts/kconfig/conf -o Config.in || exit 1
        # Preserve Kbuild's separation between config and generated header mtimes.
        sleep 1
    ) > "$output/config.log" 2>&1 || { cat "$output/config.log" >&2; fail 'configure failed'; }
    mark "applets/usage and applets/applet_tables"
    logged_make config.log applets/applets.o V=1
    receipts busybox-1.37.0/.config busybox-1.37.0/include/autoconf.h \
        busybox-1.37.0/include/usage_compressed.h busybox-1.37.0/include/applet_tables.h \
        busybox-1.37.0/applets/applets.o busybox-1.37.0/applets/built-in.o
    report_time configure
}

run_inventory() {
    begin inventory
    needs configure
    check_settings
    cd "$source"
    mark "inventory object goals"
    # The native configuration and applet generators have left their headers
    # current. Only the pinned configuration's printed target compile commands
    # enter the group table. Preserve recipe order and remove duplicates.
    make_busybox -n applets/built-in.o V=1 > "$output/goals.log" 2>&1 ||
        { tail -n 12 "$output/goals.log" >&2; fail 'Kbuild object inventory failed'; }
    awk -v cc="$bin/cc" '
        index($0, cc " --target=") && / -c / {
            for (i = 1; i < NF; i++)
                if ($i == "-o" && $(i+1) ~ /^[A-Za-z0-9_.\/+-]+\.o$/) {
                    path = $(i+1)
                    sub(/^\.\//, "", path)
                    if (!seen[path]++ && path != "applets/applets.o") {
                        if (path ~ /^libbb\//)
                            print path > libbb
                        else
                            print path > applets
                    }
                }
        }
    ' libbb="$output/libbb.goals" applets="$output/applets.goals" "$output/goals.log"
    [ "$(wc -l < "$output/libbb.goals")" -eq 128 ] || fail 'Kbuild libbb inventory differs from the pinned config'
    [ "$(wc -l < "$output/applets.goals")" -eq 94 ] || fail 'Kbuild applet inventory differs from the pinned config'
    cp "$predecessor" "$receipt.new"
    (cd "$output" && sha256sum libbb.goals applets.goals) >> "$receipt.new" ||
        fail "cannot record $step"
    mv "$receipt.new" "$receipt"
    report_time inventory
}

# Keep the pinned Kbuild recipes and its exported build configuration. A
# second makefile dispatches to the original directory rules with just the
# listed object goals, avoiding the top-level directory sweep.
directory_recipe() {
    printf '\t$(MAKE) -f scripts/Makefile.build obj=%s%s\n' "$1" "$2"
    if [ "$1" != "$3" ]; then
        printf '\t$(MAKE) -f scripts/Makefile.build obj=%s\n' "$1"
        printf '%s\n' "$1/built-in.o" >> "$output/directory-products"
    fi
}

group_recipe() {
    next_directory=$1
    shift
    printf '.PHONY: e25-object-group\ne25-object-group:\n'
    directory=
    directory_objects=
    for object; do
        if [ -n "$directory" ] && [ "${object%/*}" != "$directory" ]; then
            directory_recipe "$directory" "$directory_objects" "${object%/*}"
            directory_objects=
        fi
        directory=${object%/*}
        directory_objects="$directory_objects $object"
    done
    directory_recipe "$directory" "$directory_objects" "$next_directory"
}

build_objects() {
    step_name=$1 kind=$2 first=$3 last=$4 predecessor_name=$5
    begin "$step_name"
    needs "$predecessor_name"
    check_settings
    cd "$source"
    list=$output/$kind.goals
    set --
    for goal in $(sed -n "${first},${last}p" "$list"); do
        [ ! -e "$goal" ] || fail "$step: object already exists: $goal"
        set -- "$@" "$goal"
    done
    [ "$#" -gt 0 ] || fail "$step: empty object group"
    mark "$step objects $first-$last"
    : > "$output/directory-products"
    case "$step" in
        build-libbb)
            printf '.PHONY: e25-object-group\ne25-object-group:\n' > "$output/objects.mk"
            printf '\t$(MAKE) -f scripts/Makefile.build obj=libbb\n' >> "$output/objects.mk"
            printf '%s\n' libbb/built-in.o > "$output/directory-products"
            ;;
        *)
            next=$(sed -n "$((last + 1))p" "$list")
            group_recipe "${next%/*}" "$@" > "$output/objects.mk"
            ;;
    esac
    logged_make compile.log -f "$output/objects.mk" -j "$jobs" e25-object-group V=1
    # Retain predecessor hashes and record new objects and directory outputs.
    cp "$predecessor" "$receipt.new"
    goals=$*
    set --
    for goal in $goals; do
        set -- "$@" "busybox-1.37.0/$goal"
    done
    while IFS= read -r product; do
        set -- "$@" "busybox-1.37.0/$product"
        archive=${product%/*}/lib.a
        if [ -f "$archive" ]; then
            set -- "$@" "busybox-1.37.0/$archive"
        fi
    done < "$output/directory-products"
    (cd "$output" && sha256sum "$@") >> "$receipt.new" || fail "$step: missing object"
    mv "$receipt.new" "$receipt"
    report_time "$step"
}

run_link() {
    begin link-and-list
    if [ -f "$output/build-applets.sha256" ]; then needs build-applets
    else needs build-applets-4
    fi
    check_settings
    cd "$source"
    mark "static link"
    compile_ok=true
    # Exclude Kbuild's intermediate links and native host generators.
    # Pass paths directly to bld so libbb/time.o and miscutils/time.o stay
    # distinct. bar's replace operation matches by archive member name.
    find . -name '*.o' ! -path './scripts/*' ! -name built-in.o > "$output/objects.unsorted"
    sort "$output/objects.unsorted" > "$output/objects.txt"
    set --
    while IFS= read -r object; do
        set -- "$@" "$object"
    done < "$output/objects.txt"

    core_links=false
    : > "$output/link.log"
    : > "$output/link.map"
    : > "$output/object-symbols.txt"
    if [ "$#" -gt 0 ]; then
        "$bin/nm" -g -P "$@" > "$output/object-symbols.txt"
    fi
    if [ -f "$bundle/lib/libbrt.a" ]; then
        set -- "$@" "$bundle/lib/libbrt.a"
    fi
    if [ -s "$output/objects.txt" ] &&
        "$bin/ld" -T "$bundle/linker.ld" --gc-sections -Map "$output/link.map" -o "$output/busybox.elf" \
            "$@" "$bundle/lib/libc.a" > "$output/link.log" 2>&1; then
        core_links=true
    fi
    report_time link

    start=$(date +%s)
    # The reference build associates each enabled applet declaration with its
    # translation unit and entry symbol. bld emits a stripped executable, so its
    # live-symbol map identifies the compiled definitions that reached the link.
    find . -name '*.c' ! -path './scripts/*' -exec awk '
        match($0, /\/\/applet:IF_[A-Z0-9_]+\([[:space:]]*APPLET(_NOEXEC|_NOFORK|_ODDNAME|_SCRIPTED)?\(/) {
            prefix = substr($0, RSTART, RLENGTH)
            fields = substr($0, RSTART + RLENGTH)
            cfg = prefix
            sub(/^\/\/applet:IF_/, "", cfg)
            sub(/\(.*/, "", cfg)
            sub(/^[[:space:]]*/, "", fields)
            split(fields, args, /[[:space:]]*,[[:space:]]*/)
            entry = (prefix ~ /APPLET(_SCRIPTED)?\($/ ? args[1] : args[2])
            object = FILENAME
            sub(/\.c$/, ".o", object)
            print args[1], cfg, entry "_main", object
        }
    ' {} + > "$output/declared-applets.txt"
    awk -v compiled="$compile_ok" -v linked="$core_links" -v dir="$output" '
        function checked(ok) { return ok ? "true" : "false" }
        function quote(s, result, i, ch) {
            result = "\""
            for (i = 1; i <= length(s); i++) {
                ch = substr(s, i, 1)
                if (ch == "\\" || ch == "\"") ch = "\\" ch
                else if (ch == "\t") ch = "\\t"
                else if (ch == "\r") ch = "\\r"
                result = result ch
            }
            return result "\""
        }
        function append(list, value) {
            return list (list == "" ? "" : ", ") quote(value)
        }
        FILENAME == ".config" && /^CONFIG_[A-Z0-9_]+=y$/ {
            enabled[substr($0, 8, length($0) - 9)] = 1
        }
        FILENAME == dir "/objects.txt" { objects[$0] = 1 }
        FILENAME == dir "/object-symbols.txt" && $2 ~ /^[A-Z]$/ && $2 != "U" { symbols[$1] = 1 }
        FILENAME == dir "/link.map" && ($2 == "strong" || $2 == "weak") { live[$1] = 1 }
        FILENAME == dir "/compile.log" && compiled == "false" {
            if (match($0, /target \047[^\047]+\.o\047: recipe exited/)) {
                object_error = substr($0, RSTART, RLENGTH)
                sub(/^target \047/, "", object_error)
                sub(/\047: recipe exited$/, "", object_error)
                if (!(object_error in seen_object)) {
                    seen_object[object_error] = 1
                    objects_failed = append(objects_failed, object_error)
                }
            }
            if ($0 ~ /\.[chS]:[0-9]+:[0-9]+: (fatal )?error: / && error_count < 60) {
                compile_error = $0
                sub(/:[0-9]+:[0-9]+: (fatal )?error: /, ": ", compile_error)
                if (!(compile_error in seen_error)) {
                    seen_error[compile_error] = 1
                    compile_errors = append(compile_errors, compile_error)
                    error_count++
                }
            }
        }
        FILENAME == dir "/link.log" && /: undefined symbol$/ {
            symbol = $(NF - 2)
            sub(/:$/, "", symbol)
            core_missing = append(core_missing, symbol)
        }
        FILENAME == dir "/declared-applets.txt" && ($2 in enabled) {
            if ($1 in seen) next
            seen[$1] = 1
            object = $4 in objects
            resolved = object && ($3 in symbols) && ($3 in live)
            missing = ""
            if (object && !resolved)
                missing = quote(linked == "true" ? $3 : "(final link failed)")
            if (count++) print ","
            printf "    %s: {\"config\": %s, \"compiles\": %s, \"links\": %s, \"missing\": [%s]}",
                quote($1), quote($2), checked(object), checked(resolved), missing
            if (!resolved) failed = 1
        }
        BEGIN { print "{\n  \"applets\": {" }
        END {
            print "\n  },"
            printf "  \"compile_ok\": %s,\n", compiled
            if (linked == "false" && core_missing == "")
                core_missing = quote("(see link.log)")
            printf "  \"core\": {\"links\": %s, \"missing\": [%s]},\n", linked, core_missing
            printf "  \"compile_errors\": [%s],\n  \"objects_failed\": [%s]\n}\n",
                compile_errors, objects_failed
            if (!count || failed) exit 1
        }
    ' .config "$output/objects.txt" "$output/object-symbols.txt" "$output/link.map" \
        "$output/compile.log" "$output/link.log" \
        "$output/declared-applets.txt" > "$output/applets.json" ||
        report_failed=1
    report_time report
    printf 'tree_kib=%s\n' "$(du -sk "$output" | awk '{print $1}')"

    if [ "$compile_ok" != true ]; then
        cat "$output/compile.log" >&2
        fail "compile failed, see $output/applets.json"
    fi
    if [ "$core_links" != true ]; then
        [ ! -f "$output/link.log" ] || cat "$output/link.log" >&2
        fail "link failed, see $output/applets.json"
    fi
    [ "${report_failed:-0}" -eq 0 ] || fail "applets missing from output, see $output/applets.json"
    receipts busybox.elf applets.json
}

run_phase() {
    case "$1" in
        unpack) run_unpack ;;
        patch) run_patch ;;
        generators) run_generators ;;
        configure) run_configure ;;
        inventory) run_inventory ;;
        unpack-configure) run_unpack; run_patch; run_generators; run_configure; run_inventory ;;
        build-libbb) build_objects "$1" libbb 1 128 inventory ;;
        build-libbb-1) build_objects "$1" libbb 1 32 inventory ;;
        build-libbb-2) build_objects "$1" libbb 33 64 build-libbb-1 ;;
        build-libbb-3) build_objects "$1" libbb 65 96 build-libbb-2 ;;
        build-libbb-4) build_objects "$1" libbb 97 128 build-libbb-3 ;;
        build-applets)
            if [ -f "$output/build-libbb-4.sha256" ]; then last_libbb=build-libbb-4
            else last_libbb=build-libbb
            fi
            build_objects "$1" applets 1 94 "$last_libbb" ;;
        build-applets-1) build_objects "$1" applets 1 32 build-libbb-4 ;;
        build-applets-2) build_objects "$1" applets 33 64 build-applets-1 ;;
        build-applets-3) build_objects "$1" applets 65 79 build-applets-2 ;;
        build-applets-4) build_objects "$1" applets 80 94 build-applets-3 ;;
        link-and-list) run_link ;;
        *) fail "unknown phase: $1" ;;
    esac
}

if [ "$phase" = all ]; then
    for phase in unpack-configure build-libbb build-applets link-and-list; do
        run_phase "$phase"
    done
else
    run_phase "$phase"
fi
