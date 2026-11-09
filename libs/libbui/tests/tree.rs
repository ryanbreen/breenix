use libbui::input::PointerCapture;
use libbui::tree::{Flag, Role};
use libbui::widget::{
    button::Button, checkbox::Checkbox, file_picker::{FileEntry, FilePicker}, label::Label,
    list::{List, ListItem}, menu_bar::{Menu, MenuBar, MenuItem}, panel::Panel,
    scroll_bar::ScrollBar, slider::Slider, tab_bar::TabBar, text_field::TextField, Widget,
};
use libbui::{InputState, Rect, Theme, WidgetEvent};

#[test]
fn widget_set_reports_roles_values_flags_and_children() {
    let r = Rect::new(10, 20, 200, 100);
    let clip = Rect::new(0, 0, 400, 400);
    let theme = Theme::dark();
    let mut button = Button::new(r, b"Submit");
    button.set_enabled(false);
    let mut check = Checkbox::new(r, b"Accept");
    check.checked = true;
    let mut slider = Slider::new(r, 10.0, 20.0);
    slider.value = 0.5;
    let mut field = TextField::new(r, 80);
    field.insert_str("Breenix λ");
    field.set_focused(true);
    let label = Label::new(r, b"Status");
    let panel = Panel::with_title(r, b"Form");
    let bar = ScrollBar::new(r, 400, 100);
    let widgets: Vec<(&str, &dyn Widget<Event = WidgetEvent>)> = vec![
        ("submit", &button), ("accept", &check), ("amount", &slider),
        ("entry", &field), ("status", &label), ("form", &panel),
    ];
    let nodes: Vec<_> = widgets.iter().map(|(key, w)| w.tree_node(key, Some("root"), clip)).collect();
    assert_eq!(nodes.iter().map(|n| (n.key.as_str(), n.role)).collect::<Vec<_>>(), vec![
        ("submit", Role::Button), ("accept", Role::Checkbox), ("amount", Role::Slider),
        ("entry", Role::Textfield), ("status", Role::Label), ("form", Role::Canvas),
    ]);
    assert_eq!(nodes[0].name, "Submit");
    assert!(!nodes[0].flags.contains(&Flag::Enabled));
    assert!(nodes[1].flags.contains(&Flag::Checked));
    assert_eq!(nodes[1].value, true);
    assert_eq!(nodes[2].value, 15.0);
    assert_eq!(nodes[3].value, "Breenix λ");
    assert!(nodes[3].flags.contains(&Flag::Focused));
    assert!(nodes[3].flags.contains(&Flag::Editable));
    for n in &nodes {
        assert_eq!(n.parent.as_deref(), Some("root"));
        assert_eq!(n.bounds, [10, 20, 200, 100]);
        assert_eq!(n.clip, [10, 20, 200, 100]);
        assert!(n.flags.contains(&Flag::Visible));
    }
    assert_eq!(bar.tree_node("scroll", None, clip).role, Role::Scrollbar);
    let idle_bar = ScrollBar::new(r, 100, 100);
    assert!(!idle_bar.tree_node("idle-scroll", None, clip).flags.contains(&Flag::Visible));
    let mut tabs = TabBar::new(r, vec![b"First", b"Second"], &theme);
    tabs.set_selected(1);
    let t = tabs.tree_nodes("tabs", Some("root"), clip, &theme);
    assert_eq!(t.iter().map(|n| n.role).collect::<Vec<_>>(), vec![Role::Tablist, Role::Tab, Role::Tab]);
    assert_eq!(t[2].parent.as_deref(), Some("tabs"));
    assert_eq!(t[2].name, "Second");
    assert!(t[2].flags.contains(&Flag::Selected));
    let mut menu = MenuBar::new(Rect::new(10, 20, 200, 22), vec![Menu { label: b"File", items: vec![
        MenuItem::checkable(b"Show", 1, libbui::shortcut::Shortcut::NONE, true),
    ] }]);
    menu.layout(&theme);
    menu.update(&InputState::from_raw(15, 25, 1, 0, 0), &theme);
    let m = menu.tree_nodes("menu", Some("root"), clip, &theme);
    assert_eq!(m.iter().map(|n| n.role).collect::<Vec<_>>(), vec![Role::Menubar, Role::Menu, Role::Menuitem]);
    assert!(m[1].flags.contains(&Flag::Expanded));
    assert!(m[2].flags.contains(&Flag::Checked));
    assert_eq!(m[2].parent.as_deref(), Some("menu/0"));
    assert!(m[2].clip[1] + m[2].clip[3] <= m[0].clip[1] + m[0].clip[3]);
    let picker = FilePicker::new(r, b"/tmp".to_vec(), vec![FileEntry { name: b"note".to_vec(), is_dir: false, size: 3 }], &theme);
    let p = picker.tree_nodes("picker", None, clip, &theme);
    assert_eq!(p[0].role, Role::Dialog);
    assert!(p.iter().any(|n| n.role == Role::Textfield && n.value == "/tmp"));
    assert!(p.iter().any(|n| n.role == Role::Listitem && n.name == "note"));
    assert_eq!(p.iter().filter(|n| n.role == Role::Button).count(), 2);
}

