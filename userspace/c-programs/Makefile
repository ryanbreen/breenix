# Build the canonical C tests with a real musl toolchain for each target.
# MUSL_CC may select an equivalent installed compiler.

BREENIX_ROOT := $(abspath $(dir $(lastword $(MAKEFILE_LIST)))/../..)

# Use the installed musl cross toolchain, including its CRT and builtins.
ARCH ?= aarch64
MUSL_CC ?= $(if $(filter x86_64,$(ARCH)),musl-gcc,aarch64-linux-musl-gcc)
BUILD_DIR := build/$(ARCH)
CFLAGS := -O2 -Wall -Wextra -Werror
LDFLAGS := -static -no-pie -Wl,--build-id=none -Wl,-Ttext-segment=0x40000000

# Output directory
INSTALL_DIR := $(BREENIX_ROOT)/userspace/programs$(if $(filter aarch64,$(ARCH)),/aarch64,)

# Programs
PROGRAMS := hello_musl env_musl_test uname_musl_test rlimit_musl_test identity_musl_test

.PHONY: all install clean

all: $(addprefix $(BUILD_DIR)/,$(addsuffix .elf,$(PROGRAMS)))

$(BUILD_DIR):
	mkdir -p "$@"

$(BUILD_DIR)/%.o: %.c Makefile | $(BUILD_DIR)
	$(MUSL_CC) $(CFLAGS) -c $< -o $@

$(BUILD_DIR)/hello_musl.elf: $(BUILD_DIR)/hello.o Makefile
	$(MUSL_CC) $(LDFLAGS) $< -o $@

$(BUILD_DIR)/env_musl_test.elf: $(BUILD_DIR)/env_test.o Makefile
	$(MUSL_CC) $(LDFLAGS) $< -o $@

$(BUILD_DIR)/uname_musl_test.elf: $(BUILD_DIR)/uname_test.o Makefile
	$(MUSL_CC) $(LDFLAGS) $< -o $@

$(BUILD_DIR)/rlimit_musl_test.elf: $(BUILD_DIR)/rlimit_test.o Makefile
	$(MUSL_CC) $(LDFLAGS) $< -o $@

$(BUILD_DIR)/identity_musl_test.elf: $(BUILD_DIR)/identity_test.o Makefile
	$(MUSL_CC) $(LDFLAGS) $< -o $@

install: all
	mkdir -p "$(INSTALL_DIR)"
	for prog in $(PROGRAMS); do cp $(BUILD_DIR)/$$prog.elf $(INSTALL_DIR)/$$prog.elf; done

clean:
	rm -f $(BUILD_DIR)/*.o $(BUILD_DIR)/*.elf
