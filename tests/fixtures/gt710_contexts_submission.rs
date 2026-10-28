extern crate alloc;
pub use alloc::vec::Vec;
#[path="ROOT/kernel/src/gpu/gk208/gr/channel.rs"] pub mod channel;
#[path="ROOT/kernel/src/gpu/gk208/gr/shaders.rs"] pub mod shaders;
#[path="ROOT/kernel/src/gpu/gk208/gr/draw.rs"] pub mod draw;
#[path="ROOT/kernel/src/gpu/gk208/gr/binding.rs"] pub mod binding;
use channel::{Observation,receipt_nonce};
use std::cell::RefCell;
const TARGET:u64=0x2000000;
const BOUND_US:u64=2_000_000;
struct Registers{events:RefCell<Vec<String>>,current:u32}
impl Registers{fn read(&self,a:usize)->u32{self.events.borrow_mut().push(format!("read-{a:x}"));if a==0x409b00{self.current}else{0}}}
struct Hardware<'a>{regs:&'a mut Registers}
struct Pages;
impl Pages{fn address(&self,i:usize)->u64{assert_eq!(i,0);0x800000}}
struct Identity{epoch:u64,instance:u64}
struct Report{identity:Identity}
struct Session{report:Report,pages:Pages,next_sequence:u32,streams:Vec<Vec<u32>>,reject:usize}
impl Session {
    fn issue_words(&mut self,h:&mut Hardware<'_>,sequence:u32,poison:u32,nonce:u32,budget:u64,stream:&[u32],stop_on_trap:bool)->Result<Observation,&'static str>{
        assert_eq!((poison,budget,stop_on_trap),(85,BOUND_US,true));
        assert_eq!(&stream[stream.len()-2..],&[0x20010014,sequence]);
        self.streams.push(stream.to_vec());h.regs.events.borrow_mut().push(format!("release-{sequence}"));
        Ok(Observation{value:nonce,epoch:self.report.identity.epoch,consumed:true,elapsed_us:1,accepted:self.reject!=self.streams.len()})
    }
    // METHODS
}
#[test]fn activation_and_draw_release_surround_raw_binding_reads(){
    for full in [true,false]{for reject in 0..=2{
        let mut regs=Registers{events:RefCell::new(vec![]),current:0x80000300};
        let mut s=Session{report:Report{identity:Identity{epoch:42,instance:0x300000}},pages:Pages,next_sequence:6,streams:vec![],reject};
        let request=draw::Draw{target_va:0x100000,pitch:256,width:64,height:64,vertex_va:0x200000,quarantine_va:0x400000,vertex_program:0x30,fragment_program:draw::GRADIENT_PROGRAM,report_va:0,epoch:0,nonce:0};
        let mut receipt=binding::Receipt{target:0,epoch:0,nonce:0,before:[0;4],after:[0;4],observation:None};
        let mut current=[0;2];
        let result=s.context_draw(&mut Hardware{regs:&mut regs},request,full,&mut receipt,&mut current);
        assert_eq!(result,match reject{1=>Err("context activation release not observed"),2=>Err("context draw release not observed"),_=>Ok(())});
        let active=draw::Draw{report_va:TARGET,epoch:42,nonce:receipt_nonce(42,true).wrapping_add(6),..request};
        let activation=draw::release_commands(active).unwrap();
        assert_eq!(&s.streams[0][..s.streams[0].len()-2],activation);
        if reject!=1{
            assert_eq!(current,[0x80000300;2]);
            let r=draw::Draw{nonce:receipt_nonce(42,true).wrapping_add(7),..active};
            let expected=if full{draw::commands(r)}else{draw::draw_commands(r)}.unwrap();
            assert_eq!(&s.streams[1][..s.streams[1].len()-2],expected);
            assert_eq!((receipt.target,receipt.epoch,receipt.nonce),(0x800000,42,r.nonce));
            let e=regs.events.borrow();
            assert_eq!(e.iter().filter(|s|s.as_str()=="read-409b00").count(),2);
            let a=e.iter().position(|s|s=="release-6").unwrap();
            let b=e.iter().position(|s|s=="read-409b00").unwrap();
            let c=e.iter().position(|s|s=="release-7").unwrap();
            let d=e.iter().rposition(|s|s=="read-409b00").unwrap();assert!(a<b&&b<c&&c<d);
        }
    }}
}
