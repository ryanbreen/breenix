//! PNG specification, static images.
#[derive(Debug,Clone,PartialEq,Eq)]
pub struct Image { pub width:u32,pub height:u32,pub pixels:Vec<u8> }
#[derive(Debug,Clone,Copy,PartialEq,Eq)]
pub enum PngError { BadColourType, BadFilter, MissingPlte, BadPlte, ChunkOrder, BadCrc, BadChunkLength, UnknownCritical, BadPaletteIndex, BadHeader, BadSignature, Truncated, TooLarge, Bomb }
pub fn decode(input:&[u8])->Result<Image,PngError>{unimplemented!("PNG decode: {} bytes",input.len())}
pub fn encode(image:&Image)->Result<Vec<u8>,PngError>{unimplemented!("PNG encode: {}x{}",image.width,image.height)}
