//! Allocation-free parsing and checks for the dynamic process entries in procfs.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Pid {
    Caller,
    Number(u64),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Entry {
    Directory,
    Status,
    Exe,
    Stat,
    Cmdline,
}

pub(super) fn parse(path: &str) -> Option<(Pid, Entry)> {
    let relative = path.strip_prefix("/proc/")?.trim_end_matches('/');
    let (pid, tail) = relative.split_once('/').unwrap_or((relative, ""));
    let pid = if pid == "self" {
        Pid::Caller
    } else {
        if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit())
            || (pid.len() > 1 && pid.starts_with('0')) {
            return None;
        }
        Pid::Number(pid.parse().ok()?)
    };
    let entry = match tail {
        "" => Entry::Directory,
        "status" => Entry::Status,
        "exe" => Entry::Exe,
        "stat" => Entry::Stat,
        "cmdline" => Entry::Cmdline,
        _ => return None,
    };
    Some((pid, entry))
}

/// Whether a caller with effective IDs `caller` (uid, gid) may read a
/// process's executable link: root, or a caller whose user matches the
/// process's real, effective and saved user IDs and whose group matches all
/// three of its group IDs.
pub(super) fn may_read(caller: (u32, u32), uids: [u32; 3], gids: [u32; 3]) -> bool {
    caller.0 == 0 || (uids.iter().all(|&id| id == caller.0) && gids.iter().all(|&id| id == caller.1))
}

/// The name /proc/<pid>/status reports: the program's final pathname
/// component, cut to Linux's 15-byte command name on a character boundary.
pub(super) fn command_name(name: &str) -> &str {
    let base = name.rsplit('/').next().unwrap_or(name);
    let mut end = base.len().min(15);
    while !base.is_char_boundary(end) {
        end -= 1;
    }
    &base[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_and_caller_paths() {
        assert_eq!(parse("/proc/self/exe"), Some((Pid::Caller, Entry::Exe)));
        assert_eq!(parse("/proc/self/status"), Some((Pid::Caller, Entry::Status)));
        assert_eq!(parse("/proc/self"), Some((Pid::Caller, Entry::Directory)));
        assert_eq!(parse("/proc/123/exe"), Some((Pid::Number(123), Entry::Exe)));
        assert_eq!(parse("/proc/123/stat"), Some((Pid::Number(123), Entry::Stat)));
        assert_eq!(parse("/proc/self/cmdline"), Some((Pid::Caller, Entry::Cmdline)));
        assert_eq!(parse("/proc/123/"), Some((Pid::Number(123), Entry::Directory)));
        assert_eq!(parse("/proc/18446744073709551615/status"), Some((Pid::Number(u64::MAX), Entry::Status)));
    }

    #[test]
    fn reject_malformed_process_paths() {
        for path in ["/proc/", "/proc//exe", "/proc/+1/exe", "/proc/-1/exe",
            "/proc/01/exe", "/proc/1x/exe", "/proc/18446744073709551616/exe",
            "/proc/1/exe/child", "/proc/1/unknown", "/proc/selfish/exe",
            "/proc/self//exe", "proc/1/exe", "/process/1/exe", "/proc/1/stat/x",
            "/proc/1/statm"] {
            assert_eq!(parse(path), None, "{path}");
        }
    }

    #[test]
    fn executable_link_needs_matching_ids_or_root() {
        assert!(may_read((0, 7), [5, 5, 5], [6, 6, 6]));
        assert!(may_read((5, 6), [5, 5, 5], [6, 6, 6]));
        assert!(!may_read((5, 6), [5, 0, 5], [6, 6, 6]));
        assert!(!may_read((5, 6), [5, 5, 0], [6, 6, 6]));
        assert!(!may_read((5, 6), [5, 5, 5], [6, 6, 0]));
        assert!(!may_read((5, 6), [0, 0, 0], [0, 0, 0]));
    }

    #[test]
    fn command_name_is_a_short_basename() {
        assert_eq!(command_name("/bin/true"), "true");
        assert_eq!(command_name("bsh"), "bsh");
        assert_eq!(command_name("/usr/local/cbin/a_rather_long_program"), "a_rather_long_p");
        assert_eq!(command_name("/bin/\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}"), "\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}");
    }
}
