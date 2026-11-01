// Synthetic DMEM/rings: validate the source-defined INFO/upload/EXEC exchange.
struct Pmu {
 regs: std::collections::BTreeMap<u32,u32>, writes: Vec<(u32,u32)>,
 dmem: std::collections::BTreeMap<u32,u32>, cursor:u32, clock:u64, reply:bool,
}
impl Pmu { fn new(reply:bool)->Self {Self {regs:std::collections::BTreeMap::from([(0x10a4d0,0x00800270),(0x10a4dc,0x008002f0)]),writes:vec![],dmem:Default::default(),cursor:0,clock:0,reply}} }
impl memx::Access for Pmu {
 fn now_us(&mut self)->u64 {self.clock+=100_000;self.clock}
 fn read(&mut self,a:u32)->u32 {
  if a==0x10a1c4 {let v=*self.dmem.get(&self.cursor).unwrap();self.cursor+=4;return v;}
  if a==0x10a1c0 {return self.cursor;}
  *self.regs.get(&a).unwrap_or(&0)
 }
 fn write(&mut self,a:u32,v:u32) {
  self.writes.push((a,v)); self.regs.insert(a,v);
  if a==0x10a1c0 {self.cursor=v&0xffffff;}
  if a==0x10a1c4 {self.dmem.insert(self.cursor,v);self.cursor+=4;}
  if a==0x10a4a0 && self.reply {
   let packet=0x270+(((v-1)&7)<<4);let message=self.dmem[&(packet+4)];
   let out=if message==0 {[0x400,0x800]}else{[12345,0x00010000]};
   let put=*self.regs.get(&0x10a4c8).unwrap_or(&0);
   for (i,w) in [0x584d454d,message,out[0],out[1]].iter().enumerate() {self.dmem.insert(0x2f0+((put&7)<<4)+i as u32*4,*w);}
   self.regs.insert(0x10a4c8,(put+1)&15);
  }
 }
}
fn protocol_test() {
 let mut io=Pmu::new(true);
 let session=memx::Session::init(&mut io).unwrap();
 assert_eq!(io.writes,vec![(0x10a580,1),(0x10a1c0,0x01000270),(0x10a1c4,0x584d454d),(0x10a1c4,0),(0x10a1c4,0),(0x10a1c4,0),(0x10a4a0,1),(0x10a580,0),(0x10a580,2),(0x10a1c0,0x020002f0),(0x10a4cc,1),(0x10a580,0),(0x10a004,0x40),(0x10a580,3),(0x10a1c0,0x01000400)]);
 let words=vec![1,0x20003,0x10f314,1,2];
 session.load(&mut io,&words).unwrap();
 assert_eq!(session.exec(&mut io).unwrap(),[12345,0x10000]);
 assert_eq!(io.dmem[&0x280],0x584d454d);
 assert_eq!(io.dmem[&0x284],1);
 assert_eq!(io.dmem[&0x288],0x400);
 assert_eq!(io.dmem[&0x28c],0x414);
 assert_eq!(io.regs[&0x10a580],0);
 let mut missing=Pmu::new(false);
 assert_eq!(memx::Session::init(&mut missing).err(),Some("PMU reply timeout"));
}
