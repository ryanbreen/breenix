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
