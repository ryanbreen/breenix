arch ?= x86_64
target ?= $(arch)-unknown-none-gnu
rust_os := target/$(target)/debug/libbreenix.a
kernel := build/kernel-$(arch).bin
iso := build/os-$(arch).iso

linker_script := src/arch/$(arch)/linker.ld
grub_cfg := src/arch/$(arch)/grub.cfg
assembly_source_files := $(wildcard src/arch/$(arch)/*.asm)
assembly_object_files := $(patsubst src/arch/$(arch)/%.asm, build/arch/$(arch)/%.o, $(assembly_source_files))

.PHONY: all clean run iso isovagrant libcore patch_libcore

all: $(iso)

clean:
	@rm -r target

run: $(iso)
	@qemu-system-x86_64 -hda $(iso) -m 5G

iso: $(iso)

$(iso): $(kernel) $(grub_cfg)
	@mkdir -p build/isofiles/boot/grub
	@cp $(kernel) build/isofiles/boot/kernel.bin
	@cp $(grub_cfg) build/isofiles/boot/grub
	@grub-mkrescue -o $(iso) build/isofiles 2> /dev/null
	@rm -r build/isofiles

$(kernel): cargo $(rust_os) $(assembly_object_files) $(linker_script)
	@x86_64-elf-ld -n --gc-sections -T $(linker_script) -o $(kernel) $(assembly_object_files) $(rust_os)

cargo:
	@cargo rustc --target $(target) -- -Z no-landing-pads -C no-redzone

patch_libcore:
	@cp libcore_nofp.patch core/src
	@cd core/src
	@patch -p1 <libcore_nofp.patch
	@cd -

libcore: $(patch_libcore)
	@rustc --target x86_64-unknown-none-gnu --cfg disable_float -Z no-landing-pads -C no-redzone core/src/lib.rs	
	@mv libcore.rlib ~/.multirust/toolchains/nightly/lib/rustlib/x86_64-unknown-none-gnu/lib/libcore.rlib

# compile assembly files
build/arch/$(arch)/%.o: src/arch/$(arch)/%.asm
	@mkdir -p $(shell dirname $@)
	@nasm -felf64 $< -o $@
