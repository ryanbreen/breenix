//! Independent BreenGL image checks. RFC 1951 literal bits and PNG literal samples;
//! none of the fixtures or expected values are made by the codec under test.
use libimg::inflate::{inflate, zlib_decompress, InflateError as IE, Inflater};
use libimg::png::{self, Image, PngError as PE};
pub type Check = Result<(), String>;
fn eq<T: std::fmt::Debug + PartialEq>(got: T, want: T) -> Check {
    if got == want { Ok(()) } else { Err(format!("contract §§7,11–13: got {got:?}, expected {want:?}")) }
}
#[derive(Default)]
pub struct Bits { pub bytes: Vec<u8>, n: usize }
impl Bits {
    pub fn put(&mut self, v: u32, n: usize) {
        for i in 0..n { if self.n % 8 == 0 { self.bytes.push(0); } self.bytes[self.n / 8] |= (((v >> i) & 1) as u8) << (self.n % 8); self.n += 1; }
    }
    pub fn code(&mut self, v: u32, n: usize) { for i in (0..n).rev() { self.put((v >> i) & 1, 1); } }
    pub fn fixed(&mut self, s: u32) { match s { 0..=143 => self.code(0x30+s,8), 144..=255 => self.code(0x190+s-144,9), 256..=279 => self.code(s-256,7), _ => self.code(0xc0+s-280,8) } }
    pub fn stored(&mut self, data: &[u8], last: bool) { self.put(u32::from(last),3); while self.n % 8 != 0 { self.put(0,1); } self.put(data.len() as u32,16); self.put(!(data.len() as u32) & 65535,16); for &b in data { self.put(b as u32,8); } }
}
// A complete code-length alphabet: symbols 0..15 each have length 4. This
// deliberately uses no encoder under test and permits arbitrary target lengths.
fn dynamic(lit: &[u8], dist: &[u8], body: &[(u32, usize)]) -> Vec<u8> {
    let mut b=Bits::default(); b.put(5,3); b.put((lit.len()-257) as u32,5); b.put((dist.len()-1) as u32,5); b.put(15,4);
    for s in [16,17,18,0,8,7,9,6,10,5,11,4,12,3,13,2,14,1,15] { b.put(if s<16 {4} else {0},3); }
    for &n in lit.iter().chain(dist) { b.code(n as u32,4); }
    for &(v,n) in body { b.code(v,n); } b.bytes
}
fn literal_tree() -> Vec<u8> { let mut l=vec![0;257]; l[65]=1; l[256]=1; l }
fn fixed_data(data: &[u8]) -> Vec<u8> { let mut b=Bits::default(); b.put(3,3); for &v in data { b.fixed(v as u32); } b.fixed(256); b.bytes }
pub fn inflate_stored() -> Check { let mut b=Bits::default(); b.stored(b"stored\0bytes",true); eq(inflate(&b.bytes,12),Ok(b"stored\0bytes".to_vec())) }
pub fn inflate_stored_length() -> Check { eq(inflate(&[1,1,0,0,0,42],10),Err(IE::BadStoredLength)) }
pub fn inflate_fixed_huffman() -> Check { let data:Vec<u8>=(0..=255).collect(); eq(inflate(&fixed_data(&data),256),Ok(data)) }
pub fn inflate_dynamic_huffman() -> Check { eq(inflate(&dynamic(&literal_tree(),&[1],&[(0,1),(0,1),(1,1)]),2),Ok(b"AA".to_vec())) }
pub fn inflate_block_sequence() -> Check {
    let mut b=Bits::default(); b.stored(b"S",false); b.put(2,3); b.fixed(70); b.fixed(256);
    // Finish the fixed block with a stored block to align the following dynamic block.
    b.stored(b"",false); let d=dynamic(&literal_tree(),&[0],&[(0,1),(1,1)]); b.bytes.extend(d); eq(inflate(&b.bytes,3),Ok(b"SFA".to_vec()))
}
pub fn inflate_block_type_invalid() -> Check { eq(inflate(&[7],1),Err(IE::BadBlockType)) }
pub fn inflate_code_oversubscribed() -> Check { let mut l=literal_tree(); l[66]=1; eq(inflate(&dynamic(&l,&[1],&[]),1),Err(IE::CodeOversubscribed)) }
pub fn inflate_code_incomplete() -> Check {
    let mut b=Bits::default(); b.put(5,3); b.put(0,14); for _ in 0..4 { b.put(0,3); }
    eq(inflate(&b.bytes,1),Err(IE::CodeIncomplete))?;
    let mut l=literal_tree(); l[65]=2; l[256]=2; eq(inflate(&dynamic(&l,&[1],&[]),1),Err(IE::CodeIncomplete))?;
    eq(inflate(&dynamic(&literal_tree(),&[2],&[]),1),Err(IE::CodeIncomplete))
}
pub fn inflate_code_single() -> Check { let mut l=vec![0;257]; l[256]=1; eq(inflate(&dynamic(&l,&[1],&[(0,1)]),0),Ok(vec![]))?; let mut l=vec![0;286]; l[65]=1; l[256]=2; l[285]=2; eq(inflate(&dynamic(&l,&[1],&[(0,1),(3,2),(0,1),(2,2)]),259),Ok(vec![65;259])) }
pub fn inflate_no_distance_codes() -> Check { eq(inflate(&dynamic(&literal_tree(),&[0],&[(0,1),(1,1)]),1),Ok(vec![65]))?; let mut l=vec![0;258]; l[256]=1;l[257]=1; eq(inflate(&dynamic(&l,&[0],&[(1,1)]),10),Err(IE::BadSymbol)) }
pub fn inflate_repeat_first() -> Check {
    let mut b=Bits::default(); b.put(5,3); b.put(0,5);b.put(0,5);b.put(0,4);
    // code-length alphabet: 0 and 16, each one bit. First decoded length = 16.
    for n in [1,0,0,1] { b.put(n,3); } b.code(1,1);b.put(0,2); eq(inflate(&b.bytes,1),Err(IE::RepeatWithoutPrevious))
}
pub fn inflate_symbols_invalid() -> Check {
    for s in [286,287] { let mut b=Bits::default();b.put(3,3);b.fixed(s);eq(inflate(&b.bytes,10),Err(IE::BadSymbol))?; }
    for d in [30,31] { let mut b=Bits::default();b.put(3,3);b.fixed(65);b.fixed(257);b.code(d,5);eq(inflate(&b.bytes,10),Err(IE::BadSymbol))?; } Ok(())
}
pub fn inflate_distance_too_far() -> Check { let mut b=Bits::default();b.put(3,3);b.fixed(257);b.code(0,5);eq(inflate(&b.bytes,10),Err(IE::DistanceTooFar)) }
pub fn inflate_distance_max() -> Check { let seed:Vec<u8>=(0..32768).map(|i|(i%251) as u8).collect(); let mut b=Bits::default();b.stored(&seed,false);b.put(3,3);b.fixed(257);b.code(29,5);b.put(8191,13);b.fixed(256);let mut want=seed;want.extend_from_slice(&[0,1,2]); libimg::work::reset();let got=inflate(&b.bytes,want.len());let work=libimg::work::steps();eq(got,Ok(want))?;if work>4*32771+16*b.bytes.len() as u64+2048 {return Err(format!("inflate work {work} exceeds §7"));} Ok(()) }
pub fn inflate_length_258() -> Check { let mut b=Bits::default();b.put(3,3);b.fixed(88);b.fixed(285);b.code(0,5);b.fixed(256);eq(inflate(&b.bytes,259),Ok(vec![88;259])) }
pub fn inflate_truncated() -> Check { for data in [fixed_data(b"test"),dynamic(&literal_tree(),&[1],&[(0,1),(1,1)]),{let mut b=Bits::default();b.stored(b"test",true);b.bytes}] { for cut in 0..data.len() { eq(inflate(&data[..cut],100),Err(IE::Truncated))?; }} Ok(()) }
fn header(cmf:u8,flags:u8)->[u8;2] { let v=(cmf as u16)*256+flags as u16;[cmf, flags+((31-v%31)%31) as u8] }
pub fn inflate_zlib_header() -> Check { eq(zlib_decompress(&[0x78,0],1),Err(IE::BadZlibHeader))?;for (h,e) in [(header(0x78,0x20),IE::PresetDictionary),(header(0x77,0),IE::BadMethod),(header(0x88,0),IE::WindowTooLarge)] {eq(zlib_decompress(&h,1),Err(e))?;} Ok(()) }
pub fn inflate_adler32() -> Check { let mut z=zstored(b"Wikipedia"); eq(zlib_decompress(&z,9),Ok(b"Wikipedia".to_vec()))?;z.pop();z.push(0);eq(zlib_decompress(&z,9),Err(IE::Adler32Mismatch)) }
pub fn inflate_output_limit() -> Check { eq(inflate(&fixed_data(b"12345"),4),Err(IE::OutputLimit))?;let mut b=Bits::default();b.stored(b"12345",true);eq(inflate(&b.bytes,4),Err(IE::OutputLimit)) }
pub fn inflate_byte_split() -> Check { let data=dynamic(&literal_tree(),&[1],&[(0,1),(0,1),(1,1)]);for split in 0..=data.len() {let mut i=Inflater::new(2);i.feed(&data[..split]).map_err(|e|format!("feed: {e:?}"))?;i.feed(&data[split..]).map_err(|e|format!("feed: {e:?}"))?;eq(i.finish(),Ok(b"AA".to_vec()))?;}let mut i=Inflater::new(2);for b in data {i.feed(&[b]).map_err(|e|format!("byte feed: {e:?}"))?;}eq(i.finish(),Ok(b"AA".to_vec())) }

