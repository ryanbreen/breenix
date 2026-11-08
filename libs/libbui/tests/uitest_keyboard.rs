#[path="../../../userspace/programs/src/uitest_widget_layout.rs"] mod uitest_widget_layout;
#[path="../../../userspace/programs/src/uitest_widgets.rs"] mod uitest_widgets;
#[path="../../../userspace/programs/src/uitest_keyboard.rs"] mod keyboard;
use uitest_widgets::Widgets;
fn value(s:&Widgets)->serde_json::Value {s.tree().children.into_iter().find(|n|n.key=="entry").unwrap().value}
#[test]
fn key_and_derived_text_insert_once_and_ctrl_a_selects_without_inserting_controls() {
    let mut s=Widgets::new();
    keyboard::press(&mut s,30,false,false,false,"a"); keyboard::text(&mut s,"a");
    keyboard::press(&mut s,57,false,false,false," "); keyboard::text(&mut s," ");
    keyboard::press(&mut s,28,false,false,false,"\r"); keyboard::text(&mut s,"\r");
    assert_eq!(value(&s),"a ");
    keyboard::press(&mut s,30,false,true,false,"\u{1}");keyboard::text(&mut s,"\u{1}");
    keyboard::text(&mut s,"héλ🙂界");assert_eq!(value(&s),"héλ🙂界");
    keyboard::press(&mut s,15,false,false,false,"\t");keyboard::text(&mut s,"\t");
    keyboard::text(&mut s,"ignored");assert_eq!(value(&s),"héλ🙂界");
    keyboard::press(&mut s,28,false,false,false,"\r");keyboard::text(&mut s,"\r");
    assert_eq!(s.tree().root.value["submissions"],1);
}
