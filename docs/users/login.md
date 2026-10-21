# Logins and administration

The login and admin milestones implement section 3 of [the users design](design.md) from "Identity, PAM-free"
onward: any account logs in on the console and over SSH through the accounts library, terminals change owner for
a session, `ROOT_LOGIN` governs root at login, and su, sudo and passwd are the set-user-ID administration path,
with each attempt recorded in the authentication log.

## One session path

`libbreenix::accounts::session` holds the steps login, su, sudo and bsshd share:

1. `validate`: a strict passwd entry (the accounts library's parser), a shadow line, no expired account
   (`expire` before today), and a login shell listed in `/etc/shells`. A password method also needs a
   SHA-512-crypt hash that has not aged out; `!` locks only the password, `*` and an empty hash do not match.
2. `grant_terminal`: the session's terminal becomes the account's, group tty (5), mode 0620. The system console
   (`/dev/console`) stays root's 0600.
3. `take_identity`: supplementary groups, then setresgid, then setresuid, each read back. For a target other
   than root, `setuid(0)` must then fail.
4. The umask from `/etc/login.defs` (`UMASK`, default 022).
5. `exec_shell`: a clean environment (HOME, USER, LOGNAME, SHELL, PATH, a printable TERM), the home (or `/`
   with a message when it is unavailable), and the shell as a login shell (`argv[0]` is `-` and its name).

`/etc/login.defs` is read strictly: a malformed value keeps the secure default. `ROOT_LOGIN yes` is the only
value that lets login accept uid 0; the shipped file says `ROOT_LOGIN no`. bsshd refuses uid 0 whatever it says
(D4). `FAIL_DELAY` (default 2 seconds) is a fixed wait after each refused authentication, whatever the reason.

## Programs

| Program | Behaviour |
|---|---|
| `getty [ttyN \| pts/N]` | Serves the console, a virtual terminal or a pseudo-terminal line. A line that the previous session left owned by an account is returned to root (0620, group tty) before the prompt, and the logout is recorded. |
| `/bin/login NAME` | Reads the password with echo off, validates the account, refuses root under `ROOT_LOGIN no`, prints `Login incorrect` after the failure delay for each refusal, and records the attempt with its reason. A session then follows the steps above. The kernel's console input-provenance report runs on the console and the virtual terminals only. |
| `/bin/su [-] [-c command] [name]` | 4755 root. The target is root unless named. A caller other than root types the target's password on `/dev/tty`; becoming uid 0 also needs the caller's account in wheel, checked before any password is asked. `-` gives the target's login environment, umask and home; without it the caller's environment and directory stay, with HOME, SHELL, USER and LOGNAME set to the target's and PATH reset for root. |
| `/usr/bin/sudo [-u user] [-i] [-n] [command]` | 4755 root. The policy `/etc/sudo.policy` must be a regular root-owned file at 0440 with one link and must parse. The command resolves to an absolute path (through `/usr/sbin:/usr/bin:/sbin:/bin` when it has no `/`) before the policy check. The caller types their own password unless the rule says `nopass`; a success is remembered for 5 minutes for that session and terminal in `/run/sudo/<uid>/<session>`, and `-n` does not ask. The command gets PATH as above, the target's HOME, USER and LOGNAME, TERM kept, SUDO_USER and SUDO_COMMAND, umask 022 and the target's groups and IDs; `-i` runs the target's login shell in its home. |
| `/usr/bin/passwd [name]` | 4755 root. Your own password after the current one, typed twice; root may set any account's. The accounts library checks the caller from the kernel's real user ID and writes a fresh 100000-round hash. |
| `bsshd` | Resolves the account through `session::validate`, refuses uid 0, reads `~/.ssh/authorized_keys` only when the file, `~/.ssh`, the home and each directory above are owned by root or the account and not group- or world-writable, gives an interactive session's PTY to the account (0620, group tty) and returns it to root when the session ends, and becomes the account through `session::become_account`. |

The policy grammar, `libbreenix::accounts::policy`, is Breenix's own:
`permit (USER | %GROUP) [as TARGET] [cmd /ABSOLUTE/PATH] [nopass]`. A rule without `as` permits root only, and one
without `cmd` any command; the first rule matching the caller, target and command decides. The shipped policy is
`permit %wheel as root`. Root running sudo is not checked against the policy.

## The authentication log

Each login, logout, su, sudo, bsshd session and account or password change appends one line to
`/var/log/auth.log` (0640 root:adm, append, fsync) and writes it to `/dev/kmsg`, which puts it in the kernel log
and the serial capture:

```text
2026-10-21T19:04:11Z sudo[214] user=uw target=root tty=pts/0 result=ok cmd=/usr/bin/id -u
2026-10-21T19:04:15Z login[230] user=ua target=ua tty=tty2 result=refused:password cmd=
```

Each field keeps printable ASCII without `=`, so a record stays one line. No password or hash is written.

## Terminals

The kernel keeps an owner, group and mode for each virtual terminal (`kernel/src/fs/devfs/mod.rs`), root's and
the tty group's at 0620 until a login gives one away; fchown and fchmod on a terminal descriptor follow the rules
for any file, and open, stat and access checks read them. `/dev/tty0` reports the foreground terminal's.
Pseudo-terminal slaves already carried the same metadata.

## Test accounts

Test images carry the users suite's fixture accounts `ua` (2001, in `ug` 2100), `ub` (2002) and `uw` (2003, in
wheel), with locked passwords and homes under `/etc/breenix/test-users`. `persistent-install` leaves their lines,
their group memberships and their homes out of each generation and the machine store, as it does the SSH oracle's
`sshtest`.

On the R240's test roots, console and default boots stage the private console password for root (su's target) and
for `uw`, keeping the image's other shadow lines. The automated console challenge logs in as `uw`, is refused once
with a wrong password, computes at the bsh prompt, and reads the root-only `/proc/fbcon/check` through
`sudo`. Images from before this milestone carry no `uw`, and the challenge logs in as root on them. An operator
console session on a test root logs in as `uw` with the console password and uses sudo or `su -`.

## The users suite's categories

`login` (8 cases), `ssh` (5), `su` (7) and `sudo` (8) measure the login and admin milestones. Each case makes its
accounts through the accounts library (useradd -m and a random password set as root) with
`/usr/libexec/breenix/users-login-probe_test` as the login shell; the probe prints one line describing the session
it was given (IDs, groups, umask, working directory, `argv[0]`, the terminal's owner and mode, and the environment).
Logins run `getty pts/N` and login on a fresh pseudo-terminal line the suite holds; su, sudo and passwd are typed
into a terminal as the caller's account; the SSH cases run bsshd on a loopback port and the bssh client with the
image's test key. Each refusal case also shows the same step succeeding once its one condition is lifted. The SSH
category uses loopback bssh on each target; host-driven OpenSSH logins belong to the `operator` category.
