#!/bin/bash
# Print this host's MAC address for Breenix VMware VMs, in VMware's static range
# (00:50:56:00:00:00-00:50:56:3f:ff:ff), derived from the Mac's hardware UUID.
#
# A real machine keeps its MAC. With a MAC generated for each VM, every boot was a
# new client to the bridged network's DHCP server, which probes a fresh address
# before it offers one (3.6-5 s measured, sometimes past the guest's 10 s DHCP
# window) and leases one more address per boot. With one address per host, the
# server answers from the lease the previous boot held (0.3 s measured). Breenix
# VMware boots hold the host's mac-boot slot, so no two share the address at once.
set -euo pipefail
uuid="$(ioreg -rd1 -c IOPlatformExpertDevice | awk -F'"' '/IOPlatformUUID/ {print $4}')"
[ -n "$uuid" ] || { echo "static-mac.sh: no IOPlatformUUID" >&2; exit 1; }
hash="$(printf 'breenix-vmware:%s' "$uuid" | shasum -a 256 | cut -c1-6)"
printf '00:50:56:%02x:%s:%s\n' $((0x${hash:0:2} & 0x3f)) "${hash:2:2}" "${hash:4:2}"
