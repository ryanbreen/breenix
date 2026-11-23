.POSIX:
# Build one category with the pinned Breenix tool paths. FUNCTIONS is the list
# of external functions declared by the public libc headers, one per line.
# The host fixture obtains it from scripts/libc-symbol-audit.py. AR names
# the tool used to prepare the prebuilt input archives.
CC=cc
AS=as
LD=ld
AR=ar
NM=nm
ARCH=x86_64
SUITE=userspace/libc-suite
INCLUDE=libs/libbreenix-libc/include
CC_INCLUDE=toolchain/include
LINKER_SCRIPT=userspace/programs/linker.ld
LIBC=lib/libc.a
BRT=lib/libbrt.a
FUNCTIONS=libc-functions.txt
BUILD=build/libc-suite
CFLAGS=-std=c17 -fno-builtin -frounding-math -O2 -Wall -Werror
CPPFLAGS=-I "$(SUITE)" -I "$(INCLUDE)" -I "$(CC_INCLUDE)"

source_dirent-glob=dirent_glob
source_getopt-env=getopt_env
source_locale-wchar=locale_wchar
source_math=math
source_pthread=pthread
source_stdio=stdio
source_string-stdlib=string_stdlib
SOURCE=$(SUITE)/$(source_$(CATEGORY)).c
ELF=$(BUILD)/libc-$(CATEGORY)_test.elf
DATA=stdin stdout stderr optind optarg opterr optopt signgam environ timezone daylight tzname

all: $(ELF)
.PHONY: all

$(BUILD)/abi_check.o: $(SUITE)/abi_check.c
	mkdir -p "$(BUILD)"
	"$(CC)" --target=$(ARCH) $(CFLAGS) $(CPPFLAGS) -S "$(SUITE)/abi_check.c" -o "$@.s"
	"$(AS)" --target $(ARCH) -o "$@" "$@.s"

$(BUILD)/harness.o: $(SUITE)/harness.c $(SUITE)/case.h
	mkdir -p "$(BUILD)"
	"$(CC)" --target=$(ARCH) $(CFLAGS) $(CPPFLAGS) -S "$(SUITE)/harness.c" -o "$@.s"
	"$(AS)" --target $(ARCH) -o "$@" "$@.s"

$(BUILD)/$(CATEGORY).o: $(SOURCE) $(SUITE)/case.h
	mkdir -p "$(BUILD)"
	"$(CC)" --target=$(ARCH) $(CFLAGS) $(CPPFLAGS) -S "$(SOURCE)" -o "$@.s"
	"$(AS)" --target $(ARCH) -o "$@" "$@.s"

$(BUILD)/libc-nm.txt: $(LIBC)
	mkdir -p "$(BUILD)"
	LC_ALL=C; export LC_ALL; set -e; \
	"$(NM)" -Ag "$(LIBC)" > "$@.tmp"; \
	mv "$@.tmp" "$@"

# Ruling 52 follows the archive rule in scripts/libc-symbol-audit.py.
# Record the strong Breenix members for the subsequent link-map audit.
$(BUILD)/libc-strong.txt: $(BUILD)/libc-nm.txt $(FUNCTIONS)
	LC_ALL=C; export LC_ALL; set -e; \
	awk -v archive="$(LIBC)" '\
	    FILENAME == ARGV[1] { declared[$$1] = 1; next } \
	    NF < 4 || !($$NF in declared) { next } \
	    { \
	        name = $$NF; kind = $$(NF-1); \
	        member = substr($$1, length(archive) + 2, length($$1) - length(archive) - 3); \
	        if (index($$1, archive "[") != 1 || $$1 !~ /\]:$$/) { print "libc-suite: invalid archive record", $$0; bad = 1; next } \
	        defined[name] = 1; \
	        if (kind == "T" && member !~ /^compiler_builtins/) { \
	            strong[name] = 1; print name, archive "(" member ")"; \
	        } \
	    } \
	    END { \
	        for (name in defined) if (!(name in strong)) { \
	            print "libc-suite:", name ": no strong Breenix libc definition"; bad = 1; \
	        } \
	        if (bad) exit 1; \
	    }' "$(FUNCTIONS)" "$(BUILD)/libc-nm.txt" > "$@.tmp" \
	    || { cat "$@.tmp" >&2; exit 1; }; \
	mv "$@.tmp" "$@"

