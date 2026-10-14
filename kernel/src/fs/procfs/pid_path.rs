//! Allocation-free parsing of the dynamic process entries in procfs.
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
        _ => return None,
    };
    Some((pid, entry))
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
        assert_eq!(parse("/proc/123/"), Some((Pid::Number(123), Entry::Directory)));
        assert_eq!(parse("/proc/18446744073709551615/status"), Some((Pid::Number(u64::MAX), Entry::Status)));
    }

    #[test]
    fn reject_malformed_process_paths() {
        for path in ["/proc/", "/proc//exe", "/proc/+1/exe", "/proc/-1/exe",
            "/proc/01/exe", "/proc/1x/exe", "/proc/18446744073709551616/exe",
            "/proc/1/exe/child", "/proc/1/unknown", "/proc/selfish/exe",
            "/proc/self//exe", "proc/1/exe", "/process/1/exe"] {
            assert_eq!(parse(path), None, "{path}");
        }
    }
}
