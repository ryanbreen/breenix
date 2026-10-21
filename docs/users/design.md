# Users: Unix users, permissions and the standard layout

Design, 2026-10-20, from main at 718994f6 and the ssh-doors lane (branch `metal-fix/ssh-doors`, in progress). The
`users` effort in `docs/efforts/path.json` carries out section 7, and its suite manifest is `docs/suites/users.json`.
The program this belongs to is [the server plan](../metal/server-plan.md).

**Operator decisions and rulings** (2026-10-20, ET; the server plan records the full list):

- **D5** (7:20 PM): "a normal Unix file system layout ... I log in, not as root, I log in as myself ... permissions
  built into the system and non-root users ... a first class implementation." Full kernel credentials, accounts,
  su and a sudo-style admin path, the FHS layout with owners and modes, and the operator's own account on the
  console and over SSH.
- **D4:** LAN only, keys only, an operator account for daily use, and root over SSH off once that account exists.
- **S7:** the private site file supplies the operator's account name, group and home (uid 1000, login shell `sh`).
  Examples below use `operator` as a placeholder, not a fixed account name.
- **S8** (7:45 PM): machine state that must survive upgrades (SSH host keys, accounts, shadow entries, admin policy)
  lives in `/.breenix-shared` on the system partition, outside the generation trees, and is reconciled into `/etc`
  at boot (section 4); generations keep shipping `/etc` defaults.
- **S9:** root gets its own password from a private site field (`ROOT_PASSWORD_HASH`, a hash kept out of the repository) for
  su and the console; root cannot log in over SSH. A broken `operator` account is recovered by a test-root boot, with no
  back door in the persistent install.
- **D6** (8:00 PM): the effort covers each target. x86-64 on the R240 leads, and the catch-up brings ARM64 QEMU,
  Parallels and VMware to the same bar, each VM with its own persistent install (S10).

References: POSIX.1-2024 (XBD 4.5; XSH exec, setuid, kill, chmod, chown, utimensat, umask,
link, unlink, rename), FHS 3.0, the SHA-crypt specification, RFC 4252/4254, and the passwd(5), group(5) and
shadow(5) format descriptions. No other system's source is read.

## 1. What exists today, and the gaps

