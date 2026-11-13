# A.5 u6a: SMB loopback forward

Coordinator ledger line 459 attributes #2878 to host Local Network privacy.
The QEMU launcher starts a dedicated SSH loopback forward for SMB selections,
waits for its listener, and stops the owned process on exit. Staging gives
both the server file and share mounts the slirp host address and forwarded
port. The private destination remains in the site configuration, not in
staged guest files or launcher output. SSH connection sharing is disabled so
the recorded process owns the forward.

The check in commit 8aabd731 fails on af30ce3d because the launcher has no
forward startup. With the implementation, the fake-SSH test passes its normal,
error and non-SMB selections, reads the actual staged server file, and checks
launcher ordering. The existing staging and import checks passed (4 tests);
the staging test now checks the deliberately changed destination and retains
its metadata and credential comparisons. Shell syntax and diff checks passed.
The scoped x86_64 Linux release build of smbfs completed without warnings.

The unsigned host smbfs probe from u6 connected through the dedicated loopback
forward, negotiated dialect 0x311, and exited 0. An earlier attempt reused an
existing SSH multiplexing master, so its probe result is inconclusive; the
implementation disables connection sharing and the recorded successful check
used its own process. No endpoint is recorded here. No guest boot was run.