#[test]
fn scrolled_items_keep_keys_and_report_ancestor_clip_and_visibility() {
    let theme = Theme::dark();
    let mut list = List::new(Rect::new(10, 20, 100, 40), 20, vec![
        ListItem::new("a", "Alpha"), ListItem::new("b", "Beta"), ListItem::new("c", "Gamma"),
    ]);
    let ancestor = Rect::new(15, 20, 80, 35);
    let before = list.tree_nodes("list", Some("panel"), ancestor, &theme);
    assert_eq!(before[0].role, Role::List);
    assert!(before[1].flags.contains(&Flag::Visible));
    assert!(!before[3].flags.contains(&Flag::Visible));
    list.set_offset(20);
    let after = list.tree_nodes("list", Some("panel"), ancestor, &theme);
    assert_eq!(after[1].key, before[1].key);
    assert_eq!(after[1].bounds, [10, 0, 100, 20]);
    assert_eq!(after[1].clip, [15, 20, 80, 0]);
    assert!(!after[1].flags.contains(&Flag::Visible));
    assert_eq!(after[2].clip, [15, 20, 80, 20]);
    assert_eq!(after[3].clip, [15, 40, 80, 15]);
    list.update(&InputState::from_raw(20, 45, 1, 0, 1));
    assert_eq!(list.selected(), Some(2));
    assert!(list.tree_nodes("list", None, ancestor, &theme)[3].flags.contains(&Flag::Selected));
}

#[test]
fn press_capture_stays_with_widget_through_drag_and_release() {
    let mut capture = PointerCapture::default();
    let mut slider = Slider::new(Rect::new(0, 0, 100, 20), 0.0, 100.0);
    let mut button = Button::new(Rect::new(100, 0, 100, 20), b"Other");
    let press = InputState::from_raw(10, 10, 1, 0, 0);
    assert_eq!(capture.route(&press, Some(7)), Some(7));
    slider.update(&press);
    slider.rect = Rect::new(0, 30, 100, 20);
    let drag = InputState::from_raw(150, 10, 1, 1, 1);
    assert_eq!(capture.route(&drag, Some(9)), Some(7));
    assert_eq!(slider.update(&drag), WidgetEvent::ValueChanged(100.0));
    assert!(slider.tree_node("slider", None, Rect::new(0, 0, 200, 100)).flags.contains(&Flag::Pressed));
    let release = InputState::from_raw(150, 10, 0, 1, 2);
    assert_eq!(capture.route(&release, Some(9)), Some(7));
    slider.update(&release);
    assert_eq!(capture.route(&InputState::from_raw(150, 10, 0, 0, 3), Some(9)), Some(9));
    assert_eq!(button.update(&release), WidgetEvent::None);
    assert_eq!(capture.route(&press, None), None);
    assert_eq!(capture.route(&drag, Some(9)), None);
    capture.cancel();
    assert_eq!(capture.route(&press, Some(9)), Some(9));
}

#[test]
fn layout_helpers_and_scroll_report_layout_changes() {
    // Run alone: the counter is shared by the windows of this process.
    let before = libbui::layout::layout_version();
    libbui::layout::v_stack(&Rect::new(0, 0, 100, 80), 20, 3, 0);
    assert!(libbui::layout::layout_version() > before);
    let mut bar = ScrollBar::new(Rect::new(90, 0, 10, 80), 400, 80);
    let before = libbui::layout::layout_version();
    bar.scroll(-1);
    assert!(libbui::layout::layout_version() > before);
    let mut list = List::new(Rect::new(0, 0, 80, 20), 20, vec![ListItem::new("a", "A"), ListItem::new("b", "B")]);
    let before = libbui::layout::layout_version();
    list.set_offset(20);
    assert!(libbui::layout::layout_version() > before);
    assert!(include_str!("../../breengel/src/window.rs").contains("m.layout_version = libbui::layout::layout_version()"));
}
