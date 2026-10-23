//! Rust std: std::fs and std::io, std::process, std::thread and std::sync, std::net,
//! std::time and std::env, measured against Rust's own API documentation and what those
//! APIs do on Linux. Each case calls std's public API only (std::os::unix's extensions
//! included) and asserts one documented behaviour: a value, or the io::ErrorKind or errno
//! the call fails with. std for Breenix is the forked library's `breenix` OS module on
//! Breenix's own C library (libs/libbreenix-libc), so a case measures the whole path:
//! std, the C library and the kernel.
//!
//! Some std APIs link C-library functions the library may not define yet: Command's spawn
//! path links execvp, setuid, setgid and chroot, fs::canonicalize links realpath and
//! fs::remove_dir_all links fdopendir and unlinkat. build.rs reads libc.a's symbol index
//! and sets `libc_has = "<name>"` for each one present. A case that needs a function the
//! library lacks is compiled out and fails saying which, so the suite still links and the
//! case starts measuring once the library has it.
//!
//! Programs the process cases start are `/usr/libexec/breenix/rust-std-child_test`, a
//! std-only helper that reports what it saw on its stdout or in its exit status.
//!
//! Each case runs in its own forked child under the runner's default 10-second limit.
//! Waits for another process or thread are bounded at 3 seconds and fail saying what was
//! still running; sockets that wait for data have a 2-second read timeout. Sleeps and
//! timeouts may never end early and may end at most 30 ms late: two 5 ms ticks of x86-64's
//! timer and 20 ms, the time suite's bound on the coarser of the two architectures.
//! Files go in a directory of the case's own, /tmp/rust-std-<pid>, removed when the case
//! ends; Unix socket paths sit beside it. TCP and UDP use 127.0.0.1 and ports 20000-29999,
//! numbered from the case's process ID.
use breenix_suite::suite::{case, category, check, fail, suite, value, wait_for, CaseError, CaseResult, Suite};
use std::cell::Cell;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, ErrorKind, Read, Seek, SeekFrom, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixDatagram, UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, TryRecvError, TrySendError};
use std::sync::{Arc, Barrier, Condvar, Mutex, Once, OnceLock, RwLock, TryLockError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static SUITE: Suite = suite("rust-std", "Rust std", &[
    category("fs-io", "std::fs & std::io", &[
        case("create-read-back", "File::create and write_all, then fs::read returns exactly the bytes written", create_read_back),
        case("create-truncates", "File::create on an existing file truncates it to length 0", create_truncates),
        case("open-missing", "File::open of a missing path fails with ErrorKind::NotFound", open_missing),
        case("create-new-exists", "OpenOptions::create_new on an existing path fails with ErrorKind::AlreadyExists", create_new_exists),
        case("append", "OpenOptions::append writes at the end of the file even after a seek to the start", append),
        case("write-read-only", "Writing through a File opened with File::open fails with EBADF", write_read_only),
        case("seek-from-end", "Seek to SeekFrom::End(-3) returns the new position and reads the last three bytes", seek_from_end),
        case("seek-before-start", "Seeking to before the start of a file fails with ErrorKind::InvalidInput", seek_before_start),
        case("seek-past-end", "Writing after a seek past the end leaves a gap that reads back as zeros", seek_past_end),
        case("set-len", "File::set_len extends a file with zeros and truncates it", set_len),
        case("try-clone-offset", "File::try_clone shares the file offset with the original", try_clone_offset),
        case("sync", "File::sync_all and File::sync_data succeed on a written file", sync),
        case("metadata", "metadata reports a file's length, a regular file type and nlink 1, and a directory's type", metadata),
        case("metadata-missing", "metadata of a missing path fails with ErrorKind::NotFound", metadata_missing),
        case("permissions-mode", "set_permissions with mode 0o640 is the mode PermissionsExt::mode reports", permissions_mode),
        case("permissions-readonly", "Permissions::set_readonly(true) clears every write bit and readonly() reports it", permissions_readonly),
        case("modified-now", "A file just written has a modified time within 5 seconds of SystemTime::now", modified_now),
        case("set-modified", "File::set_modified sets the modified time metadata reports", set_modified),
        case("read-dir", "read_dir lists exactly the entries created, without . and .., with their file types", read_dir),
        case("read-dir-missing", "read_dir of a missing path fails with ErrorKind::NotFound", read_dir_missing),
        case("create-dir-all", "create_dir_all creates every missing parent and succeeds when the path exists", create_dir_all),
        case("remove-dir-not-empty", "remove_dir of a directory that has an entry fails with ErrorKind::DirectoryNotEmpty", remove_dir_not_empty),
        case("remove-dir-all", "remove_dir_all removes a tree of files, directories and symlinks, and does not follow a symlink out of it", remove_dir_all),
        case("rename", "rename moves a file, and the old path then fails with ErrorKind::NotFound", rename),
        case("rename-replace", "rename onto an existing file replaces it", rename_replace),
        case("hard-link", "hard_link names the same inode: one ino, nlink 2, and writes through one name read through the other", hard_link),
        case("symlink", "symlink and read_link round-trip the target; symlink_metadata sees the link and metadata the target", symlink_case),
        case("symlink-dangling", "metadata of a dangling symlink fails with ErrorKind::NotFound while symlink_metadata succeeds", symlink_dangling),
        case("canonicalize", "canonicalize resolves symlinks, . and .. to an absolute path", canonicalize),
        case("buf-reader-lines", "BufReader::lines returns each line of a file without its line terminator", buf_reader_lines),
        case("buf-writer", "BufWriter keeps small writes until flush, and then the file holds them", buf_writer),
        case("io-copy", "io::copy between two files returns the byte count and copies every byte", io_copy),
        case("fs-copy", "fs::copy copies a file's contents and its permission bits", fs_copy),
        case("create-on-directory", "File::create on a directory fails with ErrorKind::IsADirectory", create_on_directory),
        case("std-streams", "io::stdin, stdout and stderr are descriptors 0, 1 and 2, and stdout and stderr accept writes", std_streams),
        case("stdin-pipe", "A program's io::stdin reads to end of file what its parent wrote to a Stdio::piped stdin", stdin_pipe),
    ]),
    category("process", "std::process", &[
        case("status-success", "Command::status of a program that exits 0 is success with code Some(0)", status_success),
        case("exit-code", "A program that exits 42 has ExitStatus::code Some(42) and is not success", exit_code),
        case("exit-low-bits", "process::exit(263) reaches the parent as code 7, its low 8 bits", exit_low_bits),
        case("into-raw-exit", "ExitStatusExt::into_raw of an exit with code 42 is the wait status 0x2a00", into_raw_exit),
        case("args", "Command::args reach the program in order, including an empty one and one with a space", args),
        case("args-non-utf8", "An argument that is not UTF-8 reaches the program byte for byte through args_os", args_non_utf8),
        case("arg0", "CommandExt::arg0 sets the argv[0] the program sees", arg0),
        case("env-set", "Command::env sets a variable the program sees", env_set),
        case("env-inherit", "A variable set with env::set_var in the parent is inherited by the program", env_inherit),
        case("env-remove", "Command::env_remove hides an inherited variable from the program", env_remove),
        case("env-clear", "Command::env_clear leaves the program only the variables the Command sets", env_clear),
        case("current-dir", "Command::current_dir runs the program in that directory", current_dir),
        case("current-dir-missing", "spawn with a missing current_dir fails with ErrorKind::NotFound", current_dir_missing),
        case("spawn-not-found", "spawn of a missing absolute path fails with ErrorKind::NotFound", spawn_not_found),
        case("spawn-path-missing", "spawn of a bare name in no PATH directory fails with ErrorKind::NotFound", spawn_path_missing),
        case("spawn-path-found", "spawn of a bare name finds the program in the PATH the Command sets", spawn_path_found),
        case("spawn-not-executable", "spawn of a file without execute permission fails with ErrorKind::PermissionDenied", spawn_not_executable),
        case("spawn-directory", "spawn of a directory fails with ErrorKind::PermissionDenied", spawn_directory),
        case("output", "Command::output captures stdout and stderr separately along with the exit status", output),
        case("stdout-piped", "A Stdio::piped stdout read from Child::stdout to end of file holds what the program wrote", stdout_piped),
        case("stdin-piped", "Bytes written to Child::stdin come back through the program's stdout", stdin_piped),
        case("stdin-large", "256 KiB written to a piped stdin while another thread reads stdout arrive intact", stdin_large),
        case("stdio-null", "Stdio::null gives the program an empty stdin and discards its stdout", stdio_null),
        case("stdout-to-file", "Stdio::from(File) sends the program's stdout into that file", stdout_to_file),
        case("wait", "Child::wait returns the program's exit status, and a second wait returns it again", wait),
        case("try-wait", "Child::try_wait returns None while the program runs and its status once it exits", try_wait),
        case("kill", "Child::kill ends a running program, and ExitStatusExt::signal reports SIGKILL", kill),
        case("kill-exited", "Child::kill of a program that exited but was not waited for succeeds, and wait reports its exit", kill_exited),
        case("abort-signal", "A program that calls process::abort is reported killed by SIGABRT", abort_signal),
        case("signal-into-raw", "ExitStatusExt::into_raw of a SIGKILL death has 9 in its low 7 bits", signal_into_raw),
        case("child-id", "Child::id is the process::id the program reports, and its parent_id is the caller's id", child_id),
        case("process-id", "process::id is nonzero, the same on every call, and the Pid in /proc/<id>/status", process_id),
        case("concurrent-children", "Eight programs run at once and each one's exit code reaches its own Child", concurrent_children),
    ]),
    category("thread-sync", "std::thread & std::sync", &[
        case("spawn-join", "JoinHandle::join returns the value the thread's closure returned", spawn_join),
        case("spawn-many", "Sixteen threads each return their own result through join", spawn_many),
        case("panic-aborts", "Under panic=abort a panic in a spawned thread ends the process with SIGABRT", panic_aborts),
        case("panic-hook", "A hook set with panic::set_hook runs before a panicking process aborts", panic_hook),
        case("builder-name", "thread::current().name() in a thread is the name given to Builder::name", builder_name),
        case("main-name", "thread::current().name() on the main thread is Some(\"main\")", main_name),
        case("builder-stack-size", "A thread built with Builder::stack_size of 4 MiB can use 3 MiB of stack", builder_stack_size),
        case("thread-ids", "ThreadId is the same on every call in a thread, differs between threads, and matches Thread::id", thread_ids),
        case("is-finished", "JoinHandle::is_finished is false while the thread blocks and true once it returns", is_finished),
        case("scoped-borrow", "Threads in thread::scope borrow and write a local array, and all have finished when it returns", scoped_borrow),
        case("scoped-join", "ScopedJoinHandle::join returns the scoped thread's result", scoped_join),
        case("mutex-counter", "Four threads incrementing a Mutex-guarded counter 10000 times each leave 40000", mutex_counter),
        case("mutex-try-lock", "Mutex::try_lock while another thread holds the lock fails with TryLockError::WouldBlock", mutex_try_lock),
        case("mutex-blocked-waiter", "A thread blocked in Mutex::lock acquires it once the holder releases it", mutex_blocked_waiter),
        case("rwlock-readers", "RwLock::read in another thread succeeds while a reader holds the lock", rwlock_readers),
        case("rwlock-writer-excludes", "RwLock try_read fails while a writer holds the lock, and try_write while a reader does", rwlock_writer_excludes),
        case("condvar-notify-one", "Condvar::notify_one wakes a thread in wait_while once its condition holds", condvar_notify_one),
        case("condvar-notify-all", "Condvar::notify_all wakes all four waiting threads", condvar_notify_all),
        case("condvar-timeout", "Condvar::wait_timeout_while reports timed_out after 100 ms, no earlier and at most 30 ms late", condvar_timeout),
        case("atomic-fetch-add", "Four threads doing 100000 AtomicU64::fetch_add each leave 400000", atomic_fetch_add),
        case("atomic-cas-lock", "A lock built on AtomicBool::compare_exchange keeps four threads' increments exclusive", atomic_cas_lock),
        case("mpsc-order", "An mpsc channel delivers 1000 messages from one sender in order", mpsc_order),
        case("mpsc-disconnect", "mpsc delivers every message from four senders, then recv fails once all are dropped", mpsc_disconnect),
        case("mpsc-recv-timeout", "Receiver::recv_timeout on an empty channel fails with Timeout after 50 ms, at most 30 ms late", mpsc_recv_timeout),
        case("mpsc-try-recv", "Receiver::try_recv fails with Empty while a sender lives and Disconnected after", mpsc_try_recv),
        case("sync-channel-full", "SyncSender::try_send on a full sync_channel(1) fails with Full", sync_channel_full),
        case("sync-channel-rendezvous", "send on sync_channel(0) blocks until the receiver takes the message", sync_channel_rendezvous),
        case("barrier", "Barrier::wait releases four threads together and names exactly one leader", barrier),
        case("once", "Once::call_once runs its closure exactly once across eight threads", once),
        case("once-lock", "OnceLock::get_or_init initializes once across eight threads, and set then fails", once_lock),
        case("thread-local", "A thread_local! value starts fresh in each thread and is separate from the main thread's", thread_local),
        case("thread-local-dtor", "A thread_local! value's destructor runs when its thread exits, before join returns", thread_local_dtor),
        case("park-token", "unpark before park makes the next park return at once", park_token),
        case("park-unpark", "Thread::unpark wakes a thread blocked in thread::park", park_unpark),
        case("park-timeout", "park_timeout returns by its 50 ms timeout plus 30 ms when nothing unparks the thread", park_timeout),
        case("available-parallelism", "available_parallelism is the number of processors /proc/cpuinfo lists", available_parallelism),
        case("available-parallelism-affinity", "available_parallelism follows the thread's affinity when sched_setaffinity narrows and restores it", available_parallelism_affinity),
        case("detached", "A thread whose JoinHandle was dropped keeps running and sends its result", detached),
    ]),
    category("net", "std::net", &[
        case("tcp-bind", "TcpListener::bind on 127.0.0.1 and a free port reports that address from local_addr", tcp_bind),
        case("tcp-bind-port-zero", "TcpListener::bind to port 0 gets a nonzero port local_addr reports", tcp_bind_port_zero),
        case("tcp-bind-in-use", "A second TcpListener::bind to a listening address fails with ErrorKind::AddrInUse", tcp_bind_in_use),
        case("tcp-accept-addrs", "accept's peer address is the client's local_addr, and the client's peer_addr is the listener", tcp_accept_addrs),
        case("tcp-echo", "Bytes written to a TcpStream arrive at the peer and come back", tcp_echo),
        case("tcp-large", "1 MiB written by one end of a TCP connection reaches the other intact", tcp_large),
        case("tcp-refused", "TcpStream::connect to a port with no listener fails with ErrorKind::ConnectionRefused", tcp_refused),
        case("tcp-read-eof", "read returns 0 once the peer has closed the connection", tcp_read_eof),
        case("tcp-shutdown-write", "shutdown(Write) gives the peer end of file while the stream still reads", tcp_shutdown_write),
        case("tcp-shutdown-read", "read after shutdown(Read) returns 0", tcp_shutdown_read),
        case("tcp-read-timeout", "read with a 100 ms read timeout fails with ErrorKind::WouldBlock after it, at most 30 ms late", tcp_read_timeout),
        case("tcp-timeout-values", "read_timeout and write_timeout return what set_read_timeout and set_write_timeout set", tcp_timeout_values),
        case("tcp-nonblocking-accept", "accept on a nonblocking listener with no client fails with ErrorKind::WouldBlock", tcp_nonblocking_accept),
        case("tcp-nonblocking-read", "read on a nonblocking stream with no data fails with ErrorKind::WouldBlock", tcp_nonblocking_read),
        case("tcp-connect-timeout", "TcpStream::connect_timeout connects to a listening address", tcp_connect_timeout),
        case("tcp-nodelay", "nodelay returns what set_nodelay set", tcp_nodelay),
        case("tcp-ttl", "ttl returns what set_ttl set", tcp_ttl),
        case("tcp-peek", "peek returns waiting data without consuming it", tcp_peek),
        case("tcp-write-closed", "Writing to a connection whose peer has gone fails with BrokenPipe or ConnectionReset", tcp_write_closed),
        case("udp-send-recv", "UdpSocket::send_to arrives at recv_from with the sender's address", udp_send_recv),
        case("udp-connected", "Connected UdpSockets exchange datagrams with send and recv, and peer_addr names the peer", udp_connected),
        case("udp-connected-filter", "A connected UdpSocket receives only from its peer", udp_connected_filter),
        case("udp-not-connected", "UdpSocket::peer_addr before connect fails with ErrorKind::NotConnected", udp_not_connected),
        case("udp-truncate", "recv into a buffer smaller than a datagram fills it and discards the rest", udp_truncate),
        case("udp-read-timeout", "recv_from with a 100 ms read timeout fails with ErrorKind::WouldBlock after it, at most 30 ms late", udp_read_timeout),
        case("udp-nonblocking", "recv on a nonblocking UdpSocket with nothing queued fails with ErrorKind::WouldBlock", udp_nonblocking),
        case("resolve-localhost", "(\"localhost\", 80).to_socket_addrs() includes 127.0.0.1:80", resolve_localhost),
        case("tcp-bind-localhost", "TcpListener::bind to localhost binds a loopback address", tcp_bind_localhost),
        case("unix-pair", "UnixStream::pair carries bytes both ways", unix_pair),
        case("unix-listener", "UnixListener::bind, UnixStream::connect and accept carry bytes, and local_addr names the path", unix_listener),
        case("unix-bind-in-use", "UnixListener::bind to a path that exists fails with ErrorKind::AddrInUse", unix_bind_in_use),
        case("unix-connect-missing", "UnixStream::connect to a missing path fails with ErrorKind::NotFound", unix_connect_missing),
        case("unix-datagram", "UnixDatagram::send_to arrives at recv_from with the sender's path", unix_datagram),
        case("unix-datagram-pair", "UnixDatagram::pair keeps message boundaries", unix_datagram_pair),
        case("unix-shutdown", "UnixStream::shutdown(Write) gives the peer end of file", unix_shutdown),
    ]),
    category("time-env", "std::time & std::env", &[
        case("instant-monotonic", "Successive Instant::now readings never go backwards", instant_monotonic),
        case("instant-elapsed", "Instant::elapsed after a 50 ms sleep is at least 50 ms", instant_elapsed),
        case("instant-threads", "An Instant read after another thread's is never earlier", instant_threads),
        case("system-time-epoch", "SystemTime::now is after UNIX_EPOCH and after 2024-01-01", system_time_epoch),
        case("system-time-rate", "SystemTime advances as Instant does across a 200 ms sleep, within 20 ms", system_time_rate),
        case("system-time-earlier", "duration_since a later SystemTime fails with an error holding the difference", system_time_earlier),
        case("sleep-never-early", "Ten 7 ms thread::sleep calls each last at least 7 ms", sleep_never_early),
        case("sleep-late-bound", "A 100 ms thread::sleep ends at most 30 ms late", sleep_late_bound),
        case("sleep-zero", "thread::sleep(Duration::ZERO) returns within 10 ms", sleep_zero),
        case("sleep-short", "Twenty 1 ms sleeps end with a median lateness of at most 6 ms", sleep_short),
        case("args-self", "env::args begins with argv[0], a path naming this program, suite-rust-std", args_self),
        case("args-child", "env::args in a spawned program is its path followed by its arguments", args_child),
        case("var-set-get", "env::var returns the value env::set_var set", var_set_get),
        case("var-missing", "env::var of an unset variable fails with VarError::NotPresent", var_missing),
        case("var-not-unicode", "env::var of a non-UTF-8 value fails with NotUnicode while var_os returns its bytes", var_not_unicode),
        case("var-empty", "A variable set to the empty string is present and empty", var_empty),
        case("remove-var", "env::remove_var unsets a variable, and vars no longer lists it", remove_var),
        case("vars-lists", "env::vars lists a variable set with set_var exactly once", vars_lists),
        case("set-var-overwrite", "A second env::set_var replaces the value", set_var_overwrite),
        case("current-dir-absolute", "env::current_dir is an absolute path to a directory", current_dir_absolute),
        case("set-current-dir", "env::set_current_dir changes the directory env::current_dir reports", set_current_dir),
        case("set-current-dir-dotdot", "env::set_current_dir(\"..\") moves to the parent directory", set_current_dir_dotdot),
        case("set-current-dir-missing", "env::set_current_dir to a missing path fails with ErrorKind::NotFound", set_current_dir_missing),
        case("set-current-dir-file", "env::set_current_dir to a file fails with ErrorKind::NotADirectory", set_current_dir_file),
        case("relative-path", "A relative path names a file in the current directory", relative_path),
        case("temp-dir-default", "env::temp_dir is /tmp when TMPDIR is unset", temp_dir_default),
        case("temp-dir-tmpdir", "env::temp_dir is TMPDIR when it is set", temp_dir_tmpdir),
        case("home-dir-env", "env::home_dir is HOME when it is set", home_dir_env),
        case("home-dir-passwd", "env::home_dir with HOME unset is the user's home directory in /etc/passwd", home_dir_passwd),
        case("current-exe", "env::current_exe is /sbin/suite-rust-std", current_exe),
        case("current-exe-child", "env::current_exe in a spawned program is that program's path", current_exe_child),
        case("current-exe-non-utf8", "A program at a non-UTF-8 path opens and stats /proc/self/exe, and current_exe is its bytes", current_exe_non_utf8),
    ]),
]);

fn main() { SUITE.run() }

// --- Shared helpers -------------------------------------------------------------------

/// How long a case waits for another process or thread.
const WAIT_MS: u64 = 3000;
/// How late a sleep or timeout may end.
const LATE_MS: u128 = 30;

/// Run `$body` when Breenix's C library defines every function named, which std links
/// for the API the case measures; otherwise fail saying which it lacks.
macro_rules! linked {
    ([$($f:literal),+], $api:literal, $body:block) => {{
        #[cfg(all($(libc_has = $f),+))]
        let result: CaseResult = (|| -> CaseResult { $body })();
        #[cfg(not(all($(libc_has = $f),+)))]
        let result: CaseResult = fail(format!("the C library has no {}, which {} links",
            [$(($f, cfg!(libc_has = $f))),+].iter().filter(|f| !f.1).map(|f| f.0).collect::<Vec<_>>().join(", "),
            $api));
        result
    }};
}

/// A case that starts a program with std::process::Command.
macro_rules! spawning {
    ($body:block) => { linked!(["execvp", "setuid", "setgid", "chroot"], "std::process::Command's spawn", $body) };
}

/// Items only spawning cases use.
macro_rules! spawn_items {
    ($($item:item)*) => {
        $(#[cfg(all(libc_has = "execvp", libc_has = "setuid", libc_has = "setgid", libc_has = "chroot"))] $item)*
    };
}

trait Ctx<T> {
    /// A failed call as a FAIL naming the call, its error and its ErrorKind.
    fn ctx(self, what: &str) -> Result<T, CaseError>;
}

impl<T> Ctx<T> for io::Result<T> {
    fn ctx(self, what: &str) -> Result<T, CaseError> {
        self.map_err(|e| CaseError::Fail(format!("{what} failed: {e} ({:?})", e.kind())))
    }
}

/// Pass when `result` failed with `kind`.
fn want_kind<T>(what: &str, result: io::Result<T>, kind: ErrorKind) -> CaseResult {
    match result {
        Ok(_) => fail(format!("{what} succeeded; expected {kind:?}")),
        Err(e) if e.kind() == kind => Ok(()),
        Err(e) => fail(format!("{what} failed with {e} ({:?}); expected {kind:?}", e.kind())),
    }
}

fn ms(d: Duration) -> u128 { d.as_millis() }

/// Check that a wait meant to last `want` lasted `took`: never less, at most LATE_MS more,
/// reporting how late it ended.
fn on_time(what: &str, took: Duration, want: Duration) -> CaseResult {
    let late_us = took.as_micros() as i64 - want.as_micros() as i64;
    value("late", late_us, "us", Some((0, LATE_MS as i64 * 1000)));
    check(took >= want, &format!("{what} ended after {} us, before its {} ms", took.as_micros(), ms(want)))?;
    check(took <= want + Duration::from_millis(LATE_MS as u64),
        &format!("{what} ended {} us late; at most {LATE_MS} ms is allowed", late_us))
}

/// A directory of the case's own, removed with everything in it when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Scratch, CaseError> {
        let dir = PathBuf::from(format!("/tmp/rust-std-{}", std::process::id()));
        clear(&dir);
        fs::create_dir(&dir).ctx("create_dir of the case directory")?;
        Ok(Scratch(dir))
    }

    fn path(&self, name: &str) -> PathBuf { self.0.join(name) }
}

