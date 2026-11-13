//! Findings in section 5's output form: `error <ID> <detail>` or `warning <detail>`, one per
//! line, in the order the checker finds them.

pub struct Report {
    pub lines: Vec<String>,
    errors: usize,
    warnings: usize,
}

impl Report {
    pub fn new() -> Report {
        Report { lines: Vec::new(), errors: 0, warnings: 0 }
    }

    pub fn error(&mut self, id: &str, detail: String) {
        self.errors += 1;
        self.lines.push(format!("error {id} {detail}"));
    }

    pub fn warning(&mut self, detail: String) {
        self.warnings += 1;
        self.lines.push(format!("warning {detail}"));
    }

    /// 4 for any error, 1 for warnings only, 0 for a clean image. Exit 8 (unreadable, or no
    /// valid slot) is decided by the caller and takes precedence.
    pub fn exit_code(&self) -> i32 {
        if self.errors > 0 {
            4
        } else if self.warnings > 0 {
            1
        } else {
            0
        }
    }
}