$(ELF): $(BUILD)/abi_check.o $(BUILD)/harness.o $(BUILD)/$(CATEGORY).o $(BUILD)/libc-strong.txt $(BRT) $(LINKER_SCRIPT) $(SUITE)/categories
	LC_ALL=C; export LC_ALL; set -e; \
	mode=$$(awk -v category="$(CATEGORY)" '\
	    !NF || /^ *#/ { next } \
	    NF > 2 || (NF == 2 && $$2 != "complete") { bad = 1 } \
	    $$1 == category { count++; print $$2 } \
	    END { if (bad || count != 1) exit 1 }' "$(SUITE)/categories") \
	    || { printf '%s\n' "libc-suite: invalid categories or missing $(CATEGORY)" >&2; exit 1; }; \
	base="$(BUILD)/$(CATEGORY)"; \
	shim="$(BUILD)/missing-$(CATEGORY)"; \
	"$(NM)" -gP "$$base.o" "$(BUILD)/harness.o" > "$$base.symbols"; \
	"$(NM)" -gP "$(BRT)" > "$$base.brt"; \
	awk '$$2 ~ /^(U|w|v)$$/ { print $$1 }' "$$base.symbols" > "$$base.undefined"; \
	sort -u -o "$$base.undefined" "$$base.undefined"; \
	awk '\
	    FILENAME == ARGV[1] { needed[$$1] = 1; next } \
	    NF != 4 { next } \
	    FILENAME == ARGV[2] { delete needed[$$1]; next } \
	    FILENAME == ARGV[3] { \
	        if ($$1 ~ /^__atomic_(load|store|exchange|compare_exchange|is_lock_free)$$/) delete needed[$$1]; \
	        next; \
	    } \
	    { delete needed[$$NF] } \
	    END { for (name in needed) print name }' \
	    "$$base.undefined" "$$base.symbols" "$$base.brt" "$(BUILD)/libc-nm.txt" > "$$base-missing.txt"; \
	sort -u -o "$$base-missing.txt" "$$base-missing.txt"; \
	missing_data=$$(awk -v data="$(DATA)" 'BEGIN { n = split(data, names); for (i = 1; i <= n; i++) known[names[i]] = 1 } $$1 in known { print }' "$$base-missing.txt"); \
	if [ -n "$$missing_data" ]; then \
	    printf '%s\n' "libc-suite: $(CATEGORY) uses data Breenix's C library does not define: $$(printf '%s\n' "$$missing_data" | paste -s -d ' ' -)" >&2; exit 1; \
	fi; \
	set -- "$$base.o" "$(BUILD)/harness.o"; \
	if [ -s "$$base-missing.txt" ]; then \
	    missing=$$(paste -s -d ' ' "$$base-missing.txt"); \
	    if [ "$$mode" = complete ]; then \
	        printf '%s\n' "libc-suite: $(CATEGORY) is complete but libc.a lacks: $$missing" >&2; exit 1; \
	    fi; \
	    printf '%s\n' "libc-suite: $(CATEGORY) stand-ins: $$missing"; \
	    awk 'BEGIN { print "void libc_case_missing(const char *name);" } \
	        !/^[A-Za-z_][A-Za-z_0-9]*$$/ { bad = 1; next } \
	        { printf "void %s(void) { libc_case_missing(\042%s\042); }\n", $$1, $$1 } \
	        END { if (bad) exit 1 }' "$$base-missing.txt" > "$$shim.c"; \
	    "$(CC)" --target=$(ARCH) $(CFLAGS) $(CPPFLAGS) -S "$$shim.c" -o "$$shim.s"; \
	    "$(AS)" --target $(ARCH) -o "$$shim.o" "$$shim.s"; \
	    set -- "$$@" "$$shim.o"; \
	fi; \
	trap 'rm -f "$@.tmp"' 0; \
	"$(LD)" -T "$(LINKER_SCRIPT)" --gc-sections -Map "$$base.map" -o "$@.tmp" "$$@" "$(BRT)" "$(LIBC)"; \
	awk -v shim="$$shim.o" '\
	    FILENAME == ARGV[1] { declared[$$1] = 1; next } \
	    FILENAME == ARGV[2] { strong[$$1, $$2] = 1; next } \
	    FILENAME == ARGV[3] { missing[$$1] = 1; next } \
	    FILENAME == ARGV[4] { if ($$1 in declared) required[$$1] = 1; next } \
	    !($$1 in declared) { next } \
	    { \
	        seen[$$1] = 1; \
	        if ($$2 == "strong" && (($$1, $$3) in strong || ($$1 in missing && $$3 == shim))) next; \
	        print "libc-suite:", $$1 ": linked definition is not from Breenix libc (" $$3 ")"; bad = 1; \
	    } \
	    END { \
	        for (name in required) if (!(name in seen)) { \
	            print "libc-suite:", name ": missing from the link map"; bad = 1; \
	        } \
	        if (bad) exit 1; \
	    }' "$(FUNCTIONS)" "$(BUILD)/libc-strong.txt" "$$base-missing.txt" "$$base.undefined" "$$base.map" >&2; \
	mv "$@.tmp" "$@"
