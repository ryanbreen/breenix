#!/bin/bash
# Host-side diagnostics; a requested bridged adapter can still start disconnected.
vmware_network_warning() {
    echo "VMWARE NETWORK HOST WARNING: $VMWARE_NETWORK_REASON; guest networking is unavailable for this boot (vmware.log's VNET lines give Fusion's reason)."
}

vmware_check_network_log() {
    [ -r "$1" ] || return 0
    if grep -Eiq 'Could not connect Ethernet0|Ethernet0.*(start disconnected|disconnect(ed)?)' "$1"; then
        if [ "${VMWARE_NETWORK_REASON:-}" != "Fusion reported an Ethernet0 connection failure" ]; then
            VMWARE_NETWORK_REASON="Fusion reported an Ethernet0 connection failure"
            vmware_network_warning
        fi
    fi
}

vmware_network_summary() {
    [ -z "${VMWARE_NETWORK_REASON:-}" ] || vmware_network_warning
}