| Area | Today |
|---|---|
| Credentials | `ProcessCredentials` (`kernel/src/process/credentials.rs:19`) holds real, effective and saved uid and gid, plus a shared supplementary list. setuid, setgid, setreuid and setregid follow the saved-ID rules. Each commits to the thread group under one PM acquisition (`syscall/handlers.rs:3918`). setgroups is root-only. |
| Set-ID exec | S_ISUID, and S_ISGID with group execute, set the effective IDs; saved IDs follow; no identity is granted while the working directory is shared; AT_SECURE is reported. Both architectures check execute by class (`handlers.rs:2049`, `aarch64/syscall_entry.rs:1669`). |
| Access check | One class selection (`fs/permissions.rs:50`): owner, then group (effective or supplementary), then other, with no fall-through. Root bypasses read and write, and needs an x bit or a directory for execute. |
| Paths | Search is checked per component (`fs/namei.rs:531`) and on chdir (`fs.rs:3779`). The parent needs write and search for create, unlink, rename, link, mkdir, symlink and mknod (`fifo_policy.rs:13`). In a sticky directory, removal and rename replacement are limited to the file's owner, the directory's owner or root (`fs.rs:2019`). |
| Creation, chmod, chown | New files take the euid and the egid (or a set-GID parent's group), with `mode & ~umask`. chmod is owner or root; only root gives a file away; the owner may chgrp to a group they belong to; both clear set-ID bits on regular files (`syscall/metadata.rs:88`). |
| Signals | kill: root, or the sender's real or effective uid equals the target's real or saved uid. SIGCONT is allowed within the session (`syscall/signal.rs:295`). `may_renice` controls setpriority. |
| Other objects | SysV IPC, message queues, `/dev/shm`, FIFOs, ptys and AF_UNIX socket files check modes; device mknod is root-only. `/proc/kmsg` is root-only and `<pid>/exe` is checked (`procfs/mod.rs:1827`). ext2 stores 32-bit owners (`ext2/inode.rs:200`). |
| Suites | processes: 26 credential and set-ID cases; directories: 18 permission cases; signals: `kill-eperm`. |
| Login | login verifies sha512-crypt from `/etc/shadow` (staged 0600 by `scripts/metal/metal_root.py:182`), drops to the account and execs its shell. |
| SSH (ssh-doors, in progress) | `auth.rs` is publickey-only through an `Authorizer` trait, with Ed25519. The lane covers host keys in `/etc/ssh`, per-account `authorized_keys`, the bsshd privilege drop, the `operator` account and the end of telnetd. |

**Kernel gaps:**
- **K1:** `Credentials::current` starts at euid 0 (`permissions.rs:16`), so a user thread whose process lookup
  fails is checked as root.
- **K2:** setgroups and umask change only the caller's row (`handlers.rs:4001`, `:4042`), not the thread group.
- **K3:** utimensat makes no owner or write check (`fs.rs:4563`).
- **K4:** a non-owner's write or truncate leaves S_ISUID and S_ISGID set.
- **K5:** there is no setresuid, getresuid, setresgid or getresgid, in the kernel or in libc.
- **K6:** there are no mount flags (nosuid, nodev, noexec) and no mount syscall; `/home` honours set-ID bits.
- **K7:** devfs has no owners or modes. stat reports 0666 for devices (`fs.rs:1256`), and open makes no class
  check, so a user can open `/dev/console`, the 6 VTs and `/dev/vcs*`, and read another session's screen.
- **K8:** TCP bind lacks the below-1024 rule that UDP has (`udp.rs:440`, `tcp.rs:189`). A user could take port 22
  while bsshd is down.
- **K9:** I found no credential check on `/proc/<pid>/task/<tid>/kernel_output`, and `/proc/trace/*` is readable
  by any user.
- **K10:** there is no audited list of privileged operations. In the framebuffer, audio and GPU syscalls, only
  `graphics.rs` mentions euid (2 lines).
- **K11:** there is no ptrace; its rule is recorded below for when it lands.
- **K12:** `permits(&Ext2Inode)` and `ExecIdentity::of(&Ext2Inode)` tie the checks to ext2.

**Provenance and issues.** `credentials.rs:121` justifies a rule with a Linux kernel-internal name
(LSM_UNSAFE_SHARE). The rule should be argued from its hazard (a sharer can move the privileged program's
directory), and review should confirm the behaviour came from documentation. Issue #1089's list (parent checks,
sticky rename, exec class, chdir search) is largely present in the code above, but it is open without evidence;
the suite's `files` and `sticky` categories supply it.

**Userland and layout gaps:**
- login accepts only `root` (`login.rs:63`), loads no supplementary groups, leaves the terminal's owner and sets
  no umask.
- Images have 2 accounts, root and nobody (`create_ext2_disk.sh:233`), no `/etc/shadow` and no wheel, tty, adm or
  users group. There is no useradd, userdel, usermod, passwd, su or sudo; BusyBox supplies id, whoami, chown,
  chgrp and chmod.
- `/tmp` is 0755 and not sticky (`create_ext2_disk.sh:264`). There is no `/root`, `/run`, `/var/tmp`, `/opt`,
  `/srv` or `/usr/sbin`, and test fixtures (`/test`, `/deep`) sit at the root.

**Persistent /etc is not machine state.** `persistent-install` keeps only `root` shared. It copies the new image's
other top-level directories, `etc` and `var` included, into each generation (`persistent_install.rs:13`,
`:198-205`). An upgrade therefore replaces accounts, password changes, SSH host keys and logs written on the box.
The ssh-doors brief assumes that generations keep `/etc`. Without a change, the host-key fingerprint changes at
each upgrade and strict host-key checking fails.

## 2. The kernel credential model

**Credentials.**
- Credentials stay per process. Privilege is euid 0, with no capability split (POSIX.1e was withdrawn).
- setgroups and umask go through the same thread-group commit as setuid (K2). umask lives in the shared
  filesystem context, so it follows CLONE_FS like the working directory.
- Add setresuid, setresgid, getresuid and getresgid on the existing ABI numbers (K5); unprivileged, each new ID
  must be one of the current three, so su and sudo drop in one verifiable call.
