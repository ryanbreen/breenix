#[path = "../userspace/programs/src/bcheck/node.rs"]
mod node;

#[test]
fn totals_equal_published_rows() {
    let rows = [
        node::Row { name: "pass", state: node::State::Done { verdict: "pass", failed: false } },
        node::Row { name: "fail", state: node::State::Done { verdict: "exit 1", failed: true } },
        node::Row { name: "run", state: node::State::Running { pid: 42 } },
        node::Row { name: "wait", state: node::State::Pending },
    ];
    assert_eq!(node::rows(&rows, 18), serde_json::json!({"rows":[
        {"name":"pass","state":"done","verdict":"pass"},
        {"name":"fail","state":"done","verdict":"exit 1"},
        {"name":"run","state":"running","verdict":null,"pid":42},
        {"name":"wait","state":"pending","verdict":null}
    ],"offset":18}));
    assert_eq!(node::totals(&rows), serde_json::json!({"passed":1,"failed":1,"pending":2,"done":2}));
}
