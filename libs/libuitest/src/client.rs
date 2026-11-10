//! One record grammar for outer and contained clients; only the outer mode writes
//! to the runner's mandatory result pipe.
use breenix_suite::suite::{self, result::Accounting};
#[cfg(any(target_os = "breenix", test))]
use crate::{FailureClass, ScenarioOutcome};

pub struct RecordClient<'a> {
    accounting: Accounting<'a>,
    outer: bool,
    records: Vec<String>,
    error: Option<String>,
}

impl<'a> RecordClient<'a> {
    /// The ordinary case client. The runner remains the authority on its records.
    pub fn outer(suite: &'a str, case: &'a str, checks: &'a [&'a str]) -> Result<Self, String> {
        Self::new(suite, case, checks, true)
    }
    #[cfg(any(target_os = "breenix", test))]
    pub(crate) fn contained(suite: &'a str, case: &'a str, checks: &'a [&'a str]) -> Result<Self, String> {
        Self::new(suite, case, checks, false)
    }
    fn new(suite: &'a str, case: &'a str, checks: &'a [&'a str], outer: bool) -> Result<Self, String> {
        Ok(Self { accounting: Accounting::new(suite, case, checks, false)?, outer, records: Vec::new(), error: None })
    }
    pub fn is_outer(&self) -> bool { self.outer }
    /// Validate one complete FRAME/TREE/INPUT line and bind it to this scenario.
    pub fn record(&mut self, line: &str) -> Result<(), String> {
        let valid = self.accounting.accept(line);
        if self.outer {
            if let Err(error) = suite::result_record(line) {
                let reason = format!("{error:?}");
                self.error.get_or_insert_with(|| reason.clone());
                return Err(reason);
            }
        }
        match valid {
            Ok(()) => { self.records.push(line.into()); Ok(()) },
            Err(error) => { self.error.get_or_insert_with(|| error.clone()); Err(error) },
        }
    }
    #[cfg(any(target_os = "breenix", test))]
    pub(crate) fn finish(self, mut outcome: ScenarioOutcome) -> ScenarioOutcome {
        let problem = self.error.or_else(|| {
            if matches!(outcome, ScenarioOutcome::Pass { .. }) { self.accounting.problem() } else { None }
        });
        if let Some(problem) = problem {
            outcome = ScenarioOutcome::fail(FailureClass::Infrastructure, "record-accounting", &problem);
        }
        *outcome.records_mut() = self.records;
        outcome
    }
}
