"""Startup presentation and optional warm-up errors are not pixel mismatches."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]

class StartupRecovery(unittest.TestCase):
 def test_presentation_error_keeps_its_own_class(self):
  text=(ROOT/'userspace/programs/src/bwm.rs').read_text()
  start=text.index('startup_prepare::during(')
  closure=item(text[start:],'}, || {');body=closure[closure.index('{')+1:closure.rfind('}')]
  run_rust(self,r'''
#[derive(Debug)]struct FrameError;impl std::fmt::Display for FrameError{fn fmt(&self,f:&mut std::fmt::Formatter)->std::fmt::Result{write!(f,"present unavailable")}}
enum CheckError{Failed(String)}
#[derive(Debug,PartialEq)]enum Outcome{Pixels(u64),Mismatch,Presentation}
trait Classified{fn classified(self)->Outcome;}
impl Classified for Result<u64,CheckError>{fn classified(self)->Outcome{match self{Ok(n)=>Outcome::Pixels(n),Err(CheckError::Failed(e))=>{assert!(!e.is_empty());Outcome::Mismatch}}}}
impl Classified for Result<Result<u64,CheckError>,FrameError>{fn classified(self)->Outcome{match self{Ok(r)=>r.classified(),Err(_)=>Outcome::Presentation}}}
struct Gpu{fail_present:bool,fail_pixels:bool}impl Gpu{
 fn present_startup_desktop(&mut self,_p:Option<()>)->Result<(),FrameError>{if self.fail_present{Err(FrameError)}else{Ok(())}}
 fn check(&mut self,_id:u64,_layers:&[()],_p:Option<()>)->Result<u64,CheckError>{if self.fail_pixels{Err(CheckError::Failed("wrong pixel".into()))}else{Ok(4096000)}}
}
mod libuitest{pub mod startup{pub fn mark(_name:&str){}}}
fn measure_check<T>(f:impl FnOnce()->T)->T{f()}
fn attempt(gpu:&mut Gpu)->Outcome{let first_pointer=();let first=(||{@BODY@})();first.classified()}
#[test]fn failure_classes(){assert_eq!(attempt(&mut Gpu{fail_present:true,fail_pixels:false}),Outcome::Presentation);assert_eq!(attempt(&mut Gpu{fail_present:false,fail_pixels:true}),Outcome::Mismatch);assert_eq!(attempt(&mut Gpu{fail_present:false,fail_pixels:false}),Outcome::Pixels(4096000));}
'''.replace('@BODY@',body))

 def test_fill_presentation_error_is_not_a_pixel_mismatch(self):
  text=(ROOT/'userspace/programs/src/bwm/chrome.rs').read_text()
  body=item(text,'pub(super) fn check_desktop_fills').replace('pub(super) fn','fn',1)
  run_rust(self,r'''
#[derive(Debug)]struct FrameError;impl std::fmt::Display for FrameError{fn fmt(&self,f:&mut std::fmt::Formatter)->std::fmt::Result{write!(f,"present unavailable")}}
struct Color{r:u8,g:u8,b:u8}const TASKBAR_BG:Color=Color{r:0,g:0,b:0};const APPBAR_BORDER:Color=Color{r:1,g:1,b:1};const APPBAR_BG:Color=Color{r:2,g:2,b:2};const TASKBAR_HEIGHT:usize=28;const APPBAR_HEIGHT:usize=36;
fn background_pixel(_x:usize,_y:usize,_h:usize)->Color{Color{r:3,g:3,b:3}}fn px(n:usize)->usize{n*2}struct Ui{scale:u32}fn ui()->Ui{Ui{scale:2}}
struct Shown{resource:u64,width:u32,height:u32}struct Compositor{fail_present:bool,fail_pixels:bool}impl Compositor{fn present_startup_desktop(&mut self,_p:Option<()>)->Result<(),FrameError>{if self.fail_present{Err(FrameError)}else{Ok(())}}fn check_fills(&mut self,_s:&[(u32,u32,u32)])->Result<Shown,String>{if self.fail_pixels{Err("wrong pixel".into())}else{Ok(Shown{resource:7,width:2560,height:1600})}}}
mod compositor{pub struct Mode{pub width:u32,pub height:u32}pub fn scanout_resource()->Result<u64,String>{Ok(7)}pub fn display()->Result<(u32,Mode),String>{Ok((0,Mode{width:2560,height:1600}))}}
fn measure_check<T>(f:impl FnOnce()->T)->T{f()}
#[derive(Debug,PartialEq)]enum Outcome{Pixels,Mismatch,Presentation}trait Classified{fn classified(self)->Outcome;}impl Classified for Result<String,String>{fn classified(self)->Outcome{match self{Ok(_)=>Outcome::Pixels,Err(_)=>Outcome::Mismatch}}}impl Classified for Result<Result<String,String>,FrameError>{fn classified(self)->Outcome{match self{Ok(v)=>v.classified(),Err(_)=>Outcome::Presentation}}}
@BODY@
#[test]fn fill_failure_classes(){assert_eq!(check_desktop_fills(&mut Compositor{fail_present:true,fail_pixels:false},2560,1600,0).classified(),Outcome::Presentation);assert_eq!(check_desktop_fills(&mut Compositor{fail_present:false,fail_pixels:true},2560,1600,0).classified(),Outcome::Mismatch);assert_eq!(check_desktop_fills(&mut Compositor{fail_present:false,fail_pixels:false},2560,1600,0).classified(),Outcome::Pixels);}
'''.replace('@BODY@',body))

 def test_optional_title_warmup_error_does_not_exit(self):
  text=(ROOT/'userspace/programs/src/bwm.rs').read_text()
  block=item(text,'if let Err(error) = prepared {')
  run_rust(self,r'''
mod process{pub fn exit(_status:i32)->!{panic!("optional warmup killed desktop")}}
fn warm(prepared:Result<(),&str>){@BLOCK@}
#[test]fn warmup_is_optional(){let old_exit:fn(i32)->! =process::exit;let _=old_exit;assert!(std::panic::catch_unwind(||warm(Err("device error"))).is_ok());warm(Ok(()));}
'''.replace('@BLOCK@',block))

 def test_recovery_retries_only_presentation_errors(self):
  path=ROOT/'userspace/programs/src/bwm/startup_prepare.rs'
  run_rust(self,r'''
#[path="@PATH@"]mod startup;
#[test]fn strict_session_does_not_retry(){let mut calls=0;let r:Result<Result<u64,&str>,&str>=startup::recover(Err("present"),true,|_|{calls+=1;Ok(Ok(4096000))});assert_eq!(r,Err("present"));assert_eq!(calls,0);}
#[test]fn persistent_refusal_stops_reopening(){let mut calls=0;let r:Result<(),&str>=startup::recover(Err("not display owner"),false,|_|{calls+=1;assert!(calls<=2,"persistent refusal reopened a third context");Err("still refused")});assert_eq!(r,Err("still refused"));assert_eq!(calls,2);}
#[test]fn production_retries_then_checks(){let mut calls=0;let r:Result<Result<u64,&str>,&str>=startup::recover(Err("present"),false,|_|{calls+=1;if calls==1{Err("present again")}else{Ok(Ok(4096000))}});assert_eq!(r,Ok(Ok(4096000)));assert_eq!(calls,2);let mut retries=0;let r:Result<Result<u64,&str>,&str>=startup::recover(Ok(Err("pixel mismatch")),false,|_|{retries+=1;Ok(Ok(4096000))});assert_eq!(r,Ok(Err("pixel mismatch")));assert_eq!(retries,0);let(prepared,checked)=startup::during(||Ok(()),||7);assert_eq!((prepared,checked),(Ok(()),7));}
'''.replace('@PATH@',str(path)))

 def test_pending_sigterm_stops_startup_before_reopening(self):
  text=(ROOT/'userspace/programs/src/bwm.rs').read_text()
  # Extract the real retry checkpoint, including its signal-mask transition.
  body=item(text,'fn startup_retry_wait(')
  run_rust(self,r'''
use std::sync::atomic::{AtomicBool,Ordering};
static TERMINATE:AtomicBool=AtomicBool::new(false);
static BLOCKED:AtomicBool=AtomicBool::new(true);
mod signal{pub const SIG_UNBLOCK:i32=1;pub const SIG_BLOCK:i32=0;}
fn term_mask(how:i32){BLOCKED.store(how==signal::SIG_BLOCK,Ordering::Relaxed);if how==signal::SIG_UNBLOCK{TERMINATE.store(true,Ordering::Relaxed);}}
mod libbreenix{pub mod time{pub fn sleep_ms(_ms:u64)->Result<(),()>{assert!(!super::super::BLOCKED.load(super::super::Ordering::Relaxed));Ok(())}}}
mod process{pub fn exit(status:i32)->!{assert_eq!(status,0);panic!("terminated")}}
@BODY@
#[test]fn queued_term_is_delivered_before_retry(){assert!(std::panic::catch_unwind(startup_retry_wait).is_err(),"startup ignored SIGTERM");assert!(TERMINATE.load(Ordering::Relaxed));assert!(BLOCKED.load(Ordering::Relaxed));}
'''.replace('@BODY@',body))

if __name__=='__main__':unittest.main()
