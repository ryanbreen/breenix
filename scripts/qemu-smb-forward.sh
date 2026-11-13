# Sourced by boot-interactive.sh; the caller owns the EXIT trap.
SMB_FORWARD_PID=
SMB_FORWARD_HOST=
SMB_FORWARD_PORT=
qemu_smb_forward_stop() {
    if [ -n "$SMB_FORWARD_PID" ]; then
        kill "$SMB_FORWARD_PID" 2>/dev/null || true
        wait "$SMB_FORWARD_PID" 2>/dev/null || true
        SMB_FORWARD_PID=
    fi
}
qemu_smb_forward_start() {
    local config
    config=$(PYTHONPATH="$ROOT/scripts:$ROOT/scripts/metal" python3 - "$1" "$2" "$3" <<'PY'
import socket
import sys
from metal.metal_root import smb_gui_selection
from vm_site import load_site
if smb_gui_selection(sys.argv[1], sys.argv[2], sys.argv[3] or None):
    site = load_site()
    host, port = site['SMB_TEST_HOST'], site['SMB_TEST_PORT']
    import re
    if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9.-]{0,252}', host) or not port.isdecimal() or not 1 <= int(port) <= 65535:
        raise ValueError('invalid private GUI SMB configuration')
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        local_port = sock.getsockname()[1]
    print(local_port, host, port)
PY
    ) || return
    [ -n "$config" ] || return 0
    local remote_host remote_port
    read -r SMB_FORWARD_PORT remote_host remote_port <<< "$config"
    SMB_FORWARD_HOST=10.0.2.2
    ssh -S none -N -o ExitOnForwardFailure=yes -L "127.0.0.1:$SMB_FORWARD_PORT:$remote_host:$remote_port" beast </dev/null >/dev/null 2>&1 &
    SMB_FORWARD_PID=$!
    # Wait for the listener, and fail before QEMU if ssh cannot establish it.
    local attempt
    for ((attempt=0; attempt<100; attempt++)); do
        kill -0 "$SMB_FORWARD_PID" 2>/dev/null || { echo 'SMB forward failed' >&2; return 1; }
        if python3 - "$SMB_FORWARD_PORT" <<'PYREADY'
import socket, sys
with socket.socket() as sock:
    sock.settimeout(.1)
    sys.exit(sock.connect_ex(('127.0.0.1', int(sys.argv[1]))) != 0)
PYREADY
        then return 0; fi
        sleep .1
    done
    echo 'SMB forward did not become ready' >&2
    return 1
}
