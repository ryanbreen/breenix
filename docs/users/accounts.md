# Account databases and administration

The accounts milestone implements section 3 of [the users design](design.md).
`libbreenix::accounts::parse` contains allocation-free borrowed passwd, group
and shadow parsers, shared with libc's getpw* and getgr* functions. Invalid
lines do not invalidate their neighbours. Names follow
`[a-z_][a-z0-9_-]{0,31}`, IDs are unsigned 32-bit values except the reserved
all-ones value, and homes and shells are absolute paths without dot components.
Authentication must refuse an empty password, `!` locks password authentication,
`*` has no password, and account expiration is separate from password locking.
The login milestone connects the login and SSH consumers to these policies.

`libbreenix::accounts` supplies the database and update operations to the Rust
administration tools. On a v2 persistent install `/etc` is already the kernel's
binding of `/.breenix-shared/etc`; the library writes that live database, with
no duplicate store and no generation pointer of its own. Generation factory
files remain defaults. Factory reconciliation adds missing required entries
through this library and retains existing lines.

Each update holds an exclusive POSIX record lock on `/etc/.pwd.lock` (0600,
root:root) before reading the databases, and holds it through publication.
A process-local mutex also serializes library writers in one process, because
POSIX record locks belong to the process. libc's lckpwdf and ulckpwdf lock the
same inode, with a bounded 15-second wait for lckpwdf. Closing the descriptor
or exiting releases the lock; there is no stale lock-file removal protocol.

Changed databases are published in shadow, group, passwd order. For each,
the writer creates `<file>+` with `create_new(true)`, sets the final owner and mode, writes
the buffer with `write_all`, fsyncs the file, renames it over the database and fsyncs `/etc`.
An abandoned scratch file is unlinked under the lock before the next attempt;
the next attempt does not open it for writing. Databases and the lock must have their
specified owner and mode and be regular files with one link. Each file is old
or new after an interrupted operation; the three-file set is not one atomic
transaction. In particular a new passwd entry cannot precede its shadow entry.
This protocol addresses operation boundaries; torn writes on ext2 remain outside its model.

The installed tools are `/usr/sbin/useradd`, `userdel`, `usermod`, `groupadd`
and `groupdel`, with mode 0755 and an effective-root requirement. Without `-g`, useradd
creates a private group. The default UID is the first unused ID at or above
1000, the home `/home/<name>` and the shell `/bin/bsh`. `-m` creates the home
0700 and copies regular files and directories from `/etc/skel`, preserving
ordinary permission bits and setting ownership to the account. Skeleton links
and special files are refused. Automatic home operations require trusted,
root-owned parents; userdel -r additionally requires the account to own the
home. userdel refuses system accounts and an account with running processes.
Group deletion refuses a group used as a primary group. BusyBox's groups,
id and whoami use the shared libc parsers.

The passwd library operation reads the real caller ID from the kernel, requires
effective root, checks the old password for a non-root caller changing their
own password, and refuses changes to another account. New passwords contain
8 to 1024 bytes and use SHA-512-crypt with a random 16-character salt and
100000 rounds. The interactive set-ID passwd command arrives with the admin
milestone; this milestone adds no executable stub or set-ID program.
Account tools and password changes append sanitized records to the 0640
authentication log, fsync it, and send the record to the root-only `/dev/kmsg`
audit sink. Records contain action and result fields, omitting passwords and hashes.

The users suite's accounts category exercises installed tool results, home
metadata and skeleton bytes, refusal without database changes, deletion with
live processes and with a foreign-owned home, supplementary groups, password
locks and expiry, group operations, two concurrent useradds, libc lock
interoperability, abrupt process exits at publication boundaries and recovery,
passwd library authorization and fresh hashes, libc identity output and audit records.
The password cases call the library in-process; malformed-line isolation parses
strings in memory. The blocked-process observation in `lock-serializes` does not
distinguish a record-lock wait from another kernel wait.
The suite uses disposable roots. Persistent acceptance separately creates an
account in a running install and verifies it after an install-persistent upgrade.

R240 acceptance at runtime commit `b5be44d5` is retained in boot
`20261021T154156Z-metal`: accounts 18/18, layout 5/5, kernel checks 54/54,
and reclamation within the unchanged 48-page allowance. The later admin
`su/setuid-set` case failed because its executable belongs to that milestone;
this record is accounts acceptance rather than a complete users-suite result.
Private boot records `20261021T160751Z-metal` and `20261021T161800Z-metal`
contain the account-created and preservation markers. The intervening install
reinstalled the same commit. The version change was the restoration to main
`2af2ebe7`, followed by `20261021T162738Z-metal`, which retained the account
and operator/key comparisons. Raw records remain in the private boot store.
