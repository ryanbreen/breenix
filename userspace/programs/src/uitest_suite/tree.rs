//! Guest checks over committed widget trees, with independent literal expectations.
use libbui::tree::{Flag, Node, Role, Tree};
#[path = "widget_literals.rs"]
pub mod literals;

pub fn node<'a>(tree: &'a Tree, key: &str) -> Result<&'a Node, String> {
    let mut found = std::iter::once(&tree.root).chain(&tree.children).filter(|n| n.key == key);
    let n = found.next().ok_or_else(|| format!("tree-mismatch node={key} field=key missing"))?;
    if found.next().is_some() { return Err(format!("tree-mismatch node={key} field=key duplicate")); }
    Ok(n)
}
fn equal<T: std::fmt::Debug + PartialEq>(key: &str, field: &str, got: T, want: T) -> Result<(), String> {
    if got == want { Ok(()) } else { Err(format!("tree-mismatch node={key} field={field} expected={want:?} got={got:?}")) }
}
pub fn expect_roles_keys(tree: &Tree) -> Result<(), String> {
    use literals::*;
    let mut expected = vec![("root".to_string(), Role::Window)];
    for (key, role) in [("entry",Role::Textfield),("submit",Role::Button),("disabled",Role::Button),
        ("accept",Role::Checkbox),("amount",Role::Slider),("status",Role::Label),
        ("menu",Role::Menubar),("menu/0",Role::Menu),("menu/0/0",Role::Menuitem),("menu/0/1",Role::Menuitem),
        ("list",Role::List)] { expected.push((key.into(),role)); }
    for i in 0..LIST_ITEMS { expected.push((format!("list/item-{i:02}"),Role::Listitem)); }
    expected.extend([("tabs".into(),Role::Tablist),("tabs/0".into(),Role::Tab),("tabs/1".into(),Role::Tab),("terminal".into(),Role::Terminal)]);
    let got: Vec<_> = std::iter::once(&tree.root).chain(&tree.children).map(|n|(n.key.clone(),n.role)).collect();
    equal("root","roles-keys",got,expected)?;
    equal("root","truncated",tree.root.flags.contains(&Flag::Truncated),false)?;
    for (key,rect) in [("root",[0,0,WIDTH as i32,HEIGHT as i32]),("entry",ENTRY),("submit",SUBMIT),
        ("disabled",DISABLED),("accept",CHECKBOX),("amount",SLIDER),("status",STATUS),
        ("menu",MENU),("list",LIST),("tabs",TABS),("terminal",TERMINAL)] {
        equal(key,"bounds",node(tree,key)?.bounds,rect.map(i64::from))?;
    }
    Ok(())
}
pub fn expect_names_values(tree: &Tree, scripted: bool) -> Result<(), String> {
    use serde_json::json;
    for (key,name,value) in [("entry","",json!(if scripted {"hi"}else{""})),
        ("submit","Submit",json!(null)),("disabled","Disabled",json!(null)),
        ("accept","Accept",json!(scripted)),("amount","",json!(if scripted {50.0}else{25.0})),
        ("status","PASS",json!(null)),("menu/0","Actions",json!(null)),
        ("menu/0/0","Choose",json!(7)),("menu/0/1","Reset",json!(8)),
        ("tabs","",json!(if scripted {1}else{0})),("tabs/0","First",json!(null)),("tabs/1","Second",json!(null)),
        ("list","",if scripted {json!("item-01")}else{json!(null)})] {
        let n = node(tree,key)?;
        equal(key,"name",n.name.as_str(),name)?;
        equal(key,"value",&n.value,&value)?;
    }
    for i in 0..literals::LIST_ITEMS {
        let key = format!("list/item-{i:02}");
        equal(&key,"name",node(tree,&key)?.name.clone(),format!("Item {i:02}"))?;
        equal(&key,"value",&node(tree,&key)?.value,&json!(null))?;
    }
    if scripted { equal("root","value",&tree.root.value,&json!({"submissions":1,"menu_action":7}))?; }
    Ok(())
}
pub fn expect_enabled(tree: &Tree) -> Result<(), String> {
    for key in ["entry","submit","accept","amount","list","tabs"] {
        equal(key,"enabled",node(tree,key)?.flags.contains(&Flag::Enabled),true)?;
    }
    equal("disabled","enabled",node(tree,"disabled")?.flags.contains(&Flag::Enabled),false)
}
pub fn expect_hidden(tree: &Tree, offset: i32) -> Result<(), String> {
    use literals::*;
    for i in 0..LIST_ITEMS {
        let key = format!("list/item-{i:02}");
        let n = node(tree,&key)?;
        let y = LIST[1] + i as i32 * ROW_HEIGHT - offset;
        let top = y.max(LIST[1]);
        let height = (y + ROW_HEIGHT).min(LIST[1]+LIST[3]).saturating_sub(top).max(0);
        equal(&key,"bounds",n.bounds,[LIST[0],y,LIST[2],ROW_HEIGHT].map(i64::from))?;
        equal(&key,"clip",n.clip,[LIST[0],top,LIST[2],height].map(i64::from))?;
        equal(&key,"visible",n.flags.contains(&Flag::Visible),height > 0)?;
    }
    // Closed menu entries remain discoverable, but are not drawn.
    for key in ["menu/0/0","menu/0/1"] { equal(key,"visible",node(tree,key)?.flags.contains(&Flag::Visible),false)?; }
    Ok(())
}
pub fn expect_terminal(tree: &Tree) -> Result<(), String> {
    let n = node(tree,"terminal")?;
    equal("terminal","role",n.role,Role::Terminal)?;
    equal("terminal","name",n.name.as_str(),"Fixed grid")?;
    equal("terminal","bounds",n.bounds,literals::TERMINAL.map(i64::from))?;
    equal("terminal","cell-size",[literals::CELL_WIDTH,literals::CELL_HEIGHT],[8,16])?;
    equal("terminal","value",&n.value,&serde_json::json!({
        "rows":["PASS","grid"],"cols":4,"cursor":[1,2],
        "cells":[
            [{"fg":16777215,"bg":0,"bold":true},{"fg":16777215,"bg":0,"bold":false},{"fg":65280,"bg":0,"bold":false},{"fg":16777215,"bg":128,"bold":false}],
            [{"fg":16777215,"bg":0,"bold":false},{"fg":16777215,"bg":0,"bold":false},{"fg":16777215,"bg":0,"bold":false},{"fg":16777215,"bg":0,"bold":false}]
        ]}))
}