impl Drop for Scratch {
    fn drop(&mut self) { clear(&self.0) }
}

/// Remove `path` and everything under it, without fs::remove_dir_all, which a case measures.
fn clear(path: &Path) {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => {
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    clear(&entry.path());
                }
            }
            let _ = fs::remove_dir(path);
        }
        Ok(_) => { let _ = fs::remove_file(path); }
        Err(_) => {}
    }
}

fn write_file(path: &Path, bytes: &[u8]) -> CaseResult {
    fs::write(path, bytes).ctx("fs::write")
}

/// Bytes with no runs, so a misplaced block shows.
fn pattern(len: usize) -> Vec<u8> { (0..len).map(|i| (i * 7 % 251) as u8).collect() }

/// Wait up to WAIT_MS for `handle`'s thread to finish, then join it.
fn finish<T>(handle: thread::JoinHandle<T>, what: &str) -> Result<T, CaseError> {
    let start = Instant::now();
    while !handle.is_finished() {
        if start.elapsed() > Duration::from_millis(WAIT_MS) {
            return Err(CaseError::Fail(format!("{what} was still blocked after {WAIT_MS} ms")));
        }
        thread::sleep(Duration::from_millis(1));
    }
    handle.join().map_err(|_| CaseError::Fail(format!("{what} panicked")))
}

/// Processors online, from the `processor` lines of /proc/cpuinfo.
fn processors() -> Result<usize, CaseError> {
    let info = fs::read_to_string("/proc/cpuinfo").ctx("reading /proc/cpuinfo")?;
    let n = info.lines().filter(|line| line.starts_with("processor")).count();
    if n == 0 {
        return Err("/proc/cpuinfo lists no processor".into());
    }
    Ok(n)
}

