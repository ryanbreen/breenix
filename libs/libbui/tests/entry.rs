//! BreenGL §15: scalar editing and focus traversal with literal expectations.
use libbui::{Rect, Key, KeyInput};
use libbui::widget::text_field::TextField;
use libbui::focus::FocusChain;

fn field(max: usize) -> TextField {
    let mut f = TextField::new(Rect::new(0,0,40,20),max);
    f.set_focused(true);
    f
}
#[test]
fn unicode_selection_replacement_and_whole_paste_limit() {
    let mut f = field(5);
    assert!(f.insert_str("aé🦀中"));
    assert_eq!(f.cursor(),4);
    f.key(&KeyInput::new(Key::Left));
    f.key(&KeyInput { shift:true, ..KeyInput::new(Key::Left) });
    assert_eq!(f.selection(),Some((2,3)));
    assert_eq!(f.selected_text(),"🦀");
    assert!(!f.insert_str("XYZ"));
    assert_eq!(f.text(),"aé🦀中");
    assert_eq!(f.selection(),Some((2,3)));
    assert!(f.insert_str("XY"));
    assert_eq!(f.text(),"aéXY中");
    assert_eq!(f.cursor(),4);
    f.key(&KeyInput::new(Key::Delete));
    f.key(&KeyInput::new(Key::Backspace));
    assert_eq!(f.text(),"aéX");
    assert_eq!(f.cursor(),3);
    f.key(&KeyInput::new(Key::Home));
    f.key(&KeyInput::new(Key::Delete));
    assert_eq!(f.text(),"éX");
    assert_eq!(f.cursor(),0);
}
#[test]
fn arrows_collapse_selection_and_cursor_scrolls_both_ways() {
    let mut f = field(100);
    f.insert_str("abcdefghijk");
    assert!(f.scroll_offset()>0);
    assert!(f.cursor_x() < 40);
    f.key(&KeyInput { shift:true, ..KeyInput::new(Key::Home) });
    assert_eq!(f.selected_text(),"abcdefghijk");
    f.key(&KeyInput::new(Key::Right));
    assert_eq!(f.cursor(),11);
    assert_eq!(f.selection(),None);
    f.key(&KeyInput::new(Key::Home));
    assert_eq!(f.scroll_offset(),0);
}
#[test]
fn focus_wraps_skips_disabled_and_handles_no_enabled_entries() {
    let mut f = FocusChain::new();
    for i in 0..3 { f.add(Rect::new(i*20,0,15,20)); }
    f.set_enabled(1,false);
    let tab = KeyInput::new(Key::Tab);
    assert_eq!(f.key(&tab),Some(0));
    assert_eq!(f.key(&tab),Some(2));
    assert_eq!(f.key(&tab),Some(0));
    assert_eq!(f.key(&KeyInput {shift:true,..tab}),Some(2));
    f.set_enabled(0,false); f.set_enabled(2,false);
    assert_eq!(f.key(&tab),None);
}
