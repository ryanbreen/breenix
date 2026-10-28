extern crate alloc;
#[path = "../kernel/src/gpu/completion.rs"] pub mod gpu_completion;
#[path = "../libs/libbreenix/src/crypto/sha256.rs"] pub mod sha256;
pub mod block { pub mod install { pub mod sha256 { pub use crate::sha256::*; } } }
macro_rules! serial_println { ($($arg:tt)*) => { println!($($arg)*) }; }
pub(crate) use serial_println;
pub mod gpu {
    pub mod dev {
        #[derive(Clone, Copy)] pub enum Tag { Sync(u64) }
        pub struct Done {pub tag:Tag,pub fence:Option<u64>,pub native_fence:Option<u64>,pub answer:u32,pub echoed:u64,pub fenced:bool}
        impl Done {pub fn matches_fence(&self,f:u64)->bool {crate::gpu_completion::matches_echo(self.fence,self.native_fence,self.echoed,self.fenced,f)}}
        pub enum Reap {Empty,Done(Done),Fault}
        pub enum Request {Gk208(crate::gk208::adapter::Request),Other}
        pub enum Device {Gk208(crate::gk208::adapter::Device),Other}
        impl Device {
            pub fn send(&mut self,r:Request)->Result<(),Request>{match (self,r) {(Self::Gk208(d),Request::Gk208(r))=>d.send(r).map_err(Request::Gk208),(_,r)=>Err(r)}}
            pub fn reap(&mut self)->Reap{match self {Self::Gk208(d)=>d.reap(),_=>Reap::Empty}}
            pub fn outstanding(&self)->usize{match self {Self::Gk208(d)=>d.outstanding(),_=>0}}
            pub fn needs_reset(&self)->bool{match self {Self::Gk208(d)=>d.needs_reset(),_=>true}}
        }
    }
}
pub mod gk208 {
    #[path = "../../kernel/src/gpu/gk208/queue.rs"] pub mod queue;
    #[path = "../../kernel/src/gpu/gk208/core_record.rs"] pub mod core_record;
    #[path = "../../kernel/src/gpu/gk208/copy.rs"] pub mod copy;
    #[path = "../../kernel/src/gpu/gk208/caps.rs"] pub mod caps;
    #[path = "../../kernel/src/gpu/gk208/adapter.rs"] pub mod adapter;
    pub mod channel {pub fn elapsed(begin:u64)->u64 {use core::sync::atomic::{AtomicU64,Ordering};static NOW:AtomicU64=AtomicU64::new(0);NOW.fetch_add(10,Ordering::Relaxed)-begin}}
    pub mod bulk {pub fn line(s:String)->Result<(),&'static str>{println!("{s}");Ok(())}}
    pub mod gr {
        pub mod draw {#[derive(Clone,Copy)] pub struct Draw {pub target_va:u64}}
        pub mod channel {
            use crate::{gpu_completion::CeSample,gk208::{adapter::Operation,core_record::Control}};
            pub struct NativeChannel {pub sample:CeSample,pub drain:u32,pub ignore_control:bool,pub published:usize,pub final_sample:Option<CeSample>}
            impl NativeChannel {
                pub fn identity(&self)->(u64,u64){(0x123000,17)}
                pub fn healthy(&self)->bool{true}
                pub fn publish(&mut self,sequence:u32,_:&Operation)->Result<(),&'static str>{self.published+=1;self.final_sample=None;self.sample=CeSample{context:0x123000,epoch:17,sequence,consumed:sequence,healthy:true};self.drain=0;Ok(())}
                pub fn publish_probe(&mut self,sequence:u32,op:&Operation,control:Control)->Result<(),&'static str>{
                    self.publish(sequence,op)?;
                    if self.ignore_control{return Ok(());}
                    let Operation::Copy(rect)=*op else {panic!("control must be a copy")};
                    let words=crate::gk208::copy::probe_encode(rect,0x2000000,17,sequence,control)?;
                    let mut receipt=[0u32;4];let mut pending=(0,0);let mut offset=0;
                    while offset<words.len(){let header=words[offset];let count=((header>>16)&0x1fff) as usize;let method=(header&0x1fff)*4;
                        if method==0x240 {assert_eq!(count,3);pending=(words[offset+2],words[offset+3]);}
                        if method==0x300 && words[offset+1]==0xc {let slot=((pending.0-0x2000000)/4) as usize;receipt[slot]=pending.1;}
                        offset+=count+1;
                    }
                    self.sample.sequence=receipt[0];self.sample.epoch=u64::from(receipt[1])|(u64::from(receipt[2])<<32);self.drain=receipt[3];
                    // Model a sample racing epoch writes before the terminal word.
                    self.final_sample=Some(self.sample);self.sample.epoch=0;Ok(())
                }
                pub fn sample(&mut self,_:u32)->Result<CeSample,&'static str>{Ok(self.sample)}
                pub fn probe_drained(&mut self,sequence:u32)->Result<bool,&'static str>{if self.drain==sequence {if let Some(s)=self.final_sample.take(){self.sample=s;}Ok(true)} else {Ok(false)}}
                pub fn reset(&mut self)->Result<(),&'static str>{Ok(())}
                pub fn restart(&mut self)->Result<(),&'static str>{Ok(())}
            }
        }
    }
    pub mod native {
        use alloc::string::String;
        // PRODUCER
        pub(super) fn exercise(ignore:bool)->(Result<(),&'static str>,String,usize) {
            use crate::{gpu::dev::Device,gpu_completion::CeSample};
            use super::{adapter,gr::{channel::NativeChannel,draw::Draw},copy::Rect};
            let channel=NativeChannel{sample:CeSample{context:0x123000,epoch:17,sequence:0,consumed:0,healthy:true},drain:0,ignore_control:ignore,published:0,final_sample:None};
            let mut board=super::caps::BoardState::empty();board.chipset=0x106;board.channel=(0x123000,17);board.graphics_ready=true;board.created_classes=gpu_abi::gk208::CAPSET_IDS;
            let mut d=Device::Gk208(adapter::Device::probe(channel,board,5));let mut p=CoreProducer::new();
            let result=(||{let receipt=p.run(&mut d,adapter::Operation::Draw(Draw{target_va:0x456000}),"triangle")?;
                assert_eq!((receipt.request.sequence,receipt.sample.sequence),(6,6));assert!(receipt.elapsed_us>0);
                p.run(&mut d,adapter::Operation::Copy(Rect{src:0x400000,dst:0x456000,width:256,height:64,src_pitch:256,dst_pitch:256}),"copy")?;Ok(())})();
            p.emit().unwrap();let Device::Gk208(d)=d else{unreachable!()};
            (result,p.rows,d.into_probe_channel().published)
        }
    }
    #[test] fn clean_requests_follow_three_real_adapter_controls() {
        let (result,rows,count)=native::exercise(false);assert!(result.is_ok());
        assert_eq!(rows.matches("\"type\":\"caps\"").count(),1);
        assert_eq!(rows.matches("\"type\":\"completion\"").count(),8);
        assert_eq!(count,7,"the masked request must not publish");
        for control in ["withheld-release","stale-epoch","masked-class"] {assert_eq!(rows.matches(&format!("\"control\":\"{control}\"")).count(),1);}
        let controls:Vec<_>=rows.lines().filter(|r|r.contains("\"control\"")).collect();
        for i in [1,3,5] {assert!(!controls[i].contains("\"outcome\":\"done\""));assert!(controls[i+1].contains("\"outcome\":\"done\""));}
    }
    #[test] fn a_control_that_really_completes_is_recorded_as_a_failure() {
        let (result,rows,count)=native::exercise(true);assert!(result.is_err());assert_eq!(count,2);
        let last=rows.lines().last().unwrap();assert!(last.contains("\"outcome\":\"error\""));assert!(last.contains(&format!("\"answer\":{}",gpu_abi::gk208::SUCCESS)));
        assert!(!rows.contains("after-withheld"));
    }
}
