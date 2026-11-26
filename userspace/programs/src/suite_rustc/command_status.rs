pub(crate) fn abnormal_status(path: &str, status: i32, output: &[u8]) -> String {
    let output = String::from_utf8_lossy(output);
    let message = match output.trim() {
        "" => format!("{path}: abnormal status {status:#x}"),
        diagnostic => format!("{path}: abnormal status {status:#x}: {diagnostic}"),
    };
    if let Some(unwind) = output.lines().find(|line| line.starts_with("bunwind: fatal phase ")) {
        return format!("abnormal status {status:#x}: {unwind}\n{message}");
    }
    message
}

#[cfg(test)]
mod tests {
    use super::abnormal_status;

    #[test]
    fn abnormal_exit_reports_captured_diagnostic() {
        let failure = abnormal_status("/bin/rustc", 0x86, b"\ncompiler worker failed\n");
        assert!(failure.contains("0x86"), "{failure}");
        assert!(failure.contains("compiler worker failed"), "{failure}");
        assert_eq!(abnormal_status("/bin/rustc", 0x86, b""), "/bin/rustc: abnormal status 0x86");
    }

    #[test]
    fn fatal_unwind_survives_a_preceding_compiler_diagnostic() {
        let compiler_error = "E0308 mismatched types ".repeat(20);
        let unwind = "bunwind: fatal phase 1 unwind error after pc 0x7000234120: no FDE covers this pc in /usr/libexec/breenix/rustc/lib/librustc_driver-2e1bf4637bebadba.so at offset 0x234120";
        let capture = format!("{compiler_error}\n{unwind}\n");
        let failure = abnormal_status("/bin/rustc", 0x86, capture.as_bytes());
        assert_eq!(failure.lines().next(), Some(format!("abnormal status 0x86: {unwind}").as_str()));
    }
}
