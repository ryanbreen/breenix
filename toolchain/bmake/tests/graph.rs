use bmake::{graph::Graph, reader};

#[test]
fn cycle_is_located_error() {
    let makefile = reader::parse("project.mk", b"a: b\nb: a\n").unwrap();
    let error = Graph::from_makefile(&makefile).unwrap_err();
    let message = error.to_string();

    assert!(message.starts_with("project.mk:2:"), "{message}");
    assert!(message.contains("a -> b -> a"), "{message}");
}

#[test]
fn prerequisites_keep_makefile_order() {
    let makefile = reader::parse(
        "project.mk",
        concat!(
            "# Project rules \\\n",
            "ignored: comment\n",
            "CC = $(TOOL)# expand later\n",
            "t1 t2: z \\\n",
            "\t a # end of prerequisites\n",
            "\t@echo '# recipe' \\\n",
            "\t  second\n",
            "t1: m b\n",
            "z: shared; echo 'inline # recipe'\n",
            "a: shared\n",
        )
        .as_bytes(),
    )
    .unwrap();
    assert!(matches!(
        makefile.entries.first(),
        Some(reader::Entry::Macro(line)) if line.text == "CC = $(TOOL)"
    ));

    let graph = Graph::from_makefile(&makefile).unwrap();
    let prerequisites = |name| graph.prerequisites(name).unwrap().collect::<Vec<_>>();
    assert_eq!(prerequisites("t1"), ["z", "a", "m", "b"]);
    assert_eq!(prerequisites("t2"), ["z", "a"]);
    assert!(prerequisites("shared").is_empty());
    assert!(graph.node("ignored").is_none());

    for target in ["t1", "t2"] {
        let recipes = graph.node(target).unwrap().recipes();
        assert_eq!(recipes.len(), 1);
        assert_eq!(recipes[0].text, "@echo '# recipe' \\\n  second");
    }
    assert_eq!(
        graph.node("z").unwrap().recipes()[0].text,
        " echo 'inline # recipe'"
    );
}

#[test]
fn malformed_makefile_is_error_not_panic() {
    for (input, line, reason) in [
        (b"\techo orphan\n".as_slice(), 1, "recipe before any rule"),
        (b"# heading\nmissing colon\n", 2, "missing ':'"),
        (b"a: b\n\techo \0\n", 2, "NUL"),
        (b"a: b\nnext: c", 2, "missing newline"),
        (b"a: b \\\n", 1, "unfinished continuation"),
    ] {
        let error = reader::parse("broken.mk", input).unwrap_err();
        let message = error.to_string();
        assert!(
            message.starts_with(&format!("broken.mk:{line}:")),
            "{message}"
        );
        assert!(message.contains(reason), "{message}");
    }
}
