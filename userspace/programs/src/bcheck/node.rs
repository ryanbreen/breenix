use serde_json::{json, Value};

pub enum State<'a> {
    Pending,
    Running { pid: u32 },
    Done { verdict: &'a str, failed: bool },
}

pub struct Row<'a> {
    pub name: &'a str,
    pub state: State<'a>,
}

pub fn rows(rows: &[Row<'_>], offset: i32) -> Value {
    let rows: Vec<_> = rows.iter().map(|row| match &row.state {
        State::Pending => json!({"name":row.name,"state":"pending","verdict":null}),
        State::Running { pid } => json!({"name":row.name,"state":"running","verdict":null,"pid":pid}),
        State::Done { verdict, .. } => json!({"name":row.name,"state":"done","verdict":verdict}),
    }).collect();
    json!({"rows":rows,"offset":offset})
}

pub fn totals(rows: &[Row<'_>]) -> Value {
    let passed = rows.iter().filter(|r| matches!(r.state, State::Done { failed: false, .. })).count();
    let failed = rows.iter().filter(|r| matches!(r.state, State::Done { failed: true, .. })).count();
    let done = passed + failed;
    json!({"passed":passed,"failed":failed,"pending":rows.len()-done,"done":done})
}
