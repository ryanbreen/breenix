pub(crate) fn abnormal_status(path: &str, status: i32, _: &[u8]) -> String {
    format!("{path}: abnormal status {status:#x}")
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