spawn_items! {
    use std::process::{Child, Command, ExitStatus, Output, Stdio};
    use std::os::unix::process::{CommandExt, ExitStatusExt};

    /// The helper program the process cases start.
    const CHILD: &str = "/usr/libexec/breenix/rust-std-child_test";
    const CHILD_DIR: &str = "/usr/libexec/breenix";
    const CHILD_NAME: &str = "rust-std-child_test";
    /// The helper's hard link whose final name ends in a byte that is not UTF-8.
    const NON_UTF8_CHILD: &[u8] = b"/usr/libexec/breenix/rust-std-child_test\xff";
    const SIGKILL: i32 = 9;
    const SIGABRT: i32 = 6;

    fn helper(args: &[&str]) -> Command {
        let mut command = Command::new(CHILD);
        command.args(args);
        command
    }

    /// Wait up to WAIT_MS for `child` to exit; kill it and fail if it has not.
    fn wait_within(child: &mut Child) -> Result<ExitStatus, CaseError> {
        let start = Instant::now();
        loop {
            if let Some(status) = child.try_wait().ctx("Child::try_wait")? {
                return Ok(status);
            }
            if start.elapsed() > Duration::from_millis(WAIT_MS) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(CaseError::Fail(format!("the program was still running after {WAIT_MS} ms")));
            }
            thread::sleep(Duration::from_millis(2));
        }
    }

    /// Start `command` with stdin empty and stdout and stderr piped, wait for it within
    /// WAIT_MS, then read what it wrote.
    fn run(command: &mut Command) -> Result<Output, CaseError> {
        let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().ctx("spawn")?;
        let status = wait_within(&mut child)?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        if let Some(mut pipe) = child.stdout.take() {
            pipe.read_to_end(&mut stdout).ctx("reading the program's stdout")?;
        }
        if let Some(mut pipe) = child.stderr.take() {
            pipe.read_to_end(&mut stderr).ctx("reading the program's stderr")?;
        }
        Ok(Output { status, stdout, stderr })
    }

    /// Run `command` and require it to exit 0, returning its stdout as text.
    fn run_ok(command: &mut Command) -> Result<String, CaseError> {
        let out = run(command)?;
        if !out.status.success() {
            return Err(CaseError::Fail(format!("the program ended with {}", out.status)));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Run the helper with `args` and return its exit status.
    fn status_of(args: &[&str]) -> Result<ExitStatus, CaseError> {
        Ok(run(&mut helper(args))?.status)
    }

    fn want_spawn_kind(what: &str, command: &mut Command, kind: ErrorKind) -> CaseResult {
        let result = command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
        if let Ok(mut child) = result {
            let status = wait_within(&mut child)?;
            return fail(format!("{what} started a program, which ended with {status}; expected {kind:?}"));
        }
        want_kind(what, result, kind)
    }
}

// --- fs-io ------------------------------------------------------------------------------

fn create_read_back() -> CaseResult {
    let dir = Scratch::new()?;
    let path = dir.path("data");
    let bytes = pattern(5000);
    File::create(&path).ctx("File::create")?.write_all(&bytes).ctx("write_all")?;
    let back = fs::read(&path).ctx("fs::read")?;
    check(back == bytes, &format!("fs::read returned {} bytes that differ from the {} written", back.len(), bytes.len()))
}

fn create_truncates() -> CaseResult {
    let dir = Scratch::new()?;
    let path = dir.path("data");
    write_file(&path, &pattern(100))?;
    let file = File::create(&path).ctx("File::create of the existing file")?;
    let len = file.metadata().ctx("File::metadata")?.len();
    check(len == 0, &format!("the file is {len} bytes after File::create; expected 0"))?;
    let back = fs::read(&path).ctx("fs::read")?;
    check(back.is_empty(), &format!("fs::read returned {} bytes after File::create", back.len()))
}

fn open_missing() -> CaseResult {
    let dir = Scratch::new()?;
    want_kind("File::open of a missing path", File::open(dir.path("missing")), ErrorKind::NotFound)
}

fn create_new_exists() -> CaseResult {
    let dir = Scratch::new()?;
    let path = dir.path("data");
    write_file(&path, b"kept")?;
    want_kind("create_new on an existing file",
        OpenOptions::new().write(true).create_new(true).open(&path), ErrorKind::AlreadyExists)
}

fn append() -> CaseResult {
    let dir = Scratch::new()?;
    let path = dir.path("data");
    write_file(&path, b"abc")?;
    let mut file = OpenOptions::new().append(true).open(&path).ctx("OpenOptions::append open")?;
    file.seek(SeekFrom::Start(0)).ctx("seek to the start")?;
    file.write_all(b"def").ctx("write_all")?;
    let back = fs::read(&path).ctx("fs::read")?;
    check(back == b"abcdef", &format!("the file holds {:?}; expected \"abcdef\"", String::from_utf8_lossy(&back)))
}

fn write_read_only() -> CaseResult {
    let dir = Scratch::new()?;
    let path = dir.path("data");
    write_file(&path, b"abc")?;
    let mut file = File::open(&path).ctx("File::open")?;
    match file.write(b"x") {
        Ok(n) => fail(format!("write through a read-only File wrote {n} bytes; expected EBADF")),
        Err(e) if e.raw_os_error() == Some(9) => Ok(()),
        Err(e) => fail(format!("write through a read-only File failed with {e}; expected EBADF (9)")),
    }
}

fn seek_from_end() -> CaseResult {
    let dir = Scratch::new()?;
    let path = dir.path("data");
    write_file(&path, b"0123456789")?;
    let mut file = File::open(&path).ctx("File::open")?;
    let at = file.seek(SeekFrom::End(-3)).ctx("seek to End(-3)")?;
    check(at == 7, &format!("seek to End(-3) returned {at}; expected 7"))?;
    let mut rest = String::new();
    file.read_to_string(&mut rest).ctx("read_to_string")?;
    check(rest == "789", &format!("read after the seek returned {rest:?}; expected \"789\""))
}

fn seek_before_start() -> CaseResult {
    let dir = Scratch::new()?;
    let path = dir.path("data");
    write_file(&path, b"0123456789")?;
    let mut file = File::open(&path).ctx("File::open")?;
    want_kind("seek to Current(-1) at offset 0", file.seek(SeekFrom::Current(-1)), ErrorKind::InvalidInput)
}

fn seek_past_end() -> CaseResult {
    let dir = Scratch::new()?;
    let path = dir.path("data");
    let mut file = File::create(&path).ctx("File::create")?;
    file.write_all(b"ab").ctx("write_all")?;
    file.seek(SeekFrom::Start(10_000)).ctx("seek past the end")?;
    file.write_all(b"cd").ctx("write_all after the seek")?;
    drop(file);
    let back = fs::read(&path).ctx("fs::read")?;
    check(back.len() == 10_002, &format!("the file is {} bytes; expected 10002", back.len()))?;
    check(&back[..2] == b"ab" && &back[10_000..] == b"cd", "the bytes written are not where they were written")?;
    check(back[2..10_000].iter().all(|&b| b == 0), "the gap does not read back as zeros")
}

fn set_len() -> CaseResult {
    let dir = Scratch::new()?;
    let path = dir.path("data");
    write_file(&path, b"hello")?;
    let file = OpenOptions::new().write(true).open(&path).ctx("open for writing")?;
    file.set_len(4096).ctx("set_len(4096)")?;
    let back = fs::read(&path).ctx("fs::read")?;
    check(back.len() == 4096, &format!("the file is {} bytes after set_len(4096)", back.len()))?;
    check(&back[..5] == b"hello" && back[5..].iter().all(|&b| b == 0), "set_len(4096) did not keep the data and add zeros")?;
    file.set_len(2).ctx("set_len(2)")?;
    let back = fs::read(&path).ctx("fs::read")?;
    check(back == b"he", &format!("the file holds {:?} after set_len(2); expected \"he\"", String::from_utf8_lossy(&back)))
}

fn try_clone_offset() -> CaseResult {
    let dir = Scratch::new()?;
    let path = dir.path("data");
    write_file(&path, b"0123456789")?;
    let mut file = File::open(&path).ctx("File::open")?;
    let mut clone = file.try_clone().ctx("File::try_clone")?;
    let mut buf = [0u8; 3];
    file.read_exact(&mut buf).ctx("read through the original")?;
    clone.read_exact(&mut buf).ctx("read through the clone")?;
    check(&buf == b"345", &format!("the clone read {:?}; expected \"345\" after the original read three bytes",
        String::from_utf8_lossy(&buf)))
}

fn sync() -> CaseResult {
    let dir = Scratch::new()?;
    let mut file = File::create(dir.path("data")).ctx("File::create")?;
    file.write_all(&pattern(64 * 1024)).ctx("write_all")?;
    file.sync_all().ctx("sync_all")?;
    file.write_all(&pattern(4096)).ctx("write_all")?;
    file.sync_data().ctx("sync_data")
}

fn metadata() -> CaseResult {
    let dir = Scratch::new()?;
    let path = dir.path("data");
    write_file(&path, &pattern(1234))?;
    let meta = fs::metadata(&path).ctx("metadata of the file")?;
    check(meta.len() == 1234, &format!("len is {}; expected 1234", meta.len()))?;
    check(meta.is_file() && !meta.is_dir() && !meta.is_symlink(), "the file's type is not a regular file")?;
    check(meta.nlink() == 1, &format!("nlink is {}; expected 1", meta.nlink()))?;
    let meta = fs::metadata(&dir.0).ctx("metadata of the directory")?;
    check(meta.is_dir() && !meta.is_file(), "the directory's type is not a directory")
}

fn metadata_missing() -> CaseResult {
    let dir = Scratch::new()?;
    want_kind("metadata of a missing path", fs::metadata(dir.path("missing")), ErrorKind::NotFound)
}

fn permissions_mode() -> CaseResult {
    let dir = Scratch::new()?;
    let path = dir.path("data");
    write_file(&path, b"x")?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).ctx("set_permissions(0o640)")?;
    let mode = fs::metadata(&path).ctx("metadata")?.permissions().mode() & 0o7777;
    check(mode == 0o640, &format!("mode is {mode:o}; expected 640"))
}

fn permissions_readonly() -> CaseResult {
    let dir = Scratch::new()?;
    let path = dir.path("data");
    write_file(&path, b"x")?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o664)).ctx("set_permissions(0o664)")?;
    let mut perms = fs::metadata(&path).ctx("metadata")?.permissions();
    perms.set_readonly(true);
    fs::set_permissions(&path, perms).ctx("set_permissions(readonly)")?;
    let perms = fs::metadata(&path).ctx("metadata")?.permissions();
    check(perms.readonly(), "readonly() is false after set_readonly(true)")?;
    check(perms.mode() & 0o7777 == 0o444, &format!("mode is {:o}; expected 444", perms.mode() & 0o7777))
}

fn modified_now() -> CaseResult {
    let dir = Scratch::new()?;
    let path = dir.path("data");
    write_file(&path, b"x")?;
    let modified = fs::metadata(&path).ctx("metadata")?.modified().ctx("Metadata::modified")?;
    let now = SystemTime::now();
    let apart = now.duration_since(modified).unwrap_or_else(|e| e.duration());
    value("apart", apart.as_millis() as i64, "ms", Some((0, 5000)));
    check(apart <= Duration::from_secs(5), &format!("the modified time is {} ms from SystemTime::now", apart.as_millis()))
}

fn set_modified() -> CaseResult {
    let dir = Scratch::new()?;
    let path = dir.path("data");
    write_file(&path, b"x")?;
    let when = UNIX_EPOCH + Duration::from_secs(1_600_000_000);
    let file = OpenOptions::new().write(true).open(&path).ctx("open for writing")?;
    file.set_modified(when).ctx("File::set_modified")?;
    drop(file);
    let modified = fs::metadata(&path).ctx("metadata")?.modified().ctx("Metadata::modified")?;
    let secs = modified.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    check(secs == 1_600_000_000, &format!("the modified time is {secs} s after the epoch; expected 1600000000"))
}

fn read_dir() -> CaseResult {
    let dir = Scratch::new()?;
    write_file(&dir.path("a"), b"a")?;
    write_file(&dir.path("b"), b"b")?;
    fs::create_dir(dir.path("c")).ctx("create_dir")?;
    symlink("a", dir.path("d")).ctx("symlink")?;
    let mut seen = Vec::new();
    for entry in fs::read_dir(&dir.0).ctx("read_dir")? {
        let entry = entry.ctx("reading an entry")?;
        let kind = entry.file_type().ctx("DirEntry::file_type")?;
        let kind = if kind.is_symlink() { "symlink" } else if kind.is_dir() { "dir" } else if kind.is_file() { "file" } else { "other" };
        seen.push(format!("{}:{kind}", entry.file_name().to_string_lossy()));
    }
    seen.sort();
    let want = ["a:file", "b:file", "c:dir", "d:symlink"];
    check(seen == want, &format!("read_dir listed {seen:?}; expected {want:?}"))
}

fn read_dir_missing() -> CaseResult {
    let dir = Scratch::new()?;
    want_kind("read_dir of a missing path", fs::read_dir(dir.path("missing")), ErrorKind::NotFound)
}

fn create_dir_all() -> CaseResult {
    let dir = Scratch::new()?;
    let deep = dir.path("x/y/z");
    fs::create_dir_all(&deep).ctx("create_dir_all")?;
    fs::read_dir(&deep).ctx("read_dir of the path create_dir_all made")?;
    fs::create_dir_all(&deep).ctx("create_dir_all of an existing path")
}

fn remove_dir_not_empty() -> CaseResult {
    let dir = Scratch::new()?;
    fs::create_dir(dir.path("full")).ctx("create_dir")?;
    write_file(&dir.path("full/entry"), b"x")?;
    want_kind("remove_dir of a non-empty directory", fs::remove_dir(dir.path("full")), ErrorKind::DirectoryNotEmpty)
}

fn remove_dir_all() -> CaseResult {
    linked!(["fdopendir", "unlinkat"], "fs::remove_dir_all", {
        let dir = Scratch::new()?;
        let top = dir.path("tree");
        fs::create_dir_all(top.join("sub/deeper")).ctx("create_dir_all")?;
        fs::create_dir(top.join("empty")).ctx("create_dir")?;
        write_file(&top.join("a"), b"a")?;
        write_file(&top.join("sub/b"), b"b")?;
        write_file(&top.join("sub/deeper/c"), b"c")?;
        symlink("../a", top.join("sub/link")).ctx("symlink")?;
        // A symlink to a directory outside the tree is removed, not followed.
        let outside = dir.path("outside");
        fs::create_dir(&outside).ctx("create_dir of the outside directory")?;
        write_file(&outside.join("keep"), b"keep")?;
        symlink(&outside, top.join("sub/escape")).ctx("symlink to the outside directory")?;
        fs::remove_dir_all(&top).ctx("remove_dir_all")?;
        want_kind("symlink_metadata of the removed tree", fs::symlink_metadata(&top), ErrorKind::NotFound)?;
        check(outside.join("keep").is_file(), "remove_dir_all followed sub/escape and removed outside/keep")
    })
}

fn rename() -> CaseResult {
    let dir = Scratch::new()?;
    write_file(&dir.path("old"), b"moved")?;
    fs::rename(dir.path("old"), dir.path("new")).ctx("rename")?;
    let back = fs::read(dir.path("new")).ctx("fs::read of the new path")?;
    check(back == b"moved", "the new path does not hold the file's contents")?;
    want_kind("metadata of the old path", fs::metadata(dir.path("old")), ErrorKind::NotFound)
}

fn rename_replace() -> CaseResult {
    let dir = Scratch::new()?;
    write_file(&dir.path("from"), b"new")?;
    write_file(&dir.path("to"), b"old contents")?;
    fs::rename(dir.path("from"), dir.path("to")).ctx("rename onto an existing file")?;
    let back = fs::read(dir.path("to")).ctx("fs::read")?;
    check(back == b"new", &format!("the target holds {:?}; expected \"new\"", String::from_utf8_lossy(&back)))?;
    want_kind("metadata of the old path", fs::metadata(dir.path("from")), ErrorKind::NotFound)
}

