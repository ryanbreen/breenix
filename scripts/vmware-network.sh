#!/bin/bash
# Host-side diagnostics; a requested NAT adapter can still start disconnected.
vmware_network_warning() {
    echo "VMWARE NETWORK HOST WARNING: $VMWARE_NETWORK_REASON; guest networking is unavailable until the host configuration is repaired."
    echo 'VMWARE NETWORK HOST WARNING: quit Fusion, run sudo "/Applications/VMware Fusion.app/Contents/Library/vmnet-cli" --configure, then reopen Fusion.'
}

vmware_check_nat_config() {
    VMWARE_NETWORK_REASON=""
    if [ ! -r "$1" ] || ! grep -Eq '^[[:space:]]*answer[[:space:]]+VNET_8_NAT[[:space:]]+yes[[:space:]]*$' "$1"; then
        VMWARE_NETWORK_REASON="Fusion has no readable enabled vmnet8 NAT entry"
        vmware_network_warning
    fi
}

vmware_check_network_log() {
    [ -r "$1" ] || return 0
    if grep -Eiq 'unable to load vmnet DB|Could not connect Ethernet0|Ethernet0.*(start disconnected|disconnect(ed)?)' "$1"; then
        if [ "${VMWARE_NETWORK_REASON:-}" != "Fusion reported an Ethernet0 connection failure" ]; then
            VMWARE_NETWORK_REASON="Fusion reported an Ethernet0 connection failure"
            vmware_network_warning
        fi
    fi
}

vmware_network_summary() {
    [ -z "${VMWARE_NETWORK_REASON:-}" ] || vmware_network_warning
}
