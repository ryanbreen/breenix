# Account databases and administration

The accounts milestone implements section 3 of [the users design](design.md).
`libbreenix::accounts::parse` contains allocation-free borrowed passwd, group
and shadow parsers, shared with libc's getpw* and getgr* functions. Invalid
lines do not invalidate their neighbours. Names follow
`[a-z_][a-z0-9_-]{0,31}`, IDs are unsigned 32-bit values except the reserved
all-ones value, and homes and shells are absolute paths without dot components.
An empty password never authenticates, `!` locks password authentication,
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
the writer creates `<file>+` exclusively, sets the final owner and mode, writes
all bytes, fsyncs the file, renames it over the database and fsyncs `/etc`.
An abandoned scratch file is unlinked under the lock before the next attempt;
it is never followed or truncated. Databases and the lock must have their
specified owner and mode and be regular files with one link. Each file is old
or new after an interrupted operation; the three-file set is not one atomic
transaction. In particular a new passwd entry cannot precede its shadow entry.
This is an operation-boundary protocol, not a power-loss guarantee on ext2.

The installed tools are `/usr/sbin/useradd`, `userdel`, `usermod`, `groupadd`
and `groupdel`, all 0755 and root-only in operation. Without `-g`, useradd
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
audit sink. Passwords and hashes never enter those records.

The users suite's accounts category exercises installed tool results, home
metadata and skeleton bytes, refusal without database changes, deletion with
live processes and with a foreign-owned home, supplementary groups, password
locks and expiry, group operations, two concurrent useradds, libc lock
interoperability, abrupt process exits at publication boundaries and recovery,
passwd authorization and fresh hashes, libc identity output and audit records.
The suite uses disposable roots. Persistent acceptance separately creates an
account in a running install and verifies it after an install-persistent upgrade.
