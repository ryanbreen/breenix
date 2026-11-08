use breenix_suite::suite::CaseResult;

pub fn case_death() -> CaseResult {
    super::runner::collection_evidence("collection-death", "crash")
}
