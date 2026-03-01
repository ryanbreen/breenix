# SPDX-License-Identifier: GPL-2.0-or-later
#
# Out-of-tree build for breenix_xhci_probe Linux kernel module.
# Build on the target VM with: make
# Cross-compile with: make KDIR=/path/to/kernel/build

ifneq ($(KERNELRELEASE),)
obj-m := breenix_xhci_probe.o
ccflags-y := -DDEBUG
else
KDIR ?= /lib/modules/$(shell uname -r)/build
default:
	$(MAKE) -C $(KDIR) M=$(PWD) modules
clean:
	$(MAKE) -C $(KDIR) M=$(PWD) clean
endif