fn hard_link() -> CaseResult {
    let dir = Scratch::new()?;
    let (a, b) = (dir.path("a"), dir.path("b"));
    write_file(&a, b"one")?;
    fs::hard_link(&a, &b).ctx("hard_link")?;
    let (ma, mb) = (fs::metadata(&a).ctx("metadata")?, fs::metadata(&b).ctx("metadata")?);
    check(ma.ino() == mb.ino(), &format!("the names have inodes {} and {}", ma.ino(), mb.ino()))?;
    check(ma.nlink() == 2 && mb.nlink() == 2, &format!("nlink is {} and {}; expected 2", ma.nlink(), mb.nlink()))?;
    OpenOptions::new().append(true).open(&b).ctx("open the link to append")?.write_all(b"two").ctx("write_all")?;
    let back = fs::read(&a).ctx("fs::read")?;
    check(back == b"onetwo", &format!("the first name reads {:?}; expected \"onetwo\"", String::from_utf8_lossy(&back)))
}

fn symlink_case() -> CaseResult {
    let dir = Scratch::new()?;
    write_file(&dir.path("target"), b"target data")?;
    let link = dir.path("link");
    symlink("target", &link).ctx("symlink")?;
    let target = fs::read_link(&link).ctx("read_link")?;
    check(target == Path::new("target"), &format!("read_link returned {target:?}; expected \"target\""))?;
    let lmeta = fs::symlink_metadata(&link).ctx("symlink_metadata")?;
    check(lmeta.file_type().is_symlink(), "symlink_metadata does not report a symlink")?;
    let meta = fs::metadata(&link).ctx("metadata through the link")?;
    check(meta.is_file() && meta.len() == 11, "metadata through the link does not describe the target")?;
    let back = fs::read(&link).ctx("fs::read through the link")?;
    check(back == b"target data", "reading through the link does not return the target's contents")
}

fn symlink_dangling() -> CaseResult {
    let dir = Scratch::new()?;
    let link = dir.path("link");
    symlink("missing", &link).ctx("symlink")?;
    want_kind("metadata through a dangling link", fs::metadata(&link), ErrorKind::NotFound)?;
    let meta = fs::symlink_metadata(&link).ctx("symlink_metadata of a dangling link")?;
    check(meta.file_type().is_symlink(), "symlink_metadata does not report a symlink")
}

fn canonicalize() -> CaseResult {
    linked!(["realpath"], "fs::canonicalize", {
        let dir = Scratch::new()?;
        fs::create_dir(dir.path("real")).ctx("create_dir")?;
        write_file(&dir.path("real/file"), b"x")?;
        symlink("real", dir.path("ln")).ctx("symlink")?;
        let got = fs::canonicalize(dir.path("ln/../real/./file")).ctx("canonicalize")?;
        let want = dir.path("real/file");
        check(got == want, &format!("canonicalize returned {got:?}; expected {want:?}"))
    })
}

fn buf_reader_lines() -> CaseResult {
    let dir = Scratch::new()?;
    let path = dir.path("data");
    write_file(&path, b"one\ntwo\r\n\nthree")?;
    let lines: Vec<String> = BufReader::new(File::open(&path).ctx("File::open")?).lines()
        .collect::<io::Result<_>>().ctx("BufReader::lines")?;
    let want = ["one", "two", "", "three"];
    check(lines == want, &format!("lines returned {lines:?}; expected {want:?}"))
}

fn buf_writer() -> CaseResult {
    let dir = Scratch::new()?;
    let path = dir.path("data");
    let mut writer = BufWriter::with_capacity(64, File::create(&path).ctx("File::create")?);
    writer.write_all(b"0123456789").ctx("write_all")?;
    let before = fs::metadata(&path).ctx("metadata")?.len();
    check(before == 0, &format!("the file is {before} bytes before flush; expected 0"))?;
    writer.flush().ctx("flush")?;
    let back = fs::read(&path).ctx("fs::read")?;
    check(back == b"0123456789", &format!("the file holds {} bytes after flush; expected 10", back.len()))
}

fn io_copy() -> CaseResult {
    let dir = Scratch::new()?;
    let bytes = pattern(300 * 1024);
    write_file(&dir.path("src"), &bytes)?;
    let mut src = File::open(dir.path("src")).ctx("File::open")?;
    let mut dst = File::create(dir.path("dst")).ctx("File::create")?;
    let n = io::copy(&mut src, &mut dst).ctx("io::copy")?;
    check(n == bytes.len() as u64, &format!("io::copy returned {n}; expected {}", bytes.len()))?;
    drop(dst);
    check(fs::read(dir.path("dst")).ctx("fs::read")? == bytes, "the copy differs from the source")
}

fn fs_copy() -> CaseResult {
    let dir = Scratch::new()?;
    let bytes = pattern(20_000);
    write_file(&dir.path("src"), &bytes)?;
    fs::set_permissions(dir.path("src"), fs::Permissions::from_mode(0o751)).ctx("set_permissions")?;
    let n = fs::copy(dir.path("src"), dir.path("dst")).ctx("fs::copy")?;
    check(n == bytes.len() as u64, &format!("fs::copy returned {n}; expected {}", bytes.len()))?;
    check(fs::read(dir.path("dst")).ctx("fs::read")? == bytes, "the copy differs from the source")?;
    let mode = fs::metadata(dir.path("dst")).ctx("metadata")?.permissions().mode() & 0o7777;
    check(mode == 0o751, &format!("the copy's mode is {mode:o}; expected 751"))
}

fn create_on_directory() -> CaseResult {
    let dir = Scratch::new()?;
    want_kind("File::create on a directory", File::create(&dir.0), ErrorKind::IsADirectory)
}

fn std_streams() -> CaseResult {
    let fds = (io::stdin().as_raw_fd(), io::stdout().as_raw_fd(), io::stderr().as_raw_fd());
    check(fds == (0, 1, 2), &format!("stdin, stdout and stderr are descriptors {fds:?}"))?;
    let mut out = io::stdout().lock();
    out.write_all(b"rust-std stdout\n").ctx("write_all to stdout")?;
    out.flush().ctx("flush of stdout")?;
    io::stderr().write_all(b"rust-std stderr\n").ctx("write_all to stderr")
}