// Slow polynomial division and literal Adler recurrence are independent check code.
pub fn crc(data:&[u8])->u32 { let mut c=0xffff_ffffu32;for &b in data {c^=b as u32;for _ in 0..8 {c=if c&1!=0 {(c>>1)^0xedb8_8320}else{c>>1};}}!c }
pub fn adler(data:&[u8])->u32 {let(mut a,mut b)=(1u32,0u32);for &x in data {a=(a+x as u32)%65521;b=(b+a)%65521;}(b<<16)|a}
pub fn zstored(data:&[u8])->Vec<u8> {let mut b=Bits::default();if data.is_empty(){b.stored(&[],true);}else{let chunks=data.chunks(65535);let n=chunks.len();for(i,c)in chunks.enumerate(){b.stored(c,i+1==n);}}let mut v=vec![0x78,0x01];v.extend(b.bytes);v.extend(adler(data).to_be_bytes());v}
pub fn chunk(v:&mut Vec<u8>,name:&[u8;4],data:&[u8]) {v.extend((data.len() as u32).to_be_bytes());let start=v.len();v.extend(name);v.extend(data);v.extend(crc(&v[start..]).to_be_bytes());}
pub fn ihdr(w:u32,h:u32,d:u8,t:u8,interlace:u8)->Vec<u8>{let mut v=Vec::new();v.extend(w.to_be_bytes());v.extend(h.to_be_bytes());v.extend([d,t,0,0,interlace]);v}
pub fn pngfile(header:&[u8],before:&[([u8;4],Vec<u8>)],raw:&[u8])->Vec<u8> {let mut v=b"\x89PNG\r\n\x1a\n".to_vec();chunk(&mut v,b"IHDR",header);for(n,d)in before{chunk(&mut v,n,d);}chunk(&mut v,b"IDAT",&zstored(raw));chunk(&mut v,b"IEND",&[]);v}
fn decoded(v:&[u8],w:u32,h:u32,p:&[u8])->Check {let i=png::decode(v).map_err(|e|format!("§13 PNG decode {e:?}"))?;eq((i.width,i.height,i.pixels),(w,h,p.to_vec()))}
fn pm(c:u8,a:u8)->u8 {((c as u32*a as u32+127)/255) as u8}
fn scale(v:u16,d:u8)->u8 {if d==16 {((v as u32*255+32767)/65535) as u8}else{(v as u32*255/((1u32<<d)-1)) as u8}}
fn samples(values:&[u16],depth:u8)->Vec<u8> {if depth==16 {values.iter().flat_map(|v|v.to_be_bytes()).collect()}else if depth==8{values.iter().map(|v|*v as u8).collect()}else{let mut v=vec![0;(values.len()*depth as usize).div_ceil(8)];for(i,&x)in values.iter().enumerate(){let bit=i*depth as usize;v[bit/8]|=(x as u8)<<(8-depth as usize-bit%8);}v}}
fn colour_case(t:u8,depths:&[u8])->Check {for &d in depths {let max=if d==16{65535}else{(1<<d)-1};let vals=match t{0=>vec![0,max/2,max],2=>vec![0,max/2,max,max,0,max/2],4=>vec![max,max/2,max/2,max],6=>vec![max,max/2,0,max/2,0,max,max,max],_=>unreachable!()};let channels=match t{0=>1,2=>3,4=>2,_=>4};let mut want=Vec::new();for px in vals.chunks(channels) {let(r,g,b,a)=match t{0=>(px[0],px[0],px[0],max),2=>(px[0],px[1],px[2],max),4=>(px[0],px[0],px[0],px[1]),_=>(px[0],px[1],px[2],px[3])};let a=scale(a,d);want.extend([pm(scale(b,d),a),pm(scale(g,d),a),pm(scale(r,d),a),a]);}let mut raw=vec![0];raw.extend(samples(&vals,d));let w=(vals.len()/channels)as u32;decoded(&pngfile(&ihdr(w,1,d,t,0),&[],&raw),w,1,&want)?;}Ok(())}
pub fn png_decode_gray_depths()->Check{colour_case(0,&[1,2,4,8,16])}
pub fn png_decode_gray_alpha()->Check{colour_case(4,&[8,16])}
pub fn png_decode_rgb()->Check{colour_case(2,&[8,16])}
pub fn png_decode_rgba()->Check{colour_case(6,&[8,16])}
pub fn png_decode_palette_depths()->Check{for d in [1,2,4,8]{let mut raw=vec![0];raw.extend(samples(&[0,1,1,0],d));decoded(&pngfile(&ihdr(4,1,d,3,0),&[(*b"PLTE",vec![1,2,3,40,50,60])],&raw),4,1,&[3,2,1,255,60,50,40,255,60,50,40,255,3,2,1,255])?;}Ok(())}
pub fn png_decode_trns_gray()->Check{for d in [1,2,4,8,16]{let mut raw=vec![0];raw.extend(samples(&[0,1],d));let c=scale(1,d);decoded(&pngfile(&ihdr(2,1,d,0,0),&[(*b"tRNS",vec![0,0])],&raw),2,1,&[0,0,0,0,c,c,c,255])?;}Ok(())}
pub fn png_decode_trns_rgb()->Check{for d in [8,16]{let mut raw=vec![0];raw.extend(samples(&[1,2,3,3,2,1],d));decoded(&pngfile(&ihdr(2,1,d,2,0),&[(*b"tRNS",vec![0,1,0,2,0,3])],&raw),2,1,&[0,0,0,0,scale(1,d),scale(2,d),scale(3,d),255])?;}Ok(())}
pub fn png_decode_trns_palette()->Check{decoded(&pngfile(&ihdr(3,1,8,3,0),&[(*b"PLTE",vec![255,0,0,0,255,0,0,0,255]),(*b"tRNS",vec![0,128])],&[0,0,1,2]),3,1,&[0,0,0,0,0,128,0,128,255,0,0,255])}
pub fn png_decode_invalid_combos()->Check{for t in 0..=7{for d in [0,1,2,3,4,8,16,32]{let legal=match t{0=>[1,2,4,8,16].contains(&d),2|4|6=>[8,16].contains(&d),3=>[1,2,4,8].contains(&d),_=>false};if !legal{eq(png::decode(&pngfile(&ihdr(1,1,d,t,0),&[],&[0,0])),Err(PE::BadColourType))?;}}}Ok(())}
pub fn png_decode_rounding_16bit()->Check{let vals=[0,1,128,129,255,256,257,32767,32768,65406,65534,65535];let mut raw=vec![0];raw.extend(samples(&vals,16));let want:Vec<u8>=vals.iter().flat_map(|&v|{let c=scale(v,16);[c,c,c,255]}).collect();decoded(&pngfile(&ihdr(vals.len()as u32,1,16,0,0),&[],&raw),vals.len()as u32,1,&want)}
fn paeth(a:i32,b:i32,c:i32)->i32{let p=a+b-c;let(pa,pb,pc)=((p-a).abs(),(p-b).abs(),(p-c).abs());if pa<=pb&&pa<=pc{a}else if pb<=pc{b}else{c}}
pub fn png_decode_filters()->Check{for(t,d,ch)in[(0,8,1),(0,16,1),(2,8,3),(2,16,3),(4,8,2),(4,16,2),(6,8,4),(6,16,4)]{let bpp=ch*(d/8)as usize;let row:Vec<u8>=(0..7*bpp).map(|i|(i*37+17)as u8).collect();let mut filtered=Vec::new();for f in 0..5{filtered.push(f);for i in 0..row.len(){let a=if i>=bpp{row[i-bpp]as i32}else{0};let b=if f==0{0}else{row[i]as i32};let c=if f==0||i<bpp{0}else{row[i-bpp]as i32};let p=match f{0=>0,1=>a,2=>b,3=>(a+b)/2,_=>paeth(a,b,c)};filtered.push(row[i].wrapping_sub(p as u8));}}let mut want=Vec::new();for _ in 0..5 { for x in 0..7 { let values:Vec<u16>=if d==16{row[x*bpp..(x+1)*bpp].chunks_exact(2).map(|v|u16::from_be_bytes([v[0],v[1]])).collect()}else{row[x*bpp..(x+1)*bpp].iter().map(|v|*v as u16).collect()};let (r,g,b,a)=match t {0=>(values[0],values[0],values[0],if d==16{65535}else{255}),2=>(values[0],values[1],values[2],if d==16{65535}else{255}),4=>(values[0],values[0],values[0],values[1]),_=>(values[0],values[1],values[2],values[3])};let a=scale(a,d);want.extend([pm(scale(b,d),a),pm(scale(g,d),a),pm(scale(r,d),a),a]); }}decoded(&pngfile(&ihdr(7,5,d,t,0),&[],&filtered),7,5,&want)?;}Ok(())}
pub fn png_decode_filter_invalid()->Check{eq(png::decode(&pngfile(&ihdr(1,1,8,0,0),&[],&[5,0])),Err(PE::BadFilter))}
fn scene(w:u32,h:u32)->Vec<u8>{(0..h).flat_map(|y|(0..w).flat_map(move|x|[(x*23)as u8,(y*19)as u8,((x+y)*7)as u8,255])).collect()}
fn interlaced(w:u32,h:u32)->Vec<u8>{let pix=scene(w,h);let mut raw=Vec::new();for(x0,y0,dx,dy)in[(0,0,8,8),(4,0,8,8),(0,4,4,8),(2,0,4,4),(0,2,2,4),(1,0,2,2),(0,1,1,2)]{if x0>=w{continue;}for y in(y0..h).step_by(dy){raw.push(0);for x in(x0..w).step_by(dx){let i=((y*w+x)*4)as usize;raw.extend([pix[i+2],pix[i+1],pix[i],255]);}}}pngfile(&ihdr(w,h,8,6,1),&[],&raw)}
pub fn png_decode_interlace()->Check{decoded(&interlaced(17,19),17,19,&scene(17,19))}
pub fn png_decode_interlace_small()->Check{for w in 1..=9{for h in 1..=9{decoded(&interlaced(w,h),w,h,&scene(w,h))?;}}Ok(())}
pub fn png_decode_multi_idat()->Check{let mut v=b"\x89PNG\r\n\x1a\n".to_vec();chunk(&mut v,b"IHDR",&ihdr(1,1,8,0,0));for b in zstored(&[0,42]){chunk(&mut v,b"IDAT",&[]);chunk(&mut v,b"IDAT",&[b]);}chunk(&mut v,b"IEND",&[]);decoded(&v,1,1,&[42,42,42,255])}
pub fn png_decode_crc()->Check{let mut v=pngfile(&ihdr(1,1,8,0,0),&[],&[0,42]);v[29]^=1;eq(png::decode(&v),Err(PE::BadCrc))}
pub fn png_decode_chunk_order()->Check{let h=ihdr(1,1,8,0,0);let mut v=b"\x89PNG\r\n\x1a\n".to_vec();chunk(&mut v,b"IDAT",&zstored(&[0,0]));chunk(&mut v,b"IHDR",&h);eq(png::decode(&v),Err(PE::ChunkOrder))?;let mut v=pngfile(&h,&[],&[0,42]);v.truncate(v.len()-12);eq(png::decode(&v),Err(PE::ChunkOrder))?;chunk(&mut v,b"PLTE",&[0,0,0]);chunk(&mut v,b"IEND",&[]);eq(png::decode(&v),Err(PE::ChunkOrder))}
pub fn png_decode_plte_required()->Check{eq(png::decode(&pngfile(&ihdr(1,1,8,3,0),&[],&[0,0])),Err(PE::MissingPlte))}
pub fn png_decode_plte_size()->Check{for p in [vec![0,0],vec![0;9]]{eq(png::decode(&pngfile(&ihdr(1,1,1,3,0),&[(*b"PLTE",p)],&[0,0])),Err(PE::BadPlte))?;}Ok(())}
pub fn png_decode_palette_index()->Check{eq(png::decode(&pngfile(&ihdr(1,1,8,3,0),&[(*b"PLTE",vec![0,0,0])],&[0,1])),Err(PE::BadPaletteIndex))}
pub fn png_decode_critical_unknown()->Check{eq(png::decode(&pngfile(&ihdr(1,1,8,0,0),&[(*b"ABCD",vec![])],&[0,42])),Err(PE::UnknownCritical))}
pub fn png_decode_ancillary_ignored()->Check{let before=[(*b"gAMA",vec![0,0,177,143]),(*b"iCCP",vec![0,0]),(*b"sRGB",vec![0]),(*b"tEXt",b"name\0test".to_vec())];decoded(&pngfile(&ihdr(1,1,8,0,0),&before,&[0,42]),1,1,&[42,42,42,255])}
pub fn png_decode_ihdr_invalid()->Check{for(w,h)in[(0,1),(1,0)]{eq(png::decode(&pngfile(&ihdr(w,h,8,0,0),&[],&[])),Err(PE::BadHeader))?;}for(i,x)in[(10,1),(11,1),(12,2)]{let mut h=ihdr(1,1,8,0,0);h[i]=x;eq(png::decode(&pngfile(&h,&[],&[0,42])),Err(PE::BadHeader))?;}Ok(())}
pub fn png_decode_chunk_length()->Check{let mut v=b"\x89PNG\r\n\x1a\n".to_vec();v.extend(0x8000_0000u32.to_be_bytes());v.extend(b"IHDR");eq(png::decode(&v),Err(PE::BadChunkLength))}
pub fn png_decode_huge()->Check{for(w,h)in[(16385,1),(1,16385),(4097,4097)]{eq(png::decode(&pngfile(&ihdr(w,h,8,0,0),&[],&[])),Err(PE::TooLarge))?;}Ok(())}
pub fn png_decode_bomb()->Check{eq(png::decode(&pngfile(&ihdr(1,1,8,0,0),&[],&[0,1,2])),Err(PE::Bomb))}
pub fn png_decode_truncated()->Check{let v=pngfile(&ihdr(1,1,8,0,0),&[],&[0,42]);for cut in 0..v.len(){let result=png::decode(&v[..cut]);if result.is_ok(){return Err(format!("accepted PNG prefix {cut}/{}",v.len()));}}Ok(())}
pub fn png_decode_signature()->Check{eq(png::decode(b"not a PNG"),Err(PE::BadSignature))}
fn image(w:u32,h:u32)->Image{Image{width:w,height:h,pixels:scene(w,h)}}
pub fn png_encode_decodes()->Check{for pixels in [vec![3,2,1,255,128,0,128,128],vec![3,2,1,255,30,20,10,255]]{let i=Image{width:2,height:1,pixels};let bytes=png::encode(&i).map_err(|e|format!("encode {e:?}"))?;eq(png::decode(&bytes),Ok(i))?;}Ok(())}
pub fn png_encode_stable()->Check{let i=image(13,7);eq(png::encode(&i),png::encode(&i))}
fn encoded_chunks(v:&[u8])->Result<Vec<([u8;4],Vec<u8>,u32)>,String>{if !v.starts_with(b"\x89PNG\r\n\x1a\n"){return Err("encoder signature".into());}let mut o=8;let mut chunks=Vec::new();while o<v.len(){if o+12>v.len(){return Err("encoder truncated chunk".into());}let n=u32::from_be_bytes(v[o..o+4].try_into().unwrap())as usize;if o+12+n>v.len(){return Err("encoder chunk length".into());}chunks.push((v[o+4..o+8].try_into().unwrap(),v[o+8..o+8+n].to_vec(),u32::from_be_bytes(v[o+8+n..o+12+n].try_into().unwrap())));o+=12+n;}Ok(chunks)}
pub fn png_encode_format()->Check{for(alpha,t)in[(255,2),(128,6)]{let i=Image{width:1,height:1,pixels:vec![0,0,alpha,alpha]};let v=png::encode(&i).map_err(|e|format!("encode {e:?}"))?;let c=encoded_chunks(&v)?;eq(c.iter().map(|x|x.0).collect::<Vec<_>>(),vec![*b"IHDR",*b"IDAT",*b"IEND"])?;eq(c[0].1.clone(),ihdr(1,1,8,t,0))?;let z=&c[1].1;let kind=(z[2]>>1)&3;if kind>1{return Err(format!("encoder block type {kind}"));}let raw=zlib_decompress(z,5).map_err(|e|format!("encoder zlib {e:?}"))?;eq(raw,if t==2{vec![0,255,0,0]}else{vec![0,255,0,0,128]})?;}Ok(())}
pub fn png_encode_chunk_crc()->Check{eq(crc(b"123456789"),0xcbf4_3926)?;let v=png::encode(&image(3,2)).map_err(|e|format!("encode {e:?}"))?;for(n,d,c)in encoded_chunks(&v)?{let mut b=n.to_vec();b.extend(d);eq(c,crc(&b))?;}Ok(())}
pub fn png_encode_zlib_adler()->Check{eq(adler(b"Wikipedia"),0x11e6_0398)?;let v=png::encode(&image(3,2)).map_err(|e|format!("encode {e:?}"))?;let c=encoded_chunks(&v)?;let z=&c[1].1;let raw=zlib_decompress(z,100).map_err(|e|format!("zlib {e:?}"))?;eq(u32::from_be_bytes(z[z.len()-4..].try_into().unwrap()),adler(&raw))}
pub fn png_encode_sizes()->Check{for(w,h)in[(1,1),(3,7),(17,9),(1280,800)]{let i=image(w,h);let v=png::encode(&i).map_err(|e|format!("encode {e:?}"))?;eq(png::decode(&v),Ok(i))?;}Ok(())}
pub fn png_encode_limits()->Check{for(w,h)in[(16385,1),(1,16385),(4097,4097)]{eq(png::encode(&Image{width:w,height:h,pixels:vec![]}),Err(PE::TooLarge))?;}Ok(())}

