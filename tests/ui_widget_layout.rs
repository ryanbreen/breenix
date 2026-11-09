//! Keep the test oracle independent while catching accidental layout drift.
#[path = "../userspace/programs/src/uitest_widget_layout.rs"]
mod app;
#[path = "../userspace/programs/src/uitest_suite/widget_literals.rs"]
mod suite;
#[test]
fn widget_layout_matches_suite_literals() {
    assert_eq!((app::WIDTH, app::HEIGHT), (suite::WIDTH, suite::HEIGHT));
    assert_eq!(
        [
            app::MENU,
            app::ENTRY,
            app::SUBMIT,
            app::DISABLED,
            app::CHECKBOX,
            app::SLIDER,
            app::LIST,
            app::TABS,
            app::TERMINAL,
            app::STATUS
        ],
        [
            suite::MENU,
            suite::ENTRY,
            suite::SUBMIT,
            suite::DISABLED,
            suite::CHECKBOX,
            suite::SLIDER,
            suite::LIST,
            suite::TABS,
            suite::TERMINAL,
            suite::STATUS
        ]
    );
    assert_eq!(
        (
            app::ROW_HEIGHT,
            app::LIST_ITEMS,
            app::CELL_WIDTH,
            app::CELL_HEIGHT
        ),
        (
            suite::ROW_HEIGHT,
            suite::LIST_ITEMS,
            suite::CELL_WIDTH,
            suite::CELL_HEIGHT
        )
    );
}
