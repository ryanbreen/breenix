mod checks;
#[test]
fn image_contracts() {
 let mut failures=Vec::new(); for &(name,run) in checks::CHECKS { match std::panic::catch_unwind(run) { Ok(Ok(()))=>{}, Ok(Err(e))=>failures.push(format!("{name}: {e}")), Err(_)=>failures.push(format!("{name}: codec panicked")) } } assert!(failures.is_empty(), "{}", failures.join("\n"));
}
