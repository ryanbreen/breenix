"""Initial window discovery retains the already verified background and taskbar."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class InitialChrome(unittest.TestCase):
 def test_same_pixels_without_background_or_taskbar_repaint(self):
  s=(ROOT/'userspace/programs/src/bwm/chrome.rs').read_text()
  code=item(s,'pub(super) fn redraw_all_windows(')+item(s,'pub(super) fn compose_full_redraw(')
  if 'fn draw_window_chrome(' in s:code+=item(s,'fn draw_window_chrome(')
  code+=item(s,'pub(super) fn compose_initial_windows(') if 'pub(super) fn compose_initial_windows(' in s else 'fn compose_initial_windows(gpu:&mut Compositor,w:&[Window],s:&Stack,f:&mut Option<CachedFont>){compose_full_redraw(gpu,w,s,b"C",f)}'
  run_rust(self,r'''
#[derive(Clone)]struct Surface{pixels:Vec<u8>,background_copies:usize,taskbar_draws:usize}
struct Compositor{desktop:Surface,background:Surface}
struct CachedFont;struct Window{window_id:u64,chromeless:bool,start:usize,end:usize}
struct Stack;impl Stack{fn minimized(&self,id:u64)->bool{id==3}fn focus(&self)->Option<u64>{Some(2)}}
fn ordered_windows<'a>(w:&'a[Window],_:&Stack)->impl Iterator<Item=&'a Window>{w.iter()}
fn draw_taskbar(f:&mut Surface,c:&[u8],_:&mut Option<CachedFont>){f.taskbar_draws+=1;f.pixels[..8].fill(c[0]);}
fn draw_window_frame(f:&mut Surface,w:&Window,focus:bool,_:&mut Option<CachedFont>){f.pixels[w.start..w.end].fill(if focus{9}else{7});}
fn draw_appbar(f:&mut Surface,w:&[Window],_:&Stack,_:&mut Option<CachedFont>){f.pixels[56..].fill(w.len() as u8);}
mod libgfx{pub mod blit{pub fn blit(d:&mut super::super::Surface,b:&super::super::Surface,_:i32,_:i32){d.background_copies+=1;d.pixels.clone_from(&b.pixels);}}}
mod libuitest{pub mod startup{pub fn mark(_:&str){}}}
@CODE@
#[test]fn initial_chrome(){let initial=Surface{pixels:vec![1;64],background_copies:0,taskbar_draws:0};let mut reference=Compositor{desktop:initial.clone(),background:initial.clone()};let mut font=Some(CachedFont);compose_full_redraw(&mut reference,&[],&Stack,b"C",&mut font);let mut actual=Compositor{desktop:reference.desktop.clone(),background:initial};let w=[Window{window_id:1,chromeless:false,start:12,end:28},Window{window_id:2,chromeless:false,start:24,end:40},Window{window_id:3,chromeless:false,start:40,end:48},Window{window_id:4,chromeless:true,start:48,end:52}];compose_full_redraw(&mut reference,&w,&Stack,b"C",&mut font);let copies=actual.desktop.background_copies;let taskbars=actual.desktop.taskbar_draws;compose_initial_windows(&mut actual,&w,&Stack,&mut font);assert_eq!(actual.desktop.pixels,reference.desktop.pixels);assert_eq!(actual.desktop.background_copies,copies,"initial discovery recopied the verified background");assert_eq!(actual.desktop.taskbar_draws,taskbars,"initial discovery redrew the verified taskbar");}
'''.replace('@CODE@',code.replace('pub(super)','')))
if __name__=='__main__':unittest.main()
