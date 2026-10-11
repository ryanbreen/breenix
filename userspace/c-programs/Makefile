# Build the canonical ARM64 C tests with a real musl cross toolchain.
# MUSL_CC may select an equivalent installed compiler.

BREENIX_ROOT := $(shell cd ../.. && pwd)

# Use the installed musl cross toolchain, including its CRT and builtins.
MUSL_CC ?= aarch64-linux-musl-gcc
CFLAGS := -O2 -Wall -Wextra -Werror
LDFLAGS := -static -no-pie -Wl,--build-id=none

# Output directory
AARCH64_DIR := $(BREENIX_ROOT)/userspace/programs/aarch64

# Programs
PROGRAMS := hello_musl env_musl_test uname_musl_test rlimit_musl_test identity_musl_test

.PHONY: all install clean

all: $(addsuffix .elf,$(PROGRAMS))

%.o: %.c
	$(MUSL_CC) $(CFLAGS) -c $< -o $@

hello_musl.elf: hello.o
	$(MUSL_CC) $(LDFLAGS) $< -o $@

env_musl_test.elf: env_test.o
	$(MUSL_CC) $(LDFLAGS) $< -o $@

uname_musl_test.elf: uname_test.o
	$(MUSL_CC) $(LDFLAGS) $< -o $@

rlimit_musl_test.elf: rlimit_test.o
	$(MUSL_CC) $(LDFLAGS) $< -o $@

identity_musl_test.elf: identity_test.o
	$(MUSL_CC) $(LDFLAGS) $< -o $@

install: all
	mkdir -p "$(AARCH64_DIR)"
	for prog in $(PROGRAMS); do cp $$prog.elf $(AARCH64_DIR)/$$prog.elf; done

clean:
	rm -f *.o *.elf
