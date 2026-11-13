# Sharing a Gus Kit canvas

On the hosting Mac, start the desktop with a collaboration forward:

```sh
scripts/boot-interactive.sh --mode desktop --gpu-gl --collab-host
```

The host port defaults to 7890. To use another host port, pass it after
`--collab-host` (for example, `--collab-host 17890`); the guest still listens
on 7890. The launcher checks that the host port can be bound before starting.
Open Gus Kit, choose **Session → Host Session...**, enter a name and start the
session. If macOS asks whether to allow incoming connections for QEMU, allow
them in the hosting Mac's firewall prompt.

On the joining Mac, start the desktop and open Gus Kit. Choose **Session → Join
Session...**, enter the hosting Mac's LAN IPv4 address in Address and your name
in Name, then join. The address defaults to port 7890; append `:PORT` when the
host chose another port. Use the Mac's LAN address, not the guest's address.

If macOS requests Local Network access on the joining Mac, allow it for the QEMU
app. Check **System Settings → Privacy & Security → Local Network** if access
was denied. The GL launcher uses **QEMU GL** (`QEMU-GL.app`); the non-GL Cocoa
launcher uses **QEMU** (`QEMU-1x-qemu-system-aarch64.app`, created by
`scripts/qemu-1x-app.sh`). Grant access to the app actually used for that boot.
These permission and firewall decisions are made by the operator in macOS.