#[cfg(target_os = "breenix")]
mod guest {
    use super::*;
    use breenix_suite::suite::CaseResult;
    use libuitest::{Session, arena::{Arena,Slot,SlotRead},capture};
    use std::time::{Duration,Instant};
    struct App { session: Session, id: u64, _reader: crate::tree_cases::TreeReader }
    impl App {
        fn launch() -> Result<Self,String> {
            let mut session = Session::desktop()?;
            session.launch_app(&[b"/usr/libexec/breenix/uitest-app_test",b"widget tree",b"--widgets"],0)?;
            let ready = session.wait_window("widget tree")?;
            let reader = crate::tree_cases::TreeReader::attach(ready.id).map_err(|e|e.to_string())?;
            let mut app = Self { session,id:ready.id,_reader:reader };
            // A real pointer event after attachment forces the first serialized commit.
            app.motion(30,55)?;
            Ok(app)
        }
        fn motion(&mut self,x:i32,y:i32) -> Result<(),String> {
            let mut infos=vec![libbreenix::window::abi::WinInfo::default();64];
            let count=libbreenix::window::list(&mut infos).map_err(|e|e.to_string())?;
            let info=infos.get(..count).ok_or("window registry overflow")?.iter().find(|w|w.id==self.id).ok_or("widget window missing")?;
            let (sx,sy)=(info.x+x,info.y+y);
            self.session.raw_motion(i32::MIN/2,i32::MIN/2)?;
            self.session.raw_motion(sx,sy)
        }
        fn click(&mut self,x:i32,y:i32) -> Result<(),String> {
            self.motion(x,y)?;
            self.session.raw_press(0x110)?;
            self.session.raw_release(0x110)
        }
        fn snapshot(&mut self) -> Result<Tree,String> {
            let path=std::path::Path::new(self.session.config_root()).join("widget-tree.arena");
            let result=(|| {
                let mut arena=Arena::create(&path,self.session.arena_mut().layout())?;
                capture::window(&mut arena,"widget-tree",self.id)?;
                match arena.read(Slot::Tree)? {
                    SlotRead::Final{record,bytes} if !record.oversize => Tree::parse(&bytes),
                    other => Err(format!("incomplete widget tree: {other:?}")),
                }
            })();
            let remove=std::fs::remove_file(path).map_err(|e|e.to_string());
            let tree=result?; remove?; Ok(tree)
        }
        fn wait(&mut self,judge:impl Fn(&Tree)->Result<(),String>) -> Result<(),String> {
            let end=Instant::now()+Duration::from_secs(2);
            loop {
                let (_,meta)=libbreenix::window::read_meta(self.id).map_err(|e|e.to_string())?;
                if meta.tree_serializations>0 {
                    let tree=self.snapshot()?;
                    match judge(&tree) { Ok(())=>return Ok(()),Err(e) if Instant::now()>=end=>return Err(e),Err(_)=>{} }
                } else if Instant::now()>=end {return Err("no widget commit after reader attachment".into());}
                libbreenix::time::sleep_ms(1).map_err(|e|e.to_string())?;
            }
        }
    }
    fn run(which:u8) -> Result<(),String> {
        let mut app=App::launch()?;
        let result=(|| {
            app.wait(expect_roles_keys)?;
            match which {
                0=>Ok(()),
                1=>{
                    app.wait(|tree|expect_names_values(tree,false))?;
                    app.click(30,55)?;
                    for key in [35,23] {app.session.raw_press(key)?;app.session.raw_release(key)?;}
                    app.click(25,100)?;
                    app.click(340,102)?;
                    app.click(30,180)?;
                    app.click(485,162)?;
                    app.click(25,10)?;
                    app.click(30,33)?;
                    app.click(330,55)?;
                    app.wait(|tree|expect_names_values(tree,true))
                },
                2=>{
                    app.wait(expect_enabled)?;
                    app.click(450,55)?;
                    app.wait(|tree| {expect_enabled(tree)?;equal("root","value",&tree.root.value,&serde_json::json!({"submissions":0,"menu_action":0}))})
                },
                3=>{
                    app.wait(|tree|expect_hidden(tree,0))?;
                    app.motion(30,180)?;
                    app.session.raw_wheel(-1)?;
                    app.wait(|tree|expect_hidden(tree,40))
                },
                4=>app.wait(expect_terminal),
                _=>unreachable!(),
            }
        })();
        let teardown=app.session.teardown(); result?;teardown?;Ok(())
    }
    pub fn roles_keys()->CaseResult{run(0).map_err(Into::into)}
    pub fn names_values()->CaseResult{run(1).map_err(Into::into)}
    pub fn enabled_flag()->CaseResult{run(2).map_err(Into::into)}
    pub fn hidden_nodes()->CaseResult{run(3).map_err(Into::into)}
    pub fn terminal_grid()->CaseResult{run(4).map_err(Into::into)}
}
#[cfg(target_os = "breenix")]
pub use guest::{roles_keys,names_values,enabled_flag,hidden_nodes,terminal_grid};