pub const CHECKS: &[(&str,fn()->Check)] = &[
    ("bmp_unsupported",bmp_unsupported),
    ("inflate_independent_fixtures",inflate_independent_fixtures),
    ("png_decode_independent_fixtures",png_decode_independent_fixtures),
    ("png_decode_from_disk",png_decode_from_disk),
    ("inflate_stored", inflate_stored),
    ("inflate_stored_length", inflate_stored_length),
    ("inflate_fixed_huffman", inflate_fixed_huffman),
    ("inflate_dynamic_huffman", inflate_dynamic_huffman),
    ("inflate_block_sequence", inflate_block_sequence),
    ("inflate_block_type_invalid", inflate_block_type_invalid),
    ("inflate_code_oversubscribed", inflate_code_oversubscribed),
    ("inflate_code_incomplete", inflate_code_incomplete),
    ("inflate_code_single", inflate_code_single),
    ("inflate_no_distance_codes", inflate_no_distance_codes),
    ("inflate_repeat_first", inflate_repeat_first),
    ("inflate_symbols_invalid", inflate_symbols_invalid),
    ("inflate_distance_too_far", inflate_distance_too_far),
    ("inflate_distance_max", inflate_distance_max),
    ("inflate_length_258", inflate_length_258),
    ("inflate_truncated", inflate_truncated),
    ("inflate_zlib_header", inflate_zlib_header),
    ("inflate_adler32", inflate_adler32),
    ("inflate_output_limit", inflate_output_limit),
    ("inflate_byte_split", inflate_byte_split),
    ("png_decode_gray_depths", png_decode_gray_depths),
    ("png_decode_gray_alpha", png_decode_gray_alpha),
    ("png_decode_rgb", png_decode_rgb),
    ("png_decode_rgba", png_decode_rgba),
    ("png_decode_palette_depths", png_decode_palette_depths),
    ("png_decode_trns_gray", png_decode_trns_gray),
    ("png_decode_trns_rgb", png_decode_trns_rgb),
    ("png_decode_trns_palette", png_decode_trns_palette),
    ("png_decode_invalid_combos", png_decode_invalid_combos),
    ("png_decode_rounding_16bit", png_decode_rounding_16bit),
    ("png_decode_filters", png_decode_filters),
    ("png_decode_filter_invalid", png_decode_filter_invalid),
    ("png_decode_interlace", png_decode_interlace),
    ("png_decode_interlace_small", png_decode_interlace_small),
    ("png_decode_multi_idat", png_decode_multi_idat),
    ("png_decode_crc", png_decode_crc),
    ("png_decode_chunk_order", png_decode_chunk_order),
    ("png_decode_plte_required", png_decode_plte_required),
    ("png_decode_plte_size", png_decode_plte_size),
    ("png_decode_palette_index", png_decode_palette_index),
    ("png_decode_critical_unknown", png_decode_critical_unknown),
    ("png_decode_ancillary_ignored", png_decode_ancillary_ignored),
    ("png_decode_ihdr_invalid", png_decode_ihdr_invalid),
    ("png_decode_chunk_length", png_decode_chunk_length),
    ("png_decode_huge", png_decode_huge),
    ("png_decode_bomb", png_decode_bomb),
    ("png_decode_truncated", png_decode_truncated),
    ("png_decode_signature", png_decode_signature),
    ("png_encode_decodes", png_encode_decodes),
    ("png_encode_stable", png_encode_stable),
    ("png_encode_format", png_encode_format),
    ("png_encode_chunk_crc", png_encode_chunk_crc),
    ("png_encode_zlib_adler", png_encode_zlib_adler),
    ("png_encode_sizes", png_encode_sizes),
    ("png_encode_limits", png_encode_limits),
];

