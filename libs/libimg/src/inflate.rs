//! RFC 1951 DEFLATE and RFC 1950 zlib.
#[derive(Debug,Clone,Copy,PartialEq,Eq)]
pub enum InflateError { BadStoredLength, BadBlockType, CodeOversubscribed, CodeIncomplete, RepeatWithoutPrevious, BadSymbol, DistanceTooFar, Truncated, OutputLimit, BadZlibHeader, PresetDictionary, BadMethod, WindowTooLarge, Adler32Mismatch }
pub fn inflate(input:&[u8],limit:usize)->Result<Vec<u8>,InflateError>{unimplemented!("RFC 1951 decoder: input={} limit={limit}",input.len())}
pub fn zlib_decompress(input:&[u8],limit:usize)->Result<Vec<u8>,InflateError>{unimplemented!("RFC 1950 decoder: input={} limit={limit}",input.len())}
pub struct Inflater { input:Vec<u8>,limit:usize }
impl Inflater { pub fn new(limit:usize)->Self{Self{input:Vec::new(),limit}} pub fn feed(&mut self,input:&[u8])->Result<(),InflateError>{self.input.extend_from_slice(input);Ok(())} pub fn finish(self)->Result<Vec<u8>,InflateError>{inflate(&self.input,self.limit)} }