- `Credentials::current` returns an error, not root, for a user thread without a process (K1). Kernel threads get
  explicit kernel credentials.
- NGROUPS_MAX is 65536.

**exec** keeps today's permission and set-ID rules and adds two: set-ID bits are ignored on a nosuid mount (and
later under ptrace), and a set-ID exec with fd 0, 1 or 2 closed gets `/dev/null` there. AT_SECURE needs no loader
scrubbing because binaries are static.

**One check on neutral attributes.** `may(cred, attrs, want)` works on `{mode, uid, gid}` (K12), and the
operations below call it or the parent policy built on it:

| Operation | Rule |
|---|---|
| Path walk | Search on each directory component. |
| open | Read for O_RDONLY, write for O_WRONLY and O_TRUNC, both for O_RDWR. A new O_CREAT file needs write and search on the parent and takes its owner, group and umask as today. |
| mkdir, mknod, symlink, link | Write and search on the parent. Device mknod is root-only. Hard links to a file the caller neither owns nor can read and write are refused unless root (Breenix's documented /tmp hardening). |
| unlink, rmdir, rename | Write and search on each parent, plus the sticky rule. Moving a directory to a new parent needs write on the directory itself. |
| Symlinks in sticky world-writable directories | Followed only if the follower or the directory's owner owns the link; otherwise EACCES. |
| chmod, chown | As today. |
| write, truncate | A non-root caller's write or truncate clears S_ISUID and S_ISGID on a regular file (K4). |
| utimensat, futimens | Both times UTIME_NOW: owner, write permission, or root. Explicit times: owner or root, else EPERM. Both UTIME_OMIT: no check (K3). |
| Mounts | The kernel's mount table carries `ro`, `nosuid`, `nodev` and `noexec`, shown in `/proc/mounts` and statvfs `f_flag` (K6). `/home` is nosuid,nodev. |
| kill, priority, affinity | kill as today. Raising priority, real-time policies and changing another user's affinity need root. |
| ptrace (K11) | The tracer's real uid and gid must equal each of the target's uids and gids, and the target must not have gained an identity at exec. Otherwise root. |
| /proc | Pid directories stay listable. `exe`, `kernel_output` and any later `fd`, `environ` or `mem` use the ptrace-read rule (K9). `kmsg` and `trace/*` are root-only. Owners shown are the process's euid and egid. |
| devfs | Real owners and modes, checked at open (K7). `null`, `zero` (device names in `kernel/src/fs/devfs/mod.rs`<!-- claim-lint:ok: kernel/src/fs/devfs/mod.rs -->), `tty` and `ptmx` 0666; `console` 0600 root; `tty1`–`tty6` 0620 root:tty, given to the user at login and returned at logout; `vcs*` 0600 root; `rtc0` 0444. ext2 device inodes open no device. |
| fd passing | SCM_RIGHTS moves an open description with its access mode and no recheck, as POSIX intends. |
| Privileged operations | One list in `kernel/src/process/privilege.rs`: clock_settime, sethostname, chroot, raising hard rlimits, TCP and UDP ports below 1024 (K8), device mknod, framebuffer, audio and GPU calls outside the session's devices, reboot, and the trace controls. Refusals are EPERM, or EACCES for ports. |

**Storage.** ext2 keeps its mode and split 32-bit owners, with no ACLs. The filesystem effort's `fs-interface` exposes
`InodeAttrs { mode, uid, gid, nlink, times }` with the checks staying in the VFS; the CoW filesystem stores the same
attributes per inode and reserves an extended-attribute namespace for later ACLs.

## 3. Accounts

**Formats.**
- `/etc/passwd` (0644 root:root): `name:x:uid:gid:gecos:home:shell`
- `/etc/group` (0644 root:root): `name:x:gid:members`
- `/etc/shadow` (0600 root:root): `name:hash:lastchg:min:max:warn:inactive:expire:`

Rules:
- Parsing is strict, and a malformed line disables only that account.
- New hashes are sha512-crypt (`$6$`) at `rounds=100000`.
- A leading `!` locks the password but leaves key login. `*` means no password. A past `expire` refuses each login
  method. An empty hash is refused.
- Names match `[a-z_][a-z0-9_-]{0,31}`. System IDs are 0–999 and users start at 1000.

**One library.** `libs/libbreenix/src/accounts` parses and writes these files for login, bsshd, su, sudo, passwd
and the tools, and libc's getpw* and getgr* move onto it.

A change follows these steps:
1. Lock `/etc/.pwd.lock` (0600) with fcntl; libc's `lckpwdf` uses the same lock.
2. Write `/etc/<file>+` with the final owner and mode, then fsync it.
3. Rename it over the old file, then fsync `/etc`.

shadow is written before passwd under the same lock. A crash leaves each file old or new, and a passwd entry
without a shadow line cannot log in.

**Tools.** Ours, in Rust. BusyBox keeps id, whoami, chown, chgrp and chmod, and adds its `groups` applet.

| Tool | Path, mode | Behaviour |
|---|---|---|
| `useradd [-u] [-g] [-G] [-s] [-d] [-m] name` | `/usr/sbin`, 0755 | Root only. The next free uid ≥ 1000, a same-named group, a locked hash; `-m` makes the home 0700 from `/etc/skel`. |
| `userdel [-r]`, `usermod [-G -a -s -L -U -e]`, `groupadd`, `groupdel` | `/usr/sbin`, 0755 | userdel refuses while the user has processes, and `-r` removes only a home the user owns. |
| `passwd [name]` | `/usr/bin`, 4755 root | Your own password after the old one; root may set anyone's. At least 8 characters. |
| `su [-] [name]` | `/bin`, 4755 root | The target's password unless the caller is root. Becoming uid 0 is limited to `wheel`. `-` gives a login environment. |
| `sudo [-u user] [-i] [-n] cmd` | `/usr/bin`, 4755 root | The policy below. |

**sudo-style admin.** The command is named `sudo` for the operator's habit; the policy grammar is ours and claims
no sudoers compatibility.

- **Policy file:** `/etc/sudo.policy`, which must be 0440 root:root and must parse, or sudo refuses to run.
- **Grammar:** `permit (user|%group) [as user] [cmd /abs/path] [nopass]`, first match wins. The default policy is
  `permit %wheel as root`.
- **Authentication:** the caller's own password, cached 5 minutes in `/run/sudo/<uid>/<session>` for that session
  and terminal. `-n` does not prompt.
- **Execution:** the command resolves to an absolute path before the policy check. The environment is reset:
  PATH=/usr/sbin:/usr/bin:/sbin:/bin, the target's HOME and USER, TERM kept, SUDO_USER and SUDO_COMMAND set.
  umask is 022.

**Logging.** su, sudo, login, logout and account and password changes each append
`<ISO time> <program>[pid] user= target= tty= result=<ok|refused:reason> cmd=` to `/var/log/auth.log` (0640
root:adm, O_APPEND, fsync) and to `/dev/kmsg`. No password or hash is written.

**Identity, PAM-free.** login, su, sudo and bsshd share `accounts::session::become(account, tty)`:
1. Validate the account: a shadow line, not expired, a shell listed in `/etc/shells`.
2. Give the user the tty at 0620.
3. initgroups, setresgid, setresuid.
4. For a non-root target (uid != 0), confirm `setuid(0)` fails. For a root target, skip this no-regain-root
   assertion: su and sudo have intentionally become root, so `setuid(0)` succeeds.
5. Set the umask from `/etc/login.defs`.
6. Install a clean environment and chdir to the home.
7. Exec the login shell.

`/etc/login.defs` also holds `ROOT_LOGIN no` (login and bsshd refuse uid 0) and `FAIL_DELAY 2`.

On the ssh-doors base, bsshd's `Authorizer` resolves accounts through the library. It reads `~/.ssh/authorized_keys`
only when the file, `~/.ssh` and the home are owned by the account and not group- or world-writable, calls
`become`, and gives the user the PTY. The users effort adds the library and `ROOT_LOGIN` to that lane's work.

## 4. The standard filesystem layout

| Path | Owner, mode | Contents |
|---|---|---|
| `/bin` | root 0755 | BusyBox and its links, sh, bsh, getty, login, su |
| `/sbin` | root 0755 | init, persistent-install; `suite-<id>` on test roots, since the manifests name that path |
| `/usr/bin` | root 0755 | Other Breenix commands: bssh, bterm, bwm, passwd, sudo, later cc, as, ld, make |
| `/usr/sbin` | root 0755 | bsshd, blogd, ntp_client, the account tools |
| `/usr/lib`, `/usr/include` | root 0755 | SDK `libc.a`, linker script, headers (the toolchain effort) |
| `/usr/libexec/breenix` | root 0755 | Test binaries (from `/usr/local/test/bin`) and the `/test` and `/deep` fixtures; `/usr/local` is left to the administrator |
| `/usr/share` | root 0755 | fonts, `factory/etc`, doc |
| `/etc` | root 0755 | `shadow` 0600, `sudo.policy` 0440, `ssh/` with host keys 0600, `skel/`, `shells`, `login.defs` |
| `/root` | root 0700 | |
| `/home`, `/home/<user>` | root 0755; user:user 0700 | Mounted nosuid,nodev |
| `/tmp`, `/var/tmp` | root 1777 | `/tmp` is emptied at boot |
| `/var/log` | root 0755 | `auth.log` and `kernel.log` 0640 root:adm |
| `/var/lib`, `/var/spool`, `/var/cache` | root 0755 | Service state |
| `/run` | root 0755 | Emptied by init before services start |
| `/opt`, `/srv` | root 0755 | Empty |
| `/dev`, `/proc` | virtual | As in section 2 |

System groups: root 0, adm 4, tty 5, wheel 10, users 100, nogroup 65534. Placement becomes one table, used by
`create_ext2_disk.sh` (replacing its name matching) and by the installer. A host test compares a built image with
the table.

**The persistent install: shared machine state.**

1. **The generation holds the OS.** `/bin`, `/sbin` and `/usr` (with `usr/share/factory/etc`) and mount points.
   Its completion record becomes `BREENIX-USERLAND 2` and names the shared directories.
2. **Shared state.** `/.breenix-shared/{etc,var,opt,srv,tmp,run}` on the system partition, beside the existing
   shared `/root`. It is not at the partition root, which a legacy flat slot still uses. The generation namespace
   (`ext2/generation.rs`) already binds `etc` and the shared root; for v2 it binds each listed name. `/home` stays
   on the home partition.
3. **The first v2 install** seeds `/.breenix-shared/etc` and `var` from the current generation, so the ssh-doors
   host keys and the accounts carry over. The install intent gains `previous-userland <id>` from the manager's
   CURRENT slot, because the installer cannot otherwise tell which tree is current. Later installs leave the
   shared tree alone.
4. **`etc-reconcile`.** init runs it at boot, before services. `/etc/.breenix-factory` records which generation's
   factory files `/etc` holds and each file's SHA-256. When the selected generation differs, then for each
   factory file:
   - missing in `/etc`: install it;
   - unmodified (hash matches the record): replace it;
   - modified locally: keep it, write `<file>.factory` beside it and log it;
   - factory-provided, gone from the new set and unmodified: remove it.

   passwd, group, shadow, `ssh/*`, `sudo.policy` and `login.defs` are machine-owned: created only when absent,
   with required system accounts added through the library and existing lines left unchanged.
5. **Upgrade and rollback.** Reconciliation works in both directions. A rollback restores the old generation's
   unmodified factory files and keeps current accounts and passwords, so an old password does not come back.
   Rolling back to a v1 generation selects its frozen `/etc` without shared binds and boots as it does today;
   newer data waits in `/.breenix-shared` and `/home`. The installer's host fault-injection tests extend to
   seeding and to reconciliation interrupted at each operation.

**The ssh-doors bridge.** Until step 2 lands, an upgrade replaces `/etc/ssh`. Land ssh-doors as planned with this
stated in its PR, and finish the layout milestone before any upgrade that relies on a pinned host key.

## 5. The operator account

Example only: `operator:x:1000:1000:Operator:/home/operator:/bin/sh` (BusyBox ash), group `operator` 1000, a member of `wheel` (S7). Actual account values come from the private site file.

**Staging.**
- `install-persistent` writes the account into `/.breenix-shared/etc` from site fields: `OPERATOR_USER`,
  `OPERATOR_UID`, `OPERATOR_PASSWORD_HASH` (sha512-crypt computed on the boot host, as `CONSOLE_PASSWORD` is now)
  and the public-key field that ssh-doors adds.
- `ROOT_PASSWORD_HASH` gives root a password for su.
- The home partition is not mounted during the install, so the installer leaves a root-only
  `/.breenix-shared/var/lib/accounts/pending/operator`. The first persistent boot's reconcile creates `/home/operator` (0700)
  and `~/.ssh/authorized_keys` (0600) from it, then deletes it.

**Logging in.**
- **Console:** getty and login as `operator` with its password; root is refused.
- **SSH:** port 22, keys only, as `operator`. Root's staged `authorized_keys` from ssh-doors is removed after the first
  successful operator login over SSH (D4).
- **Root:** reached through `sudo` (operator's password) or `su -` (root's password, wheel only).
- **Recovery:** a test-root boot through metal-run runs as root on the RAM disk and can repair
  `/.breenix-shared`.

The metal-console `console-login` challenge moves from root to `operator`. The ARM64 desktop still runs as root; a
graphical login is later main-path work.

## 6. The `users` suite

`docs/suites/users.json`, `/sbin/suite-users`, running as PID 1 on the test root.
- **Fixtures:** test images only get `ua` 2001, `ub` 2002, `uw` 2003 (in wheel) and group `ug` 2100 (member
  `ua`), with a hash generated at image build. It is not committed and not staged on the persistent install.
- **Cases:** each forks a child that becomes a fixture account through `become`, performs the operation and
  reports through a pipe.
- **Targets:** ARM64 QEMU for iteration (S2); acceptance is `metal-run boot --suite users --milestone users/<milestone-id>` on the R240. The `ssh`
  category uses loopback bssh on QEMU and the boot host's OpenSSH on the R240.

| Category | Representative cases |
|---|---|
| `credentials` | `ids-after-drop`, `no-regain-root`, `saved-toggle`, `setres-rules`, `setgroups-user-eperm`, `threads-share-groups` (K2), `orphan-thread-not-root` (K1, forcing hook) |
| `files` | `other-0600-read-eacces`, `other-0600-write-eacces`, `group-0640-member`, `group-0640-nonmember`, `supplementary-access`, `root-exec-needs-x`, `search-denied`, `create-owner`, `sgid-dir-group`, `chmod-not-owner-eperm`, `chown-giveaway-eperm`, `chgrp-nonmember-eperm`, `write-clears-setid`, `utimes-explicit-nonowner-eperm` |
| `sticky` | `tmp-1777`, `unlink-other-eperm`, `rename-other-eperm`, `replace-other-eperm`, `dir-owner-may-remove`, `symlink-follow-refused`, `hardlink-other-refused` |
| `setid-exec` | `setuid-euid` (euid = owner, uid unchanged), `setuid-saved`, `setgid-egid`, `setgid-no-gx-ignored`, `nosuid-home-ignored`, `shared-fs-no-gain`, `at-secure`, `closed-stdio-opened` |
| `signals` | `kill-other-eperm`, `kill-probe-eperm`, `kill-setuid-child`, `root-kills-any`, `renice-other-eperm` |
| `proc-dev` | `proc-exe-other-eacces`, `kernel-output-other-eacces`, `kmsg-user-eacces`, `trace-user-eacces`, `vcs-user-eacces`, `tcp-low-port-eacces` |
| `umask` | `umask-file`, `umask-dir`, `umask-fork`, `umask-threads` |
| `layout` | `fhs-paths` (each table row's owner and mode), `shadow-0600`, `sudo-policy-0440`, `setuid-set` (no set-ID inode outside the design table), `run-cleared` |
| `accounts` | `useradd-entries`, `useradd-home-0700`, `useradd-duplicate-refused`, `userdel-removes`, `usermod-groups`, `lock-serializes` (two concurrent useradds keep both), `interrupted-write-intact`, `passwd-own`, `passwd-other-refused`, `id-output` |
| `su` | `setuid-set` (exactly su, passwd and sudo set-user-ID root; S11), `su-wrong-password-refused`, `su-root-ok`, `su-nonwheel-refused`, `su-dash-env`, `su-logged` |
| `sudo` | `sudo-wheel-ok`, `sudo-nonwheel-refused`, `sudo-wrong-password`, `sudo-env-reset`, `sudo-bad-policy-mode-refused`, `sudo-logged` |
| `login` | `login-user` (IDs, groups, HOME, cwd, shell), `login-root-refused`, `login-locked-refused`, `login-expired-refused`, `login-tty-owned`, `login-umask`, `logout-tty-restored` |
| `ssh` | `ssh-identity`, `ssh-root-refused`, `ssh-groups`, `ssh-pty-owner`, `ssh-bad-key-mode-refused` |

The failure delays are fixed waits, not timing assertions, so the suite is not `timingSensitive`.

## 7. The `users` effort

In `docs/efforts/path.json` after `server`, with targets ARM64 QEMU, Parallels, VMware and x86-metal (D6). Lanes
iterate on ARM64 QEMU and are accepted on the R240 first; the catch-up then brings the VMs to the same bar. Each R240 item
below is one metal-run sequence at the lane's final head, selecting that row's milestone with `--milestone users/<milestone-id>`. The `users` suite has no cases yet: each category's cases
arrive with the lane that implements it.

| # | Milestone | Measure | On the R240 |
|---|---|---|---|
| 1 | `kernel-checks`: K1–K12 and evidence for #1089 | suite `users`: `credentials`, `files`, `sticky`, `setid-exec`, `signals`, `proc-dev`, `umask` | A users suite boot on the test root. |
| 2 | `layout`: the FHS table, shared state, `etc-reconcile` | suite `users`: `layout` | A v2 install, a rollback to the v1 slot and a return. The host-key fingerprint and the `/root` and `/home` sentinels are unchanged, and a local `/etc` edit survives with its `.factory` sibling. |
| 3 | `accounts`: the library, the write protocol, the tools | suite `users`: `accounts` | A suite boot. Then `useradd` in a persistent session, an upgrade, and the account still present. |
| 4 | `login`: `become`, any-account login, bsshd on the library, tty owners, `ROOT_LOGIN` | suite `users`: `login`, `ssh` | Console login as a fixture user on the test root. |
| 5 | `admin`: su, sudo, policy, auth.log | suite `users`: `su`, `sudo` | A suite boot, with the auth.log lines in the serial capture. |
| 6 | `operator-login`: operator from the site, root login off | suite `users`: `operator` (host-driven, from the boot host's OpenSSH and the console challenge) | `install-persistent`, then OpenSSH from the boot host with strict host-key checking. `id` shows `uid=1000(operator) gid=1000(operator) groups=1000(operator),10(wheel)`; `sudo -n true` is refused; `sudo id` with the password shows uid 0; SSH as root is refused; the console challenge passes as operator. |

Milestone 6 is measured by a suite category because main-path efforts resolve `bootPath` measures against
`docs/boot-path.json`, the ARM testing-kernel gate, which has no persistent modes; the persistent boot stages stay
in `docs/metal/boot-path.json` for the metal stream.

**Ordering.**
- **server:**
  - `ssh-keys` (the ssh-doors lane) goes first.
  - This effort replaces the server effort's own `users` milestone.
  - Milestones 1 and 2 can start now, alongside ssh-doors, since they touch the kernel, the image builders and the
    installer rather than bsshd.
  - Milestone 2 precedes `services` and `logs`, which need shared `/var` and `/run`.
  - Milestone 6 precedes `resting-state` and the 24-hour session (D4: daily non-root login, root SSH off).
- **toolchain:** `deliver` uploads into `/home/operator` as operator, after milestone 4. `sdk-on-box` installs into
  `/usr/include` and `/usr/lib`, after milestone 2.
- **filesystem:**
  - `fs-interface` follows milestone 1's K12, so the trait carries `InodeAttrs`.
  - The CoW migration carries `/.breenix-shared`, and its manifest already compares owners and modes.
  - The `users` suite joins files-io and directories on the new root.

Roughly 10,000–14,500 lines: kernel 2,500–4,000, accounts and tools 3,000–4,500, layout and installer 1,500–2,500,
suite 2,500–3,500. Milestones 1 and 3 touch disjoint code and can run as parallel lanes; 4 needs 3, and 5 needs 4.
