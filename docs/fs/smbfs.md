# SMB mounts

As root, create a root-owned mode 0600 credentials file containing `username=`,
`password=` and, optionally, `domain=` on separate lines. Mount a share with:

```text
mount -t smb //HOST/SHARE /mnt/share -o credfile=/etc/share.cred[,port=N][,vers=3.0.2]
```

The directory must already exist. The default port is 445 and the default
negotiation allows SMB 3.1.1. The daemon authenticates and connects the tree
before mounting; the printed READY line reports the negotiated dialect and
whether the connection requires encryption. Unmounting closes the serving
endpoint and the daemon exits. Credentials are read from the file, not passed
on the command line.


At boot, init runs `mount -a` after the network is up. Put one share per
line in `/etc/shares`, with four whitespace-separated fields:

```text
//HOST/SHARE /mnt/share smb credfile=/etc/share.cred,port=445
```

Blank lines and lines beginning with `#` are ignored. The mount directory must
already exist. Entries already present in `/proc/mounts` are skipped. Invalid
lines and failed mounts are reported on standard error; other entries are still
attempted and the command exits with status 1 if any failed. A missing
`/etc/shares` is silently accepted. Running `mount` without arguments lists
current mounts.
