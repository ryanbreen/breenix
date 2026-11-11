"""Exercise both production menu updates after collaboration exhausts its undo history."""
import re
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import ROOT, run_rust


class UndoShortcut(unittest.TestCase):
    def test_session_can_submit_undo_at_zero_depth(self):
        source = (ROOT / 'userspace/programs/src/guskit.rs').read_text()
        updates = re.findall(r'menu\.set_enabled\(UNDO, [^;]+;', source)
        self.assertEqual(len(updates), 2)  # input dispatch and the event loop
        widget = (ROOT / 'libs/libbui/src/widget/menu_bar.rs').read_text()
        methods = '\n'.join(item(widget, f'pub fn {name}(')
                            for name in ['set_enabled', 'match_shortcut'])
        for update in updates:
            run_rust(self, r'''
type MenuAction = u16;
const UNDO: u16 = 3;
struct Shortcut { key:u8, ctrl:bool, shift:bool, alt:bool }
impl Shortcut { fn is_some(&self)->bool { self.key != 0 } }
struct MenuItem { action:u16, enabled:bool, shortcut:Shortcut }
struct Menu { items:Vec<MenuItem> }
struct MenuBar { menus:Vec<Menu> }
impl MenuBar { METHODS }
struct App { session:Option<()>, depth:usize }
impl App { fn undo_depth(&self)->usize { self.depth } }
fn update(app:&App, menu:&mut MenuBar) { UPDATE }
#[test] fn second_undo_reaches_the_session() {
    let mut menu=MenuBar { menus:vec![Menu { items:vec![MenuItem {
        action:UNDO, enabled:false,
        shortcut:Shortcut { key:b'Z',ctrl:true,shift:false,alt:false },
    }] }] };
    for session in [None,Some(())] {
        let mut app=App { session,depth:1 };
        // Keep the session input visible even on the old expression.
        assert_eq!(app.session.is_some(),session.is_some());
        update(&app,&mut menu);
        assert_eq!(menu.match_shortcut(26,true,false,false),Some(UNDO));
        app.depth=0;
        update(&app,&mut menu);
        assert_eq!(menu.match_shortcut(26,true,false,false),session.map(|()|UNDO),
            "a session must submit the second Undo for an explicit refusal");
        assert_eq!(menu.match_shortcut(b'Z',true,true,false),None);
    }
}
'''.replace('METHODS', methods).replace('UPDATE', update))


if __name__ == '__main__':
    unittest.main()
