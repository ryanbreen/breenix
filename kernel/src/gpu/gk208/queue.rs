//! Breenix-owned admission accounting; completion uses ordered engine receipts.
pub struct Queue { issued:u32, finished:u32 }
impl Queue {
    pub const fn new()->Self {Self{issued:0,finished:0}}
    #[cfg(feature = "testing")]
    pub fn drained(sequence:u32)->Self {Self{issued:sequence,finished:sequence}}
    pub fn outstanding(&self)->u32 {self.issued-self.finished}
    pub fn next(&self)->Result<u32,&'static str> {
        if self.outstanding()>=4 {return Err("CE backpressure");}
        self.issued.checked_add(1).ok_or("CE epoch exhausted")
    }
    pub fn slot(sequence:u32)->u32 {(sequence-1)%4}
    pub fn entry(sequence:u32)->u32 {(sequence-1)%512}
    pub fn consumed(put:u32,get:u32)->Option<u32> {
        if get>=512 {return None;}
        put.checked_sub((put%512+512-get)%512)
    }
    pub fn publish(&mut self,sequence:u32)->Result<(),&'static str> {
        if self.next()?!=sequence {return Err("CE publication order");}
        self.issued=sequence;Ok(())
    }
    pub fn complete(&mut self,sequence:u32)->Result<(),&'static str> {
        if sequence<=self.finished || sequence>self.issued {return Err("CE foreign release");}
        self.finished=sequence;Ok(())
    }
}