fn stdin_pipe() -> CaseResult {
    spawning!({
        let bytes: Vec<u8> = (0..1000).flat_map(|i| format!("line {i}\n").into_bytes()).collect();
        let mut child = helper(&["cat"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
            .spawn().ctx("spawn")?;
        child.stdin.take().ok_or("no Child::stdin")?.write_all(&bytes).ctx("writing the program's stdin")?;
        let mut back = Vec::new();
        child.stdout.take().ok_or("no Child::stdout")?.read_to_end(&mut back).ctx("reading the program's stdout")?;
        let status = wait_within(&mut child)?;
        check(status.success(), &format!("the program ended with {status}"))?;
        check(back == bytes, &format!("the program read {} bytes that differ from the {} written", back.len(), bytes.len()))
    })
}

// --- process ----------------------------------------------------------------------------

fn status_success() -> CaseResult {
    spawning!({
        let status = helper(&["exit", "0"]).stdin(Stdio::null()).stdout(Stdio::null()).status().ctx("Command::status")?;
        check(status.success() && status.code() == Some(0), &format!("the status is {status}; expected exit code 0"))
    })
}

fn exit_code() -> CaseResult {
    spawning!({
        let status = status_of(&["exit", "42"])?;
        check(status.code() == Some(42) && !status.success() && status.signal().is_none(),
            &format!("the status is {status}; expected exit code 42"))
    })
}

fn exit_low_bits() -> CaseResult {
    spawning!({
        let status = status_of(&["exit", "263"])?;
        check(status.code() == Some(7), &format!("the status is {status}; expected exit code 7"))
    })
}

fn into_raw_exit() -> CaseResult {
    spawning!({
        let raw = status_of(&["exit", "42"])?.into_raw();
        check(raw == 0x2a00, &format!("into_raw is {raw:#x}; expected 0x2a00"))
    })
}

fn args() -> CaseResult {
    spawning!({
        let out = run_ok(&mut helper(&["args", "one", "", "two words", "three"]))?;
        check(out == "one\n\ntwo words\nthree\n", &format!("the program saw {out:?}"))
    })
}

fn args_non_utf8() -> CaseResult {
    spawning!({
        let arg = std::ffi::OsStr::from_bytes(b"caf\xe9\xff!");
        let out = run(helper(&["args"]).arg(arg))?;
        check(out.stdout == b"caf\xe9\xff!\n", &format!("the program saw the bytes {:?}", out.stdout))
    })
}

fn arg0() -> CaseResult {
    spawning!({
        let out = run_ok(Command::new(CHILD).arg0("custom-zero").arg("argv0"))?;
        check(out == "custom-zero\n", &format!("the program's argv[0] is {out:?}; expected \"custom-zero\""))
    })
}

fn env_set() -> CaseResult {
    spawning!({
        let out = run_ok(helper(&["env", "RUST_STD_A"]).env("RUST_STD_A", "alpha"))?;
        check(out == "RUST_STD_A=alpha\n", &format!("the program saw {out:?}"))
    })
}

fn env_inherit() -> CaseResult {
    spawning!({
        std::env::set_var("RUST_STD_B", "beta");
        let out = run_ok(&mut helper(&["env", "RUST_STD_B"]))?;
        check(out == "RUST_STD_B=beta\n", &format!("the program saw {out:?}"))
    })
}

fn env_remove() -> CaseResult {
    spawning!({
        set_and_confirm("RUST_STD_C", "gamma")?;
        let out = run_ok(helper(&["env", "RUST_STD_C"]).env_remove("RUST_STD_C"))?;
        check(out == "RUST_STD_C unset\n", &format!("the program saw {out:?}"))
    })
}

fn env_clear() -> CaseResult {
    spawning!({
        set_and_confirm("RUST_STD_D", "delta")?;
        let out = run_ok(helper(&["envall"]).env_clear().env("ONLY", "1"))?;
        check(out == "ONLY=1\n", &format!("the program's environment is {out:?}; expected only ONLY=1"))
    })
}

fn current_dir() -> CaseResult {
    spawning!({
        let dir = Scratch::new()?;
        let out = run_ok(helper(&["cwd"]).current_dir(&dir.0))?;
        let want = format!("{}\n", dir.0.display());
        check(out == want, &format!("the program's directory is {out:?}; expected {want:?}"))
    })
}

fn current_dir_missing() -> CaseResult {
    spawning!({
        let missing = format!("/tmp/rust-std-{}-missing", std::process::id());
        want_spawn_kind("spawn in a missing directory", helper(&["cwd"]).current_dir(missing), ErrorKind::NotFound)
    })
}

fn spawn_not_found() -> CaseResult {
    spawning!({
        want_spawn_kind("spawn of a missing path", &mut Command::new("/usr/libexec/breenix/rust-std-no-such-program"),
            ErrorKind::NotFound)
    })
}

fn spawn_path_missing() -> CaseResult {
    spawning!({
        want_spawn_kind("spawn of a name in no PATH directory",
            Command::new("rust-std-no-such-program").env("PATH", format!("{CHILD_DIR}:/bin:/sbin")), ErrorKind::NotFound)
    })
}

fn spawn_path_found() -> CaseResult {
    spawning!({
        let mut command = Command::new(CHILD_NAME);
        command.env("PATH", format!("/rust-std-no-such-dir:{CHILD_DIR}")).args(["exit", "5"]);
        let status = run(&mut command)?.status;
        check(status.code() == Some(5), &format!("the status is {status}; expected exit code 5"))
    })
}

fn spawn_not_executable() -> CaseResult {
    spawning!({
        let dir = Scratch::new()?;
        let path = dir.path("plain");
        fs::copy(CHILD, &path).ctx("fs::copy of the helper")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).ctx("set_permissions(0o644)")?;
        want_spawn_kind("spawn of a file without execute permission", &mut Command::new(&path), ErrorKind::PermissionDenied)
    })
}

fn spawn_directory() -> CaseResult {
    spawning!({
        let dir = Scratch::new()?;
        want_spawn_kind("spawn of a directory", &mut Command::new(&dir.0), ErrorKind::PermissionDenied)
    })
}

fn output() -> CaseResult {
    spawning!({
        let out = helper(&["split"]).output().ctx("Command::output")?;
        check(out.status.success(), &format!("the program ended with {}", out.status))?;
        check(out.stdout == b"out" && out.stderr == b"err", &format!("stdout is {:?} and stderr {:?}; expected \"out\" and \"err\"",
            String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    })
}

fn stdout_piped() -> CaseResult {
    spawning!({
        let mut child = helper(&["args", "a", "b"]).stdin(Stdio::null()).stdout(Stdio::piped()).spawn().ctx("spawn")?;
        let mut text = String::new();
        child.stdout.take().ok_or("no Child::stdout")?.read_to_string(&mut text).ctx("reading Child::stdout")?;
        let status = wait_within(&mut child)?;
        check(status.success(), &format!("the program ended with {status}"))?;
        check(text == "a\nb\n", &format!("Child::stdout held {text:?}; expected \"a\\nb\\n\""))
    })
}

fn stdin_piped() -> CaseResult {
    spawning!({
        let mut child = helper(&["cat"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().ctx("spawn")?;
        child.stdin.take().ok_or("no Child::stdin")?.write_all(b"hello\nworld\n").ctx("writing Child::stdin")?;
        let mut text = String::new();
        child.stdout.take().ok_or("no Child::stdout")?.read_to_string(&mut text).ctx("reading Child::stdout")?;
        let status = wait_within(&mut child)?;
        check(status.success(), &format!("the program ended with {status}"))?;
        check(text == "hello\nworld\n", &format!("the program echoed {text:?}"))
    })
}

fn stdin_large() -> CaseResult {
    spawning!({
        let bytes = pattern(256 * 1024);
        let mut child = helper(&["cat"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().ctx("spawn")?;
        let mut stdin = child.stdin.take().ok_or("no Child::stdin")?;
        let sent = bytes.clone();
        let writer = thread::spawn(move || stdin.write_all(&sent));
        let mut back = Vec::new();
        child.stdout.take().ok_or("no Child::stdout")?.read_to_end(&mut back).ctx("reading Child::stdout")?;
        finish(writer, "the writing thread")?.ctx("writing Child::stdin")?;
        let status = wait_within(&mut child)?;
        check(status.success(), &format!("the program ended with {status}"))?;
        check(back == bytes, &format!("{} bytes came back that differ from the {} written", back.len(), bytes.len()))
    })
}

fn stdio_null() -> CaseResult {
    spawning!({
        let out = helper(&["cat"]).stdin(Stdio::null()).output().ctx("Command::output")?;
        check(out.status.success() && out.stdout.is_empty(),
            &format!("cat of a null stdin ended with {} and wrote {} bytes", out.status, out.stdout.len()))?;
        let out = helper(&["split"]).stdout(Stdio::null()).output().ctx("Command::output")?;
        check(out.stdout.is_empty() && out.stderr == b"err",
            &format!("with a null stdout, {} bytes were captured from stdout", out.stdout.len()))
    })
}

fn stdout_to_file() -> CaseResult {
    spawning!({
        let dir = Scratch::new()?;
        let file = File::create(dir.path("out")).ctx("File::create")?;
        let mut child = helper(&["args", "to-file"]).stdin(Stdio::null()).stdout(Stdio::from(file)).spawn().ctx("spawn")?;
        let status = wait_within(&mut child)?;
        check(status.success(), &format!("the program ended with {status}"))?;
        let text = fs::read_to_string(dir.path("out")).ctx("reading the file")?;
        check(text == "to-file\n", &format!("the file holds {text:?}"))
    })
}

fn wait() -> CaseResult {
    spawning!({
        let mut child = helper(&["exit", "9"]).stdin(Stdio::null()).spawn().ctx("spawn")?;
        let first = child.wait().ctx("Child::wait")?;
        check(first.code() == Some(9), &format!("wait returned {first}; expected exit code 9"))?;
        let second = child.wait().ctx("a second Child::wait")?;
        check(second == first, &format!("the second wait returned {second}; the first {first}"))
    })
}

fn try_wait() -> CaseResult {
    spawning!({
        let mut child = helper(&["sleep", "300"]).stdin(Stdio::null()).spawn().ctx("spawn")?;
        let early = child.try_wait().ctx("Child::try_wait")?;
        if let Some(status) = early {
            return fail(format!("try_wait returned {status} for a program sleeping 300 ms"));
        }
        let status = wait_within(&mut child)?;
        check(status.success(), &format!("the program ended with {status}"))
    })
}

fn kill() -> CaseResult {
    spawning!({
        let mut child = helper(&["sleep", "5000"]).stdin(Stdio::null()).spawn().ctx("spawn")?;
        child.kill().ctx("Child::kill")?;
        let status = wait_within(&mut child)?;
        check(status.signal() == Some(SIGKILL) && status.code().is_none(),
            &format!("the status is {status}; expected death by SIGKILL"))
    })
}

fn kill_exited() -> CaseResult {
    spawning!({
        let mut child = helper(&["exit", "4"]).stdin(Stdio::null()).spawn().ctx("spawn")?;
        let path = format!("/proc/{}/status", child.id());
        let start = Instant::now();
        loop {
            let status = fs::read_to_string(&path).ctx(&format!("reading {path}"))?;
            let state = status.lines().find(|line| line.starts_with("State:")).unwrap_or("").to_string();
            if state.contains('Z') {
                break;
            }
            if start.elapsed() > Duration::from_millis(WAIT_MS) {
                let _ = child.kill();
                let _ = child.wait();
                return fail(format!("the program had not exited after {WAIT_MS} ms; {path} says {state:?}"));
            }
            thread::sleep(Duration::from_millis(2));
        }
        child.kill().ctx("Child::kill of an exited program")?;
        let status = wait_within(&mut child)?;
        check(status.code() == Some(4), &format!("wait after kill returned {status}; expected exit code 4"))
    })
}

fn abort_signal() -> CaseResult {
    spawning!({
        let status = status_of(&["abort"])?;
        check(status.signal() == Some(SIGABRT), &format!("the status is {status}; expected death by SIGABRT"))
    })
}

fn signal_into_raw() -> CaseResult {
    spawning!({
        let mut child = helper(&["sleep", "5000"]).stdin(Stdio::null()).spawn().ctx("spawn")?;
        child.kill().ctx("Child::kill")?;
        let raw = wait_within(&mut child)?.into_raw();
        check(raw & 0x7f == SIGKILL, &format!("into_raw is {raw:#x}; expected 9 in its low 7 bits"))
    })
}

fn child_id() -> CaseResult {
    spawning!({
        let mut child = helper(&["ids"]).stdin(Stdio::null()).stdout(Stdio::piped()).spawn().ctx("spawn")?;
        let id = child.id();
        let mut text = String::new();
        child.stdout.take().ok_or("no Child::stdout")?.read_to_string(&mut text).ctx("reading Child::stdout")?;
        wait_within(&mut child)?;
        let want = format!("{id} {}\n", std::process::id());
        check(text == want, &format!("the program reported \"id parent_id\" as {text:?}; expected {want:?}"))
    })
}

fn process_id() -> CaseResult {
    let id = std::process::id();
    check(id != 0 && std::process::id() == id, &format!("process::id returned {id}, then {}", std::process::id()))?;
    let path = format!("/proc/{id}/status");
    let status = fs::read_to_string(&path).ctx(&format!("reading {path}"))?;
    let pid = status.lines().find_map(|line| line.strip_prefix("Pid:")).map(|rest| rest.trim().to_string());
    check(pid.as_deref() == Some(id.to_string().as_str()), &format!("{path} gives Pid {pid:?}; expected {id}"))
}

fn concurrent_children() -> CaseResult {
    spawning!({
        let mut children = Vec::new();
        for i in 0..8 {
            let code = (20 + i).to_string();
            children.push(helper(&["exit", &code]).stdin(Stdio::null()).spawn().ctx("spawn")?);
        }
        for (i, child) in children.iter_mut().enumerate() {
            let status = wait_within(child)?;
            check(status.code() == Some(20 + i as i32), &format!("program {i} ended with {status}; expected exit code {}", 20 + i))?;
        }
        Ok(())
    })
}

// --- thread-sync ------------------------------------------------------------------------

fn spawn_join() -> CaseResult {
    let got = finish(thread::spawn(|| 6 * 7), "the thread")?;
    check(got == 42, &format!("join returned {got}; expected 42"))
}

fn spawn_many() -> CaseResult {
    let handles: Vec<_> = (0..16u64).map(|i| thread::spawn(move || i * i)).collect();
    for (i, handle) in handles.into_iter().enumerate() {
        let got = finish(handle, "a thread")?;
        check(got == (i * i) as u64, &format!("thread {i} returned {got}; expected {}", i * i))?;
    }
    Ok(())
}

fn panic_aborts() -> CaseResult {
    spawning!({
        let status = status_of(&["thread-panic"])?;
        check(status.signal() == Some(SIGABRT), &format!("the status is {status}; expected death by SIGABRT"))
    })
}

fn panic_hook() -> CaseResult {
    spawning!({
        let out = run(&mut helper(&["panic-hook"]))?;
        check(out.stdout == b"hooked\n", &format!("the program wrote {:?}; expected the hook's \"hooked\"",
            String::from_utf8_lossy(&out.stdout)))?;
        check(out.status.signal() == Some(SIGABRT), &format!("the status is {}; expected death by SIGABRT", out.status))
    })
}

fn builder_name() -> CaseResult {
    let handle = thread::Builder::new().name("rust-std-worker".into())
        .spawn(|| thread::current().name().map(String::from)).ctx("Builder::spawn")?;
    check(handle.thread().name() == Some("rust-std-worker"), "Thread::name of the handle is not the given name")?;
    let name = finish(handle, "the thread")?;
    check(name.as_deref() == Some("rust-std-worker"), &format!("the thread's name is {name:?}"))
}

fn main_name() -> CaseResult {
    let name = thread::current().name().map(String::from);
    check(name.as_deref() == Some("main"), &format!("the main thread's name is {name:?}"))
}

/// Use 3 MiB of stack, touching every page of it.
#[inline(never)]
fn deep_stack() -> usize {
    let mut buf = [0u8; 3 << 20];
    for i in (0..buf.len()).step_by(4096) {
        buf[i] = 1;
    }
    std::hint::black_box(&mut buf);
    buf.iter().step_by(4096).map(|&b| b as usize).sum()
}

fn builder_stack_size() -> CaseResult {
    let handle = thread::Builder::new().stack_size(4 << 20).spawn(deep_stack).ctx("Builder::spawn")?;
    let pages = finish(handle, "the thread")?;
    check(pages == 768, &format!("the thread touched {pages} pages of its stack; expected 768"))
}

fn thread_ids() -> CaseResult {
    let main = thread::current().id();
    check(thread::current().id() == main, "the main thread's id changed between calls")?;
    let handle = thread::spawn(|| thread::current().id());
    let from_handle = handle.thread().id();
    let theirs = finish(handle, "the thread")?;
    check(theirs != main, "the thread has the main thread's id")?;
    check(theirs == from_handle, "the thread's own id differs from its JoinHandle's Thread::id")
}

fn is_finished() -> CaseResult {
    let (tx, rx) = mpsc::channel::<()>();
    let handle = thread::spawn(move || { let _ = rx.recv(); });
    thread::sleep(Duration::from_millis(20));
    check(!handle.is_finished(), "is_finished is true while the thread blocks in recv")?;
    tx.send(()).map_err(|_| "send failed")?;
    finish(handle, "the thread")
}

fn scoped_borrow() -> CaseResult {
    let mut slots = [0usize; 4];
    thread::scope(|s| {
        for (i, slot) in slots.iter_mut().enumerate() {
            s.spawn(move || *slot = (i + 1) * 10);
        }
    });
    check(slots == [10, 20, 30, 40], &format!("the scoped threads left {slots:?}"))
}

fn scoped_join() -> CaseResult {
    let data = vec![1u32, 2, 3, 4];
    let sum = thread::scope(|s| s.spawn(|| data.iter().sum::<u32>()).join());
    check(matches!(sum, Ok(10)), &format!("join returned {sum:?}; expected Ok(10)"))
}

fn mutex_counter() -> CaseResult {
    let counter = Arc::new(Mutex::new(0u64));
    let handles: Vec<_> = (0..4).map(|_| {
        let counter = Arc::clone(&counter);
        thread::spawn(move || for i in 0..10_000 {
            let mut guard = counter.lock().unwrap_or_else(|e| e.into_inner());
            let seen = *guard;
            if i % 1000 == 0 { thread::yield_now(); }
            *guard = seen + 1;
        })
    }).collect();
    for handle in handles { finish(handle, "a counting thread")?; }
    let total = *counter.lock().unwrap_or_else(|e| e.into_inner());
    check(total == 40_000, &format!("the counter is {total}; expected 40000"))
}

fn mutex_try_lock() -> CaseResult {
    let lock = Mutex::new(0);
    let _held = lock.lock().map_err(|_| "lock failed")?;
    let blocked = thread::scope(|s| s.spawn(|| matches!(lock.try_lock(), Err(TryLockError::WouldBlock))).join());
    check(matches!(blocked, Ok(true)), "try_lock in another thread did not fail with WouldBlock")
}

fn mutex_blocked_waiter() -> CaseResult {
    let lock = Arc::new(Mutex::new(()));
    let guard = lock.lock().map_err(|_| "lock failed")?;
    let theirs = Arc::clone(&lock);
    let handle = thread::spawn(move || { let _guard = theirs.lock(); Instant::now() });
    thread::sleep(Duration::from_millis(50));
    check(!handle.is_finished(), "the waiter got the lock while it was held")?;
    let released = Instant::now();
    drop(guard);
    let acquired = finish(handle, "the thread waiting in lock")?;
    check(acquired >= released, "the waiter got the lock before it was released")
}

fn rwlock_readers() -> CaseResult {
    let lock = Arc::new(RwLock::new(5));
    let _reader = lock.read().map_err(|_| "read failed")?;
    let theirs = Arc::clone(&lock);
    let seen = finish(thread::spawn(move || theirs.read().map(|v| *v).unwrap_or(0)), "the second reader")?;
    check(seen == 5, &format!("the second reader saw {seen}; expected 5"))
}

fn rwlock_writer_excludes() -> CaseResult {
    let lock = RwLock::new(0);
    {
        let _writer = lock.write().map_err(|_| "write failed")?;
        let blocked = thread::scope(|s| s.spawn(|| matches!(lock.try_read(), Err(TryLockError::WouldBlock))).join());
        check(matches!(blocked, Ok(true)), "try_read succeeded while a writer held the lock")?;
    }
    let _reader = lock.read().map_err(|_| "read failed")?;
    let blocked = thread::scope(|s| s.spawn(|| matches!(lock.try_write(), Err(TryLockError::WouldBlock))).join());
    check(matches!(blocked, Ok(true)), "try_write succeeded while a reader held the lock")
}

fn condvar_notify_one() -> CaseResult {
    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let theirs = Arc::clone(&pair);
    let handle = thread::spawn(move || {
        let (lock, cvar) = &*theirs;
        let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let _ready = cvar.wait_while(guard, |ready| !*ready);
    });
    thread::sleep(Duration::from_millis(20));
    let (lock, cvar) = &*pair;
    *lock.lock().map_err(|_| "lock failed")? = true;
    cvar.notify_one();
    finish(handle, "the waiting thread")
}

fn condvar_notify_all() -> CaseResult {
    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let handles: Vec<_> = (0..4).map(|_| {
        let theirs = Arc::clone(&pair);
        thread::spawn(move || {
            let (lock, cvar) = &*theirs;
            let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
            let _ready = cvar.wait_while(guard, |ready| !*ready);
        })
    }).collect();
    thread::sleep(Duration::from_millis(20));
    let (lock, cvar) = &*pair;
    *lock.lock().map_err(|_| "lock failed")? = true;
    cvar.notify_all();
    for handle in handles { finish(handle, "a waiting thread")?; }
    Ok(())
}

fn condvar_timeout() -> CaseResult {
    let lock = Mutex::new(());
    let cvar = Condvar::new();
    let guard = lock.lock().map_err(|_| "lock failed")?;
    let want = Duration::from_millis(100);
    wait_for(100, "late");
    let start = Instant::now();
    let (_guard, result) = cvar.wait_timeout_while(guard, want, |_| true).map_err(|_| "wait_timeout_while failed")?;
    let took = start.elapsed();
    check(result.timed_out(), "wait_timeout_while did not report timed_out")?;
    on_time("wait_timeout_while", took, want)
}

fn atomic_fetch_add() -> CaseResult {
    let counter = Arc::new(AtomicU64::new(0));
    let handles: Vec<_> = (0..4).map(|_| {
        let counter = Arc::clone(&counter);
        thread::spawn(move || for _ in 0..100_000 { counter.fetch_add(1, Ordering::Relaxed); })
    }).collect();
    for handle in handles { finish(handle, "an adding thread")?; }
    let total = counter.load(Ordering::SeqCst);
    check(total == 400_000, &format!("the counter is {total}; expected 400000"))
}

fn atomic_cas_lock() -> CaseResult {
    let locked = Arc::new(AtomicBool::new(false));
    let counter = Arc::new(AtomicU64::new(0));
    let handles: Vec<_> = (0..4).map(|_| {
        let (locked, counter) = (Arc::clone(&locked), Arc::clone(&counter));
        thread::spawn(move || for _ in 0..20_000 {
            while locked.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
                std::hint::spin_loop();
            }
            let seen = counter.load(Ordering::Relaxed);
            counter.store(seen + 1, Ordering::Relaxed);
            locked.store(false, Ordering::Release);
        })
    }).collect();
    for handle in handles { finish(handle, "a locking thread")?; }
    let total = counter.load(Ordering::SeqCst);
    check(total == 80_000, &format!("the counter is {total}; expected 80000"))
}

fn mpsc_order() -> CaseResult {
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || for i in 0..1000u32 { let _ = tx.send(i); });
    for want in 0..1000u32 {
        match rx.recv_timeout(Duration::from_millis(WAIT_MS)) {
            Ok(got) if got == want => {}
            Ok(got) => return fail(format!("message {want} was {got}")),
            Err(e) => return fail(format!("message {want} did not arrive: {e}")),
        }
    }
    finish(handle, "the sending thread")
}

fn mpsc_disconnect() -> CaseResult {
    let (tx, rx) = mpsc::channel();
    let handles: Vec<_> = (0..4u32).map(|t| {
        let tx = tx.clone();
        thread::spawn(move || for i in 0..250u32 { let _ = tx.send(t * 1000 + i); })
    }).collect();
    drop(tx);
    let mut got = Vec::new();
    loop {
        match rx.recv_timeout(Duration::from_millis(WAIT_MS)) {
            Ok(n) => got.push(n),
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => return fail(format!("recv timed out after {} messages", got.len())),
        }
    }
    for handle in handles { finish(handle, "a sending thread")?; }
    got.sort();
    let want: Vec<u32> = (0..4u32).flat_map(|t| (0..250).map(move |i| t * 1000 + i)).collect();
    check(got == want, &format!("{} messages arrived; expected each of the 1000 once", got.len()))
}

fn mpsc_recv_timeout() -> CaseResult {
    let (_tx, rx) = mpsc::channel::<u32>();
    let want = Duration::from_millis(50);
    let start = Instant::now();
    let result = rx.recv_timeout(want);
    let took = start.elapsed();
    check(result == Err(RecvTimeoutError::Timeout), &format!("recv_timeout returned {result:?}; expected Timeout"))?;
    on_time("recv_timeout", took, want)
}

fn mpsc_try_recv() -> CaseResult {
    let (tx, rx) = mpsc::channel::<u32>();
    check(rx.try_recv() == Err(TryRecvError::Empty), "try_recv on an empty channel is not Empty")?;
    let handle = thread::spawn(move || drop(tx));
    finish(handle, "the thread dropping the sender")?;
    check(rx.try_recv() == Err(TryRecvError::Disconnected), "try_recv after the sender was dropped is not Disconnected")
}

fn sync_channel_full() -> CaseResult {
    let (tx, rx) = mpsc::sync_channel(1);
    tx.try_send(1).map_err(|e| format!("the first try_send failed: {e}"))?;
    check(matches!(tx.try_send(2), Err(TrySendError::Full(2))), "try_send on a full channel is not Full")?;
    check(rx.recv() == Ok(1), "recv did not return the queued message")?;
    tx.try_send(3).map_err(|e| format!("try_send after recv failed: {e}"))?;
    Ok(())
}

fn sync_channel_rendezvous() -> CaseResult {
    let (tx, rx) = mpsc::sync_channel(0);
    let handle = thread::spawn(move || tx.send(7));
    thread::sleep(Duration::from_millis(50));
    check(!handle.is_finished(), "send on sync_channel(0) returned before anything received")?;
    let got = rx.recv_timeout(Duration::from_millis(WAIT_MS));
    check(got == Ok(7), &format!("recv returned {got:?}; expected Ok(7)"))?;
    finish(handle, "the sending thread")?.map_err(|_| "send failed")?;
    Ok(())
}

fn barrier() -> CaseResult {
    let barrier = Arc::new(Barrier::new(4));
    let arrived = Arc::new(AtomicUsize::new(0));
    let handles: Vec<_> = (0..4).map(|_| {
        let (barrier, arrived) = (Arc::clone(&barrier), Arc::clone(&arrived));
        thread::spawn(move || {
            arrived.fetch_add(1, Ordering::SeqCst);
            let leader = barrier.wait().is_leader();
            (leader, arrived.load(Ordering::SeqCst))
        })
    }).collect();
    let mut leaders = 0;
    for handle in handles {
        let (leader, seen) = finish(handle, "a thread at the barrier")?;
        check(seen == 4, &format!("a thread left the barrier when {seen} of 4 had arrived"))?;
        leaders += leader as usize;
    }
    check(leaders == 1, &format!("{leaders} threads were leader; expected 1"))
}

fn once() -> CaseResult {
    let once = Arc::new(Once::new());
    let runs = Arc::new(AtomicUsize::new(0));
    let handles: Vec<_> = (0..8).map(|_| {
        let (once, runs) = (Arc::clone(&once), Arc::clone(&runs));
        thread::spawn(move || once.call_once(|| { thread::sleep(Duration::from_millis(10)); runs.fetch_add(1, Ordering::SeqCst); }))
    }).collect();
    for handle in handles { finish(handle, "a thread in call_once")?; }
    let n = runs.load(Ordering::SeqCst);
    check(n == 1 && once.is_completed(), &format!("the closure ran {n} times"))
}

fn once_lock() -> CaseResult {
    let cell = Arc::new(OnceLock::new());
    let inits = Arc::new(AtomicUsize::new(0));
    let handles: Vec<_> = (0..8u32).map(|i| {
        let (cell, inits) = (Arc::clone(&cell), Arc::clone(&inits));
        thread::spawn(move || *cell.get_or_init(|| { inits.fetch_add(1, Ordering::SeqCst); 100 + i }))
    }).collect();
    let mut values = Vec::new();
    for handle in handles { values.push(finish(handle, "a thread in get_or_init")?); }
    let n = inits.load(Ordering::SeqCst);
    check(n == 1, &format!("get_or_init ran its closure {n} times"))?;
    check(values.iter().all(|v| Some(v) == cell.get()), &format!("the threads saw {values:?}"))?;
    check(cell.set(5).is_err(), "set on an initialized OnceLock succeeded")
}

thread_local! {
    static LOCAL: Cell<u32> = const { Cell::new(1) };
    static DROPPED: Dropper = const { Dropper };
}

static DTOR_RAN: AtomicBool = AtomicBool::new(false);

struct Dropper;

impl Drop for Dropper {
    fn drop(&mut self) { DTOR_RAN.store(true, Ordering::SeqCst); }
}

fn thread_local() -> CaseResult {
    LOCAL.with(|v| v.set(5));
    let theirs = finish(thread::spawn(|| { let start = LOCAL.with(|v| v.get()); LOCAL.with(|v| v.set(9)); start }), "the thread")?;
    check(theirs == 1, &format!("the thread's value started at {theirs}; expected 1"))?;
    let ours = LOCAL.with(|v| v.get());
    check(ours == 5, &format!("the main thread's value is {ours} after the thread set its own; expected 5"))
}

fn thread_local_dtor() -> CaseResult {
    DTOR_RAN.store(false, Ordering::SeqCst);
    finish(thread::spawn(|| DROPPED.with(|_| ())), "the thread")?;
    check(DTOR_RAN.load(Ordering::SeqCst), "the thread-local destructor had not run when join returned")
}

fn park_token() -> CaseResult {
    thread::current().unpark();
    let start = Instant::now();
    thread::park();
    let took = start.elapsed();
    check(took < Duration::from_millis(100), &format!("park after unpark took {} ms", took.as_millis()))
}

fn park_unpark() -> CaseResult {
    let go = Arc::new(AtomicBool::new(false));
    let theirs = Arc::clone(&go);
    let handle = thread::spawn(move || while !theirs.load(Ordering::SeqCst) { thread::park(); });
    thread::sleep(Duration::from_millis(30));
    go.store(true, Ordering::SeqCst);
    handle.thread().unpark();
    finish(handle, "the parked thread")
}

fn park_timeout() -> CaseResult {
    let want = Duration::from_millis(50);
    let start = Instant::now();
    thread::park_timeout(want);
    let took = start.elapsed();
    value("took", took.as_micros() as i64, "us", Some((0, (ms(want) + LATE_MS) as i64 * 1000)));
    check(took <= want + Duration::from_millis(LATE_MS as u64),
        &format!("park_timeout(50 ms) returned after {} us", took.as_micros()))
}

fn available_parallelism() -> CaseResult {
    let n = thread::available_parallelism().ctx("available_parallelism")?.get();
    let listed = processors()?;
    value("processors", n as i64, "", Some((listed as i64, listed as i64)));
    check(n == listed, &format!("available_parallelism is {n}; /proc/cpuinfo lists {listed}"))
}

unsafe extern "C" {
    fn sched_setaffinity(pid: i32, len: usize, mask: *const u64) -> i32;
}

/// Narrow a thread's affinity below the online count, then restore it: available_parallelism
/// follows the affinity both ways, which the /proc/cpuinfo fallback alone cannot do.
fn available_parallelism_affinity() -> CaseResult {
    let listed = processors()?;
    let narrowed = (listed - 1).max(1);
    // A thread of its own, so the narrowed affinity ends with it.
    let handle = thread::spawn(move || -> CaseResult {
        for (cpus, how) in [(narrowed, "narrowed"), (listed, "restored")] {
            let mut mask = [0u64; 16];
            for cpu in 0..cpus {
                mask[cpu / 64] |= 1 << (cpu % 64);
            }
            if unsafe { sched_setaffinity(0, size_of_val(&mask), mask.as_ptr()) } != 0 {
                return fail(format!("sched_setaffinity to {cpus} processors failed: {}", io::Error::last_os_error()));
            }
            let n = thread::available_parallelism().ctx("available_parallelism")?.get();
            check(n == cpus, &format!("affinity {how} to {cpus} of {listed} processors; available_parallelism is {n}"))?;
        }
        Ok(())
    });
    finish(handle, "the affinity thread")?
}

fn detached() -> CaseResult {
    let (tx, rx) = mpsc::channel();
    drop(thread::spawn(move || { thread::sleep(Duration::from_millis(20)); let _ = tx.send(11); }));
    let got = rx.recv_timeout(Duration::from_millis(WAIT_MS));
    check(got == Ok(11), &format!("the detached thread's message is {got:?}; expected Ok(11)"))
}

// --- net --------------------------------------------------------------------------------

/// A TCP or UDP port for this case's process, 0-7: 20000-29999, numbered from its process ID.
fn port(n: u16) -> u16 { 20000 + (std::process::id() % 1250) as u16 * 8 + n }

fn local(n: u16) -> SocketAddr { SocketAddr::from((Ipv4Addr::LOCALHOST, port(n))) }

/// A listener on `local(0)` and a client connected to it, with the accepted stream.
/// Both streams time reads out after 2 seconds.
fn tcp_pair() -> Result<(TcpListener, TcpStream, TcpStream), CaseError> {
    let listener = TcpListener::bind(local(0)).ctx("TcpListener::bind")?;
    let client = TcpStream::connect(local(0)).ctx("TcpStream::connect")?;
    let (server, _) = listener.accept().ctx("accept")?;
    for stream in [&client, &server] {
        stream.set_read_timeout(Some(Duration::from_secs(2))).ctx("set_read_timeout")?;
    }
    Ok((listener, client, server))
}

fn tcp_bind() -> CaseResult {
    let listener = TcpListener::bind(local(0)).ctx("TcpListener::bind")?;
    let addr = listener.local_addr().ctx("local_addr")?;
    check(addr == local(0), &format!("local_addr is {addr}; expected {}", local(0)))
}

fn tcp_bind_port_zero() -> CaseResult {
    let listener = TcpListener::bind("127.0.0.1:0").ctx("TcpListener::bind")?;
    let addr = listener.local_addr().ctx("local_addr")?;
    check(addr.port() != 0 && addr.ip() == Ipv4Addr::LOCALHOST, &format!("local_addr is {addr}"))
}

fn tcp_bind_in_use() -> CaseResult {
    let _first = TcpListener::bind(local(0)).ctx("TcpListener::bind")?;
    want_kind("a second bind", TcpListener::bind(local(0)), ErrorKind::AddrInUse)
}

fn tcp_accept_addrs() -> CaseResult {
    let listener = TcpListener::bind(local(0)).ctx("TcpListener::bind")?;
    let client = TcpStream::connect(local(0)).ctx("TcpStream::connect")?;
    let (server, peer) = listener.accept().ctx("accept")?;
    let client_local = client.local_addr().ctx("client local_addr")?;
    check(peer == client_local, &format!("accept's peer is {peer}; the client's local_addr is {client_local}"))?;
    let client_peer = client.peer_addr().ctx("client peer_addr")?;
    check(client_peer == local(0), &format!("the client's peer_addr is {client_peer}; expected {}", local(0)))?;
    let server_local = server.local_addr().ctx("server local_addr")?;
    check(server_local == local(0), &format!("the accepted stream's local_addr is {server_local}"))
}

fn tcp_echo() -> CaseResult {
    let (_listener, mut client, mut server) = tcp_pair()?;
    let bytes = pattern(1000);
    client.write_all(&bytes).ctx("client write_all")?;
    let mut buf = vec![0u8; 1000];
    server.read_exact(&mut buf).ctx("server read_exact")?;
    server.write_all(&buf).ctx("server write_all")?;
    let mut back = vec![0u8; 1000];
    client.read_exact(&mut back).ctx("client read_exact")?;
    check(back == bytes, "the bytes that came back differ from those sent")
}

fn tcp_large() -> CaseResult {
    let (_listener, mut client, server) = tcp_pair()?;
    let bytes = pattern(1 << 20);
    let sent = bytes.clone();
    let writer = thread::spawn(move || { let mut server = server; server.write_all(&sent) });
    let mut back = Vec::new();
    client.read_to_end(&mut back).ctx("read_to_end")?;
    finish(writer, "the writing thread")?.ctx("write_all")?;
    check(back == bytes, &format!("{} bytes arrived that differ from the {} sent", back.len(), bytes.len()))
}

fn tcp_refused() -> CaseResult {
    want_kind("connect to a port with no listener", TcpStream::connect(local(1)), ErrorKind::ConnectionRefused)
}

fn tcp_read_eof() -> CaseResult {
    let (_listener, mut client, server) = tcp_pair()?;
    drop(server);
    let n = client.read(&mut [0u8; 16]).ctx("read")?;
    check(n == 0, &format!("read returned {n} after the peer closed; expected 0"))
}

fn tcp_shutdown_write() -> CaseResult {
    let (_listener, mut client, mut server) = tcp_pair()?;
    client.shutdown(Shutdown::Write).ctx("shutdown(Write)")?;
    let n = server.read(&mut [0u8; 16]).ctx("peer read")?;
    check(n == 0, &format!("the peer read {n} bytes after shutdown(Write); expected end of file"))?;
    server.write_all(b"still").ctx("peer write_all")?;
    let mut buf = [0u8; 5];
    client.read_exact(&mut buf).ctx("read after shutdown(Write)")?;
    check(&buf == b"still", "the stream did not read after shutdown(Write)")
}

fn tcp_shutdown_read() -> CaseResult {
    let (_listener, mut client, _server) = tcp_pair()?;
    client.shutdown(Shutdown::Read).ctx("shutdown(Read)")?;
    let n = client.read(&mut [0u8; 16]).ctx("read after shutdown(Read)")?;
    check(n == 0, &format!("read returned {n}; expected 0"))
}

fn tcp_read_timeout() -> CaseResult {
    let (_listener, mut client, _server) = tcp_pair()?;
    let want = Duration::from_millis(100);
    client.set_read_timeout(Some(want)).ctx("set_read_timeout")?;
    wait_for(100, "late");
    let start = Instant::now();
    let result = client.read(&mut [0u8; 16]);
    let took = start.elapsed();
    want_kind("read with no data", result, ErrorKind::WouldBlock)?;
    on_time("the timed-out read", took, want)
}

fn tcp_timeout_values() -> CaseResult {
    let (_listener, client, _server) = tcp_pair()?;
    client.set_read_timeout(Some(Duration::from_millis(200))).ctx("set_read_timeout")?;
    client.set_write_timeout(Some(Duration::from_millis(1500))).ctx("set_write_timeout")?;
    let read = client.read_timeout().ctx("read_timeout")?;
    let write = client.write_timeout().ctx("write_timeout")?;
    check(read == Some(Duration::from_millis(200)) && write == Some(Duration::from_millis(1500)),
        &format!("read_timeout is {read:?} and write_timeout {write:?}; expected 200 ms and 1.5 s"))?;
    client.set_read_timeout(None).ctx("set_read_timeout(None)")?;
    let read = client.read_timeout().ctx("read_timeout")?;
    check(read.is_none(), &format!("read_timeout is {read:?} after set_read_timeout(None)"))
}

fn tcp_nonblocking_accept() -> CaseResult {
    let listener = TcpListener::bind(local(0)).ctx("TcpListener::bind")?;
    listener.set_nonblocking(true).ctx("set_nonblocking")?;
    want_kind("accept with no client", listener.accept(), ErrorKind::WouldBlock)
}

fn tcp_nonblocking_read() -> CaseResult {
    let (_listener, mut client, _server) = tcp_pair()?;
    client.set_nonblocking(true).ctx("set_nonblocking")?;
    want_kind("read with no data", client.read(&mut [0u8; 16]), ErrorKind::WouldBlock)
}

fn tcp_connect_timeout() -> CaseResult {
    let listener = TcpListener::bind(local(0)).ctx("TcpListener::bind")?;
    let client = TcpStream::connect_timeout(&local(0), Duration::from_secs(2)).ctx("connect_timeout")?;
    let (_server, peer) = listener.accept().ctx("accept")?;
    let mine = client.local_addr().ctx("local_addr")?;
    check(peer == mine, &format!("accept's peer is {peer}; the client is {mine}"))
}

fn tcp_nodelay() -> CaseResult {
    let (_listener, client, _server) = tcp_pair()?;
    client.set_nodelay(true).ctx("set_nodelay(true)")?;
    check(client.nodelay().ctx("nodelay")?, "nodelay is false after set_nodelay(true)")?;
    client.set_nodelay(false).ctx("set_nodelay(false)")?;
    check(!client.nodelay().ctx("nodelay")?, "nodelay is true after set_nodelay(false)")
}

fn tcp_ttl() -> CaseResult {
    let (_listener, client, _server) = tcp_pair()?;
    client.set_ttl(42).ctx("set_ttl")?;
    let ttl = client.ttl().ctx("ttl")?;
    check(ttl == 42, &format!("ttl is {ttl}; expected 42"))
}

fn tcp_peek() -> CaseResult {
    let (_listener, mut client, mut server) = tcp_pair()?;
    server.write_all(b"peek").ctx("write_all")?;
    let mut buf = [0u8; 4];
    let n = client.peek(&mut buf).ctx("peek")?;
    check(n > 0 && buf[..n] == b"peek"[..n], &format!("peek returned {:?}", String::from_utf8_lossy(&buf[..n])))?;
    client.read_exact(&mut buf).ctx("read_exact after peek")?;
    check(&buf == b"peek", "read after peek did not return the peeked bytes")
}

fn tcp_write_closed() -> CaseResult {
    let (_listener, mut client, server) = tcp_pair()?;
    drop(server);
    let start = Instant::now();
    loop {
        match client.write(&[0u8; 1024]) {
            Ok(_) if start.elapsed() < Duration::from_secs(2) => thread::sleep(Duration::from_millis(10)),
            Ok(_) => return fail("writes still succeeded 2 s after the peer closed"),
            Err(e) if matches!(e.kind(), ErrorKind::BrokenPipe | ErrorKind::ConnectionReset) => return Ok(()),
            Err(e) => return fail(format!("write failed with {e} ({:?}); expected BrokenPipe or ConnectionReset", e.kind())),
        }
    }
}

/// A UDP socket on `local(n)` that times reads out after 2 seconds.
fn udp(n: u16) -> Result<UdpSocket, CaseError> {
    let socket = UdpSocket::bind(local(n)).ctx("UdpSocket::bind")?;
    socket.set_read_timeout(Some(Duration::from_secs(2))).ctx("set_read_timeout")?;
    Ok(socket)
}

fn udp_send_recv() -> CaseResult {
    let (a, b) = (udp(0)?, udp(1)?);
    let n = a.send_to(b"hello", local(1)).ctx("send_to")?;
    check(n == 5, &format!("send_to returned {n}; expected 5"))?;
    let mut buf = [0u8; 16];
    let (n, from) = b.recv_from(&mut buf).ctx("recv_from")?;
    check(&buf[..n] == b"hello", &format!("recv_from returned {:?}", String::from_utf8_lossy(&buf[..n])))?;
    check(from == local(0), &format!("the datagram came from {from}; expected {}", local(0)))
}

fn udp_connected() -> CaseResult {
    let (a, b) = (udp(0)?, udp(1)?);
    a.connect(local(1)).ctx("connect")?;
    b.connect(local(0)).ctx("connect")?;
    a.send(b"one").ctx("send")?;
    let mut buf = [0u8; 16];
    let n = b.recv(&mut buf).ctx("recv")?;
    check(&buf[..n] == b"one", "recv did not return the datagram sent")?;
    let peer = b.peer_addr().ctx("peer_addr")?;
    check(peer == local(0), &format!("peer_addr is {peer}; expected {}", local(0)))
}

fn udp_connected_filter() -> CaseResult {
    let (a, b, other) = (udp(0)?, udp(1)?, udp(2)?);
    b.connect(local(0)).ctx("connect")?;
    other.send_to(b"stranger", local(1)).ctx("send_to from another socket")?;
    a.send_to(b"peer", local(1)).ctx("send_to from the peer")?;
    let mut buf = [0u8; 16];
    let n = b.recv(&mut buf).ctx("recv")?;
    check(&buf[..n] == b"peer", &format!("the connected socket received {:?}; expected only the peer's datagram",
        String::from_utf8_lossy(&buf[..n])))
}

fn udp_not_connected() -> CaseResult {
    let socket = udp(0)?;
    want_kind("peer_addr of an unconnected socket", socket.peer_addr(), ErrorKind::NotConnected)
}

fn udp_truncate() -> CaseResult {
    let (a, b) = (udp(0)?, udp(1)?);
    a.send_to(b"12345678", local(1)).ctx("send_to")?;
    a.send_to(b"next", local(1)).ctx("send_to")?;
    let mut small = [0u8; 4];
    let n = b.recv(&mut small).ctx("recv into 4 bytes")?;
    check(n == 4 && &small == b"1234", &format!("recv returned {n} bytes {:?}", String::from_utf8_lossy(&small[..n])))?;
    let mut buf = [0u8; 16];
    let n = b.recv(&mut buf).ctx("the next recv")?;
    check(&buf[..n] == b"next", &format!("the next recv returned {:?}; the rest of the first datagram was not discarded",
        String::from_utf8_lossy(&buf[..n])))
}

fn udp_read_timeout() -> CaseResult {
    let socket = udp(0)?;
    let want = Duration::from_millis(100);
    socket.set_read_timeout(Some(want)).ctx("set_read_timeout")?;
    wait_for(100, "late");
    let start = Instant::now();
    let result = socket.recv_from(&mut [0u8; 16]);
    let took = start.elapsed();
    want_kind("recv_from with nothing queued", result, ErrorKind::WouldBlock)?;
    on_time("the timed-out recv_from", took, want)
}

fn udp_nonblocking() -> CaseResult {
    let socket = udp(0)?;
    socket.set_nonblocking(true).ctx("set_nonblocking")?;
    want_kind("recv with nothing queued", socket.recv(&mut [0u8; 16]), ErrorKind::WouldBlock)
}

fn resolve_localhost() -> CaseResult {
    let addrs: Vec<SocketAddr> = ("localhost", 80).to_socket_addrs().ctx("to_socket_addrs")?.collect();
    let want = SocketAddr::from((Ipv4Addr::LOCALHOST, 80));
    check(addrs.contains(&want), &format!("localhost resolved to {addrs:?}; expected it to include {want}"))
}

fn tcp_bind_localhost() -> CaseResult {
    let listener = TcpListener::bind(("localhost", port(0))).ctx("TcpListener::bind")?;
    let addr = listener.local_addr().ctx("local_addr")?;
    check(addr.ip().is_loopback() && addr.port() == port(0), &format!("local_addr is {addr}"))
}

/// A Unix socket path beside the case's directory, removed before use and when dropped.
struct SockPath(PathBuf);

impl SockPath {
    fn new(n: u32) -> SockPath {
        let path = PathBuf::from(format!("/tmp/rust-std-{}-{n}.sock", std::process::id()));
        let _ = fs::remove_file(&path);
        SockPath(path)
    }
}

impl Drop for SockPath {
    fn drop(&mut self) { let _ = fs::remove_file(&self.0); }
}

fn unix_pair() -> CaseResult {
    let (mut a, mut b) = UnixStream::pair().ctx("UnixStream::pair")?;
    b.set_read_timeout(Some(Duration::from_secs(2))).ctx("set_read_timeout")?;
    a.set_read_timeout(Some(Duration::from_secs(2))).ctx("set_read_timeout")?;
    a.write_all(b"ping").ctx("write_all")?;
    let mut buf = [0u8; 4];
    b.read_exact(&mut buf).ctx("read_exact")?;
    check(&buf == b"ping", "the bytes read differ from those written")?;
    b.write_all(b"pong").ctx("write_all")?;
    a.read_exact(&mut buf).ctx("read_exact")?;
    check(&buf == b"pong", "the bytes read back differ from those written")
}

fn unix_listener() -> CaseResult {
    let path = SockPath::new(0);
    let listener = UnixListener::bind(&path.0).ctx("UnixListener::bind")?;
    let local = listener.local_addr().ctx("local_addr")?;
    check(local.as_pathname() == Some(path.0.as_path()), &format!("local_addr is {local:?}; expected {:?}", path.0))?;
    let mut client = UnixStream::connect(&path.0).ctx("UnixStream::connect")?;
    let (mut server, _) = listener.accept().ctx("accept")?;
    server.set_read_timeout(Some(Duration::from_secs(2))).ctx("set_read_timeout")?;
    client.write_all(b"over unix").ctx("write_all")?;
    let mut buf = [0u8; 9];
    server.read_exact(&mut buf).ctx("read_exact")?;
    check(&buf == b"over unix", "the bytes read differ from those written")
}

fn unix_bind_in_use() -> CaseResult {
    let path = SockPath::new(0);
    let _first = UnixListener::bind(&path.0).ctx("UnixListener::bind")?;
    want_kind("a second bind to the path", UnixListener::bind(&path.0), ErrorKind::AddrInUse)
}

fn unix_connect_missing() -> CaseResult {
    let path = SockPath::new(0);
    want_kind("connect to a missing path", UnixStream::connect(&path.0), ErrorKind::NotFound)
}

fn unix_datagram() -> CaseResult {
    let (pa, pb) = (SockPath::new(0), SockPath::new(1));
    let a = UnixDatagram::bind(&pa.0).ctx("UnixDatagram::bind")?;
    let b = UnixDatagram::bind(&pb.0).ctx("UnixDatagram::bind")?;
    b.set_read_timeout(Some(Duration::from_secs(2))).ctx("set_read_timeout")?;
    a.send_to(b"note", &pb.0).ctx("send_to")?;
    let mut buf = [0u8; 16];
    let (n, from) = b.recv_from(&mut buf).ctx("recv_from")?;
    check(&buf[..n] == b"note", "recv_from did not return the datagram sent")?;
    check(from.as_pathname() == Some(pa.0.as_path()), &format!("the datagram came from {from:?}; expected {:?}", pa.0))
}

fn unix_datagram_pair() -> CaseResult {
    let (a, b) = UnixDatagram::pair().ctx("UnixDatagram::pair")?;
    b.set_read_timeout(Some(Duration::from_secs(2))).ctx("set_read_timeout")?;
    a.send(b"ab").ctx("send")?;
    a.send(b"cde").ctx("send")?;
    let mut buf = [0u8; 16];
    let first = b.recv(&mut buf).ctx("recv")?;
    let first = buf[..first].to_vec();
    let second = b.recv(&mut buf).ctx("recv")?;
    check(first == b"ab" && &buf[..second] == b"cde", &format!("recv returned {:?} then {:?}; expected \"ab\" then \"cde\"",
        String::from_utf8_lossy(&first), String::from_utf8_lossy(&buf[..second])))
}

fn unix_shutdown() -> CaseResult {
    let (a, mut b) = UnixStream::pair().ctx("UnixStream::pair")?;
    b.set_read_timeout(Some(Duration::from_secs(2))).ctx("set_read_timeout")?;
    a.shutdown(Shutdown::Write).ctx("shutdown(Write)")?;
    let n = b.read(&mut [0u8; 16]).ctx("peer read")?;
    check(n == 0, &format!("the peer read {n} bytes; expected end of file"))
}

// --- time-env ---------------------------------------------------------------------------

fn instant_monotonic() -> CaseResult {
    let start = Instant::now();
    let mut last = start;
    let mut reads = 0u64;
    while reads < 100_000 && start.elapsed() < Duration::from_secs(2) {
        let now = Instant::now();
        if now < last {
            return fail(format!("Instant::now went back {} ns after {reads} reads", (last - now).as_nanos()));
        }
        last = now;
        reads += 1;
    }
    value("reads", reads as i64, "", None);
    Ok(())
}

fn instant_elapsed() -> CaseResult {
    let start = Instant::now();
    thread::sleep(Duration::from_millis(50));
    let took = start.elapsed();
    check(took >= Duration::from_millis(50), &format!("elapsed is {} us after a 50 ms sleep", took.as_micros()))
}

fn instant_threads() -> CaseResult {
    let (tx, rx) = mpsc::channel::<Instant>();
    let (back_tx, back_rx) = mpsc::channel::<Option<u128>>();
    let handle = thread::spawn(move || {
        for sent in rx {
            let now = Instant::now();
            let _ = back_tx.send((now < sent).then(|| (sent - now).as_nanos()));
        }
    });
    for i in 0..1000 {
        tx.send(Instant::now()).map_err(|_| "send failed")?;
        match back_rx.recv_timeout(Duration::from_millis(WAIT_MS)) {
            Ok(None) => {}
            Ok(Some(ns)) => return fail(format!("handoff {i}: the other thread read an Instant {ns} ns earlier")),
            Err(e) => return fail(format!("handoff {i}: no answer: {e}")),
        }
    }
    drop(tx);
    finish(handle, "the reading thread")
}

fn system_time_epoch() -> CaseResult {
    let since = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| format!("SystemTime::now is before UNIX_EPOCH by {:?}", e.duration()))?;
    check(since.as_secs() > 1_704_067_200, &format!("SystemTime::now is {} s after the epoch, before 2024-01-01", since.as_secs()))
}

fn system_time_rate() -> CaseResult {
    let (wall, mono) = (SystemTime::now(), Instant::now());
    wait_for(200, "apart");
    thread::sleep(Duration::from_millis(200));
    let wall = wall.elapsed().map_err(|e| format!("SystemTime went back {:?}", e.duration()))?;
    let mono = mono.elapsed();
    let apart = wall.as_micros() as i64 - mono.as_micros() as i64;
    value("apart", apart, "us", Some((-20_000, 20_000)));
    check(apart.abs() <= 20_000, &format!("SystemTime advanced {} us and Instant {} us", wall.as_micros(), mono.as_micros()))
}

fn system_time_earlier() -> CaseResult {
    let earlier = SystemTime::now();
    thread::sleep(Duration::from_millis(10));
    let later = SystemTime::now();
    let forward = later.duration_since(earlier).map_err(|_| "SystemTime went backwards across a 10 ms sleep")?;
    match earlier.duration_since(later) {
        Ok(d) => fail(format!("duration_since a later time returned Ok({d:?})")),
        Err(e) => check(e.duration() == forward, &format!("the error holds {:?}; expected {forward:?}", e.duration())),
    }
}

fn sleep_never_early() -> CaseResult {
    let want = Duration::from_millis(7);
    for i in 0..10 {
        let start = Instant::now();
        thread::sleep(want);
        let took = start.elapsed();
        check(took >= want, &format!("sleep {i} of 7 ms ended after {} us", took.as_micros()))?;
    }
    Ok(())
}

fn sleep_late_bound() -> CaseResult {
    let want = Duration::from_millis(100);
    wait_for(100, "late");
    let start = Instant::now();
    thread::sleep(want);
    on_time("thread::sleep", start.elapsed(), want)
}

fn sleep_zero() -> CaseResult {
    let start = Instant::now();
    thread::sleep(Duration::ZERO);
    let took = start.elapsed();
    check(took <= Duration::from_millis(10), &format!("sleep(ZERO) took {} us", took.as_micros()))
}

fn sleep_short() -> CaseResult {
    let want = Duration::from_millis(1);
    let mut late = Vec::new();
    for i in 0..20 {
        let start = Instant::now();
        thread::sleep(want);
        let took = start.elapsed();
        check(took >= want, &format!("sleep {i} of 1 ms ended after {} us", took.as_micros()))?;
        late.push((took - want).as_micros() as i64);
    }
    late.sort();
    let median = late[10];
    value("median-late", median, "us", Some((0, 6000)));
    check(median <= 6000, &format!("the median 1 ms sleep ended {median} us late"))
}

fn args_self() -> CaseResult {
    let first = std::env::args().next();
    check(first.as_deref().is_some_and(|arg| arg.ends_with("suite-rust-std")),
        &format!("env::args begins with {first:?}"))
}

fn args_child() -> CaseResult {
    spawning!({
        let out = run_ok(&mut helper(&["allargs", "x", "y z"]))?;
        let want = format!("{CHILD}\nallargs\nx\ny z\n");
        check(out == want, &format!("the program's env::args are {out:?}"))
    })
}

fn var_set_get() -> CaseResult {
    std::env::set_var("RUST_STD_VAR", "value");
    let got = std::env::var("RUST_STD_VAR");
    check(got.as_deref() == Ok("value"), &format!("env::var returned {got:?}"))
}

fn var_missing() -> CaseResult {
    let got = std::env::var("RUST_STD_NEVER_SET");
    check(got == Err(std::env::VarError::NotPresent), &format!("env::var returned {got:?}; expected NotPresent"))
}

fn var_not_unicode() -> CaseResult {
    let bytes = std::ffi::OsStr::from_bytes(b"caf\xe9");
    std::env::set_var("RUST_STD_BYTES", bytes);
    let got = std::env::var("RUST_STD_BYTES");
    check(matches!(got, Err(std::env::VarError::NotUnicode(_))), &format!("env::var returned {got:?}; expected NotUnicode"))?;
    let os = std::env::var_os("RUST_STD_BYTES");
    check(os.as_deref() == Some(bytes), &format!("var_os returned {os:?}"))
}

fn var_empty() -> CaseResult {
    std::env::set_var("RUST_STD_EMPTY", "");
    let got = std::env::var("RUST_STD_EMPTY");
    check(got.as_deref() == Ok(""), &format!("env::var returned {got:?}; expected Ok(\"\")"))
}

/// Set `name` to `value` and fail unless env::var then returns it, so a case that goes on
/// to remove or hide the variable cannot pass because it was never set.
fn set_and_confirm(name: &str, value: &str) -> CaseResult {
    std::env::set_var(name, value);
    let got = std::env::var(name);
    check(got.as_deref() == Ok(value), &format!("env::set_var did not set {name}: env::var returned {got:?}"))
}

/// Remove `name` and fail unless it is then unset, so a case that measures behaviour
/// without the variable is not measuring an inherited value.
fn remove_and_confirm(name: &str) -> CaseResult {
    std::env::remove_var(name);
    let left = std::env::var_os(name);
    check(left.is_none(), &format!("env::remove_var left {name} set to {left:?}"))
}

fn remove_var() -> CaseResult {
    set_and_confirm("RUST_STD_GONE", "x")?;
    std::env::remove_var("RUST_STD_GONE");
    let got = std::env::var("RUST_STD_GONE");
    check(got == Err(std::env::VarError::NotPresent), &format!("env::var after remove_var returned {got:?}"))?;
    check(!std::env::vars_os().any(|(name, _)| name == "RUST_STD_GONE"), "vars_os still lists the removed variable")
}

fn vars_lists() -> CaseResult {
    std::env::set_var("RUST_STD_LISTED", "here");
    let found: Vec<String> = std::env::vars().filter(|(name, _)| name == "RUST_STD_LISTED").map(|(_, value)| value).collect();
    check(found == ["here"], &format!("vars lists the variable with values {found:?}"))
}

fn set_var_overwrite() -> CaseResult {
    std::env::set_var("RUST_STD_TWICE", "first");
    std::env::set_var("RUST_STD_TWICE", "second");
    let got = std::env::var("RUST_STD_TWICE");
    check(got.as_deref() == Ok("second"), &format!("env::var returned {got:?}"))
}

fn current_dir_absolute() -> CaseResult {
    let dir = std::env::current_dir().ctx("current_dir")?;
    check(dir.is_absolute(), &format!("current_dir is {dir:?}"))?;
    fs::read_dir(&dir).ctx("read_dir of the current directory").map(drop)
}

fn set_current_dir() -> CaseResult {
    let dir = Scratch::new()?;
    std::env::set_current_dir(&dir.0).ctx("set_current_dir")?;
    let now = std::env::current_dir().ctx("current_dir")?;
    check(now == dir.0, &format!("current_dir is {now:?}; expected {:?}", dir.0))
}

fn set_current_dir_dotdot() -> CaseResult {
    let dir = Scratch::new()?;
    fs::create_dir_all(dir.path("a/b")).ctx("create_dir_all")?;
    std::env::set_current_dir(dir.path("a/b")).ctx("set_current_dir")?;
    std::env::set_current_dir("..").ctx("set_current_dir(\"..\")")?;
    let now = std::env::current_dir().ctx("current_dir")?;
    check(now == dir.path("a"), &format!("current_dir is {now:?}; expected {:?}", dir.path("a")))
}

fn set_current_dir_missing() -> CaseResult {
    let dir = Scratch::new()?;
    want_kind("set_current_dir to a missing path", std::env::set_current_dir(dir.path("missing")), ErrorKind::NotFound)
}

fn set_current_dir_file() -> CaseResult {
    let dir = Scratch::new()?;
    write_file(&dir.path("file"), b"x")?;
    want_kind("set_current_dir to a file", std::env::set_current_dir(dir.path("file")), ErrorKind::NotADirectory)
}

fn relative_path() -> CaseResult {
    let dir = Scratch::new()?;
    std::env::set_current_dir(&dir.0).ctx("set_current_dir")?;
    write_file(Path::new("relative.txt"), b"here")?;
    let back = fs::read(dir.path("relative.txt")).ctx("fs::read by absolute path")?;
    check(back == b"here", "the file named relative to the current directory is not in it")
}

fn temp_dir_default() -> CaseResult {
    remove_and_confirm("TMPDIR")?;
    let dir = std::env::temp_dir();
    check(dir == Path::new("/tmp"), &format!("temp_dir is {dir:?}; expected \"/tmp\""))
}

fn temp_dir_tmpdir() -> CaseResult {
    std::env::set_var("TMPDIR", "/tmp/rust-std-tmpdir");
    let dir = std::env::temp_dir();
    check(dir == Path::new("/tmp/rust-std-tmpdir"), &format!("temp_dir is {dir:?}; expected \"/tmp/rust-std-tmpdir\""))
}

fn home_dir_env() -> CaseResult {
    std::env::set_var("HOME", "/rust-std-home");
    let home = std::env::home_dir();
    check(home.as_deref() == Some(Path::new("/rust-std-home")), &format!("home_dir is {home:?}; expected \"/rust-std-home\""))
}

fn home_dir_passwd() -> CaseResult {
    let dir = Scratch::new()?;
    write_file(&dir.path("owner"), b"x")?;
    let uid = fs::metadata(dir.path("owner")).ctx("metadata")?.uid();
    let passwd = fs::read_to_string("/etc/passwd").ctx("reading /etc/passwd")?;
    let want = passwd.lines().map(|line| line.split(':').collect::<Vec<_>>())
        .find(|fields| fields.len() >= 7 && fields[2].parse() == Ok(uid))
        .map(|fields| PathBuf::from(fields[5]))
        .ok_or_else(|| format!("/etc/passwd has no entry for uid {uid}"))?;
    remove_and_confirm("HOME")?;
    let home = std::env::home_dir();
    check(home.as_ref() == Some(&want), &format!("home_dir is {home:?}; /etc/passwd gives uid {uid} {want:?}"))
}

fn current_exe() -> CaseResult {
    let exe = std::env::current_exe().ctx("current_exe")?;
    check(exe == Path::new("/sbin/suite-rust-std"), &format!("current_exe is {exe:?}"))
}

fn current_exe_child() -> CaseResult {
    spawning!({
        let out = run_ok(&mut helper(&["exe"]))?;
        check(out == format!("{CHILD}\n"), &format!("the program's current_exe is {out:?}"))
    })
}

fn current_exe_non_utf8() -> CaseResult {
    spawning!({
        let program = std::ffi::OsStr::from_bytes(NON_UTF8_CHILD);
        let out = run(Command::new(program).arg("exe-open"))?;
        let want = [NON_UTF8_CHILD, b"\n"].concat();
        check(out.status.success() && out.stdout == want,
            &format!("the program at {program:?} ended with {} and wrote {:?}", out.status, String::from_utf8_lossy(&out.stdout)))
    })
}
