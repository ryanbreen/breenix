arch ?= x86_64
target ?= $(arch)-breenix
rust_os := target/$(target)/debug/libbreenix.a
kernel := build/kernel-$(arch).bin
iso := build/os-$(arch).iso

linker_script := src/arch/$(arch)/linker.ld
grub_cfg := src/arch/$(arch)/grub.cfg
assembly_source_files := $(wildcard src/arch/$(arch)/*.asm)
assembly_object_files := $(patsubst src/arch/$(arch)/%.asm, build/arch/$(arch)/%.o, $(assembly_source_files))

.PHONY: all clean run iso kernel libcore patch_libcore rust_libs liballoc librustc_unicode libcollections

all: $(iso)

clean:
	@rm -r target

run: $(iso)
	@qemu-system-x86_64 -smp 1 -hda $(iso) -m 5G -net nic,macaddr=52:54:be:36:42:a9 -device rtl8139,mac=02:00:00:11:11:11 -serial stdio 2>&1

run_once: $(iso)
	@qemu-system-x86_64 -smp 1 -hda $(iso) -m 5G -no-shutdown -no-reboot -serial stdio -d int 2>&1

debug: $(iso)
	@qemu-system-x86_64 -smp 1 -hda $(iso) -m 5G -no-reboot -s -S -serial stdio 2>&1

gdb:
	@../gdb/rust-os-gdb/bin/rust-gdb "build/kernel-x86_64.bin" -ex "target remote :1234"

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
	xargo build --target $(target)

# compile assembly files
build/arch/$(arch)/%.o: src/arch/$(arch)/%.asm
	@mkdir -p $(shell dirname $@)
	@nasm -felf64 $< -o $@