fn fixture_root()->std::path::PathBuf{
    if cfg!(target_os="none"){std::path::PathBuf::from("/usr/local/test/share/breengel")}
    else {std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"),"/../../testdata/breengel"))}
}
pub fn inflate_independent_fixtures()->Check{
    let root=fixture_root().join("inflate");let index=std::fs::read_to_string(root.join("index.txt")).map_err(|e|format!("inflate fixture index: {e}"))?;
    let mut count=0;for line in index.lines(){let f:Vec<_>=line.split_whitespace().collect();if f.len()!=2{return Err("invalid inflate fixture index".into());}let input=std::fs::read(root.join(f[0])).map_err(|e|e.to_string())?;let expected=std::fs::read(root.join(f[1])).map_err(|e|e.to_string())?;eq(inflate(&input,expected.len()),Ok(expected))?;count+=1;}eq(count,12)
}
pub fn png_decode_independent_fixtures()->Check{
    let root=fixture_root().join("png");let index=std::fs::read_to_string(root.join("index.txt")).map_err(|e|format!("PNG fixture index: {e}"))?;
    let mut count=0;for line in index.lines(){let f:Vec<_>=line.split_whitespace().collect();if f.len()!=4{return Err("invalid PNG fixture index".into());}let input=std::fs::read(root.join(f[0])).map_err(|e|e.to_string())?;let expected=std::fs::read(root.join(f[3])).map_err(|e|e.to_string())?;decoded(&input,f[1].parse().map_err(|_|"fixture width")?,f[2].parse().map_err(|_|"fixture height")?,&expected).map_err(|e|format!("{}: {e}",f[0]))?;count+=1;}eq(count,72)
}
pub fn png_decode_from_disk()->Check{let root=fixture_root().join("png");let input=std::fs::read(root.join("pinned.png")).map_err(|e|format!("pinned PNG: {e}"))?;let expected=std::fs::read(root.join("pinned.bgra")).map_err(|e|e.to_string())?;decoded(&input,32,32,&expected)}

pub fn bmp_unsupported()->Check {
 for (bpp,compression) in [(8u16,0u32),(32,0),(24,1),(16,3)] {
  let mut b=vec![0;118];b[..2].copy_from_slice(b"BM");b[10..14].copy_from_slice(&54u32.to_le_bytes());b[14..18].copy_from_slice(&40u32.to_le_bytes());b[18..22].copy_from_slice(&2u32.to_le_bytes());b[22..26].copy_from_slice(&2u32.to_le_bytes());b[26..28].copy_from_slice(&1u16.to_le_bytes());b[28..30].copy_from_slice(&bpp.to_le_bytes());b[30..34].copy_from_slice(&compression.to_le_bytes());
  eq(libimg::bmp::decode_bmp(&b),Err(libimg::bmp::BmpError::Unsupported{bpp,compression}))?;
 }Ok(())
}
