pub(crate) fn abnormal_status(path: &str, status: i32, output: &[u8]) -> String {
    let output = String::from_utf8_lossy(output);
    match output.trim() {
        "" => format!("{path}: abnormal status {status:#x}"),
        diagnostic => format!("{path}: abnormal status {status:#x}: {diagnostic}"),
    }
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
}
