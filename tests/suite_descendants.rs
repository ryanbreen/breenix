//! Host check of the production descendant selector; no guest or hardware involved.
use std::{fs, process::Command};

#[test]
fn production_victims_stay_inside_the_runner_tree() {
    let root = std::env::current_dir().unwrap();
    let path = root.join("libs/libbreenix/src/suite_descendants.rs");
    let source = fs::read_to_string(path).expect("missing production descendant selector");
    let start = source.find("pub fn descendant_victims(").expect("missing pure selector");
    let brace = start + source[start..].find('{').unwrap();
    let mut depth = 0;
    let end = source[brace..].bytes().position(|b| {
        match b { b'{' => depth += 1, b'}' => depth -= 1, _ => {} }
        depth == 0
    }).unwrap() + brace + 1;
    let dir = std::env::temp_dir().join(format!("suite-descendants-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("test.rs"), format!(r#"
{}
#[test] fn victims() {{
    // 44 is an adopted orphan; 45 still has two generations of ancestors.
    let rows = [(1,0), (10,1), (40,10), (41,40), (42,41), (44,40), (45,42),
                (50,10), (51,50), (60,1), (70,999), (80,81), (81,80)];
    assert_eq!(descendant_victims(40, &rows), vec![41,42,44,45]);
    assert_eq!(descendant_victims(50, &rows), vec![51]);
    assert_eq!(descendant_victims(60, &rows), Vec::<u64>::new());
    let mut reversed = rows.to_vec(); reversed.reverse();
    let mut actual = descendant_victims(40, &reversed); actual.sort();
    assert_eq!(actual, vec![41,42,44,45]);
    // The sequence handoff shares this selector: the outer case, its helper,
    // and a separate subreaper tree are held in the table throughout reparenting.
    let mut sequence = rows.to_vec();
    sequence.extend([(90,50),(91,90),(92,91)]);
    assert_eq!(descendant_victims(40, &sequence), vec![41,42,44,45]);
    sequence.retain(|&(pid,_)| ![41,42].contains(&pid));
    for row in &mut sequence {{ if row.0 == 45 {{ row.1 = 40; }} }}
    assert_eq!(descendant_victims(40, &sequence), vec![44,45]);
    assert!(sequence.contains(&(50,10)) && sequence.contains(&(92,91)));
    // Parent exits: its children become direct children of the subreaper.
    assert_eq!(descendant_victims(40, &[(40,10),(42,40),(45,42),(50,10)]), vec![42,45]);
    assert!(descendant_victims(0, &rows).is_empty());
    assert!(descendant_victims(1, &rows).is_empty()); // PID 1 uses the unchanged sweep.
    assert!(descendant_victims(40, &[(40,41),(41,99)]).is_empty());
    assert!(descendant_victims(40, &[]).is_empty());
}}
"#, &source[start..end])).unwrap();
    let result = Command::new("rustc").args(["--edition=2021", "-Dwarnings", "--test"])
        .arg(dir.join("test.rs")).arg("-o").arg(dir.join("test")).output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let result = Command::new(dir.join("test")).output().unwrap();
    assert!(result.status.success(), "{}{}", String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
    fs::remove_dir_all(dir).unwrap();
}
