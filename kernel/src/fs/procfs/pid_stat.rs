//! Allocation-free formatting of `/proc/<pid>/stat` in Linux's field layout
//! (proc_pid_stat(5)): 52 space-separated fields on one line.

/// Clock ticks per second of the time fields, Linux's USER_HZ.
pub(super) const USER_HZ: u64 = 100;

/// What a process's stat line reports. Fields Breenix does not track are
/// written as 0, as Linux writes the fields it hides.
pub(super) struct PidStat<'a> {
    pub pid: u64,
    pub comm: &'a str,
    pub state: u8,
    pub ppid: u64,
    pub pgrp: u64,
    pub session: u64,
    pub tty_nr: u64,
    /// Foreground process group of the controlling terminal, or -1.
    pub tpgid: i64,
    pub utime_ns: u64,
    pub stime_ns: u64,
    pub cutime_ns: u64,
    pub cstime_ns: u64,
    pub nice: i8,
    pub num_threads: u64,
    /// Milliseconds after boot at which the process started.
    pub start_ms: u64,
    pub vsize: u64,
    pub rss_pages: u64,
    pub exit_code: i32,
}

/// Linux's encoding of a device number in `tty_nr`: minor bits 0-7 and
/// 20-31, major bits 8-19.
pub(super) fn tty_nr(major: u32, minor: u32) -> u64 {
    let (major, minor) = (major as u64, minor as u64);
    (minor & 0xff) | ((major & 0xfff) << 8) | ((minor & !0xff) << 12)
}

/// The status wait reports for a process that ended with `code`: a negative
/// code is the signal that ended it.
pub(super) fn exit_status(code: i32) -> i32 {
    if code < 0 {
        -code & 0xff
    } else {
        (code & 0xff) << 8
    }
}

fn ticks(ns: u64) -> u64 {
    ns / (1_000_000_000 / USER_HZ)
}

/// The most bytes `write` produces: 52 fields of at most 20 digits and a
/// sign, their separators, and the command name and its parentheses.
pub(super) const MAX_LEN: usize = 52 * 22 + 2 + 64;

/// Write the stat line, newline included.
pub(super) fn write(out: &mut impl core::fmt::Write, s: &PidStat) -> core::fmt::Result {
    // pid (comm) state ppid pgrp session tty_nr tpgid flags minflt cminflt
    // majflt cmajflt utime stime cutime cstime priority nice num_threads
    // itrealvalue starttime vsize rss rsslim
    write!(
        out,
        "{} ({}) {} {} {} {} {} {} 0 0 0 0 0 {} {} {} {} {} {} {} 0 {} {} {} {}",
        s.pid,
        s.comm,
        s.state as char,
        s.ppid,
        s.pgrp,
        s.session,
        s.tty_nr,
        s.tpgid,
        ticks(s.utime_ns),
        ticks(s.stime_ns),
        ticks(s.cutime_ns),
        ticks(s.cstime_ns),
        20 + s.nice as i64,
        s.nice,
        s.num_threads,
        s.start_ms / (1_000 / USER_HZ),
        s.vsize,
        s.rss_pages,
        u64::MAX,
    )?;
    // startcode endcode startstack kstkesp kstkeip signal blocked sigignore
    // sigcatch wchan nswap cnswap, then exit_signal (SIGCHLD), processor,
    // rt_priority, policy, delayacct_blkio_ticks, guest_time, cguest_time,
    // start_data end_data start_brk arg_start arg_end env_start env_end
    // exit_code
    writeln!(out, " 0 0 0 0 0 0 0 0 0 0 0 0 17 0 0 0 0 0 0 0 0 0 0 0 0 0 {}", s.exit_code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> PidStat<'static> {
        PidStat {
            pid: 42,
            comm: "sh",
            state: b'S',
            ppid: 7,
            pgrp: 42,
            session: 7,
            tty_nr: tty_nr(5, 1),
            tpgid: -1,
            utime_ns: 250_000_000,
            stime_ns: 19_999_999,
            cutime_ns: 0,
            cstime_ns: 1_000_000_000,
            nice: 5,
            num_threads: 1,
            start_ms: 12_345,
            vsize: 8192,
            rss_pages: 2,
            exit_code: 0,
        }
    }

    fn line(s: &PidStat) -> String {
        let mut out = String::new();
        write(&mut out, s).unwrap();
        out
    }

    #[test]
    fn fifty_two_fields_in_linux_order() {
        let text = line(&sample());
        assert!(text.ends_with('\n'));
        let fields: Vec<&str> = text.trim_end().split(' ').collect();
        assert_eq!(fields.len(), 52);
        assert_eq!(&fields[..8], &["42", "(sh)", "S", "7", "42", "7", "1281", "-1"]);
        // utime, stime, cutime, cstime in USER_HZ ticks
        assert_eq!(&fields[13..17], &["25", "1", "0", "100"]);
        // priority 20 + nice, nice, num_threads
        assert_eq!(&fields[17..20], &["25", "5", "1"]);
        assert_eq!(fields[21], "1234");
        assert_eq!(fields[22], "8192");
        assert_eq!(fields[23], "2");
        assert_eq!(fields[37], "17");
    }

    #[test]
    fn largest_values_fit_max_len() {
        let mut s = sample();
        s.comm = "a_rather_long_p";
        s.pid = u64::MAX;
        (s.ppid, s.pgrp, s.session, s.tty_nr) = (u64::MAX, u64::MAX, u64::MAX, u64::MAX);
        s.tpgid = i64::MIN;
        (s.utime_ns, s.stime_ns, s.cutime_ns, s.cstime_ns) = (u64::MAX, u64::MAX, u64::MAX, u64::MAX);
        (s.num_threads, s.start_ms, s.vsize, s.rss_pages) = (u64::MAX, u64::MAX, u64::MAX, u64::MAX);
        s.exit_code = i32::MIN;
        assert!(line(&s).len() <= MAX_LEN);
    }

    #[test]
    fn comm_with_spaces_and_parentheses_stays_one_field_to_the_last_paren() {
        let mut s = sample();
        s.comm = "a) b (c";
        let text = line(&s);
        let (head, rest) = text.rsplit_once(')').unwrap();
        assert_eq!(head, "42 (a) b (c");
        assert_eq!(rest.trim_end().split(' ').count(), 51);
    }

    #[test]
    fn negative_nice_and_exit_code() {
        let mut s = sample();
        s.nice = -20;
        s.state = b'Z';
        s.exit_code = 3 << 8;
        let text = line(&s);
        let fields: Vec<&str> = text.trim_end().split(' ').collect();
        assert_eq!(fields[2], "Z");
        assert_eq!(&fields[17..19], &["0", "-20"]);
        assert_eq!(fields[51], "768");
    }

    #[test]
    fn exit_status_matches_wait() {
        assert_eq!(exit_status(3), 0x300);
        assert_eq!(exit_status(-9), 9);
        assert_eq!(exit_status(0), 0);
    }

    #[test]
    fn pty_device_numbers_use_linux_encoding() {
        assert_eq!(tty_nr(136, 0), 0x8800);
        assert_eq!(tty_nr(136, 300), (300 & 0xff) | (136 << 8) | ((300 & !0xff) << 12));
    }
}
