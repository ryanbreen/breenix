# Build C programs with musl libc for Breenix (aarch64)
#
# Prerequisites:
#   - Homebrew LLVM (clang, llvm-ar, llvm-ranlib) at /opt/homebrew/opt/llvm/bin/
#   - LLD linker at /opt/homebrew/Cellar/lld@20/20.1.8/bin/ld.lld
#   - musl libc installed at third-party/musl-install/
#   - compiler-rt builtins at third-party/compiler-rt-builtins/
#
# Build:
#   make                # build hello_musl.elf
#   make install        # copy to userspace/programs/aarch64/
#   make clean

BREENIX_ROOT := $(shell cd ../.. && pwd)

# Toolchain
LLVM := /opt/homebrew/opt/llvm/bin
CC := $(LLVM)/clang
AR := $(LLVM)/llvm-ar
LLD := /opt/homebrew/Cellar/lld@20/20.1.8/bin/ld.lld
CLANG_INCLUDE := $(shell $(CC) --target=aarch64-linux-musl -print-resource-dir)/include

# musl libc
MUSL_INSTALL := $(BREENIX_ROOT)/third-party/musl-install
MUSL_LIB := $(MUSL_INSTALL)/lib
MUSL_INCLUDE := $(MUSL_INSTALL)/include

# compiler-rt builtins (for 128-bit float operations)
RT_LIB := $(BREENIX_ROOT)/third-party/compiler-rt-builtins

# Breenix linker script
LINKER_SCRIPT := $(BREENIX_ROOT)/userspace/programs/linker-aarch64-musl.ld

# Compiler flags
CFLAGS := --target=aarch64-linux-musl -ffreestanding -O2 -nostdinc -isystem $(MUSL_INCLUDE)

# Linker flags
LDFLAGS := -T $(LINKER_SCRIPT) --static \
	$(MUSL_LIB)/crt1.o $(MUSL_LIB)/crti.o \
	-L$(MUSL_LIB) -lc \
	-L$(RT_LIB) -lcompiler_rt_builtins \
	$(MUSL_LIB)/crtn.o

# Output directory
AARCH64_DIR := $(BREENIX_ROOT)/userspace/programs/aarch64

# Programs
PROGRAMS := hello_musl env_musl_test uname_musl_test rlimit_musl_test

.PHONY: all install clean

all: $(addsuffix .elf,$(PROGRAMS))

%.o: %.c
	$(CC) $(CFLAGS) -c $< -o $@

hello_musl.elf: hello.o
	$(LLD) $(LDFLAGS) $< -o $@

env_musl_test.elf: env_test.o
	$(LLD) $(LDFLAGS) $< -o $@

uname_musl_test.elf: uname_test.o
	$(LLD) $(LDFLAGS) $< -o $@

rlimit_musl_test.elf: rlimit_test.o
	$(LLD) $(LDFLAGS) $< -o $@

install: all
	for prog in $(PROGRAMS); do cp $$prog.elf $(AARCH64_DIR)/$$prog.elf; done

clean:
	rm -f *.o *.elf
