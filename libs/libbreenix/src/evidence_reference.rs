//! Same-boot references to completed picture parts; image encodings stay unchanged.

struct Part { index: usize, bytes: usize, digest: [u8;32] }

/// A bounded sender-side index. Entries become eligible only after HTTP acceptance.
pub struct Cache { parts: Vec<Part> }
impl Cache {
    pub const fn new() -> Self { Self { parts: Vec::new() } }
    pub fn reference(&self, index: usize, bytes: usize, digest: [u8;32]) -> Option<String> {
        let part=self.parts.iter().find(|p|p.index<index && p.bytes==bytes && p.digest==digest)?;
        let hash:String=digest.iter().map(|b|format!("{b:02x}")).collect();
        let text=format!("PICTURE_REF v=1 source={} bytes={bytes} sha256={hash}\n",part.index);
        (text.len()<bytes).then_some(text)
    }
    pub fn remember(&mut self, index: usize, bytes: usize, digest: [u8;32]) -> Result<(), &'static str> {
        if bytes==0 || bytes>2<<20 || self.parts.len()>=16384
            || self.parts.last().is_some_and(|p|p.index>=index) {
            return Err("invalid completed picture part");
        }
        self.parts.push(Part { index, bytes, digest });Ok(())
    }
}
impl Default for Cache { fn default() -> Self { Self::new() } }

/// Preserve literal HTTP acceptance and require one explicit acknowledgement for a reference.
pub fn accepted(response: &[u8], reference: bool) -> bool {
    if !response.starts_with(b"HTTP/1.1 200 ") { return false; }
    let Some(end)=response.windows(4).position(|w|w==b"\r\n\r\n") else { return false; };
    if !reference { return true; }
    let Ok(headers)=std::str::from_utf8(&response[..end]) else { return false; };
    let mut count=0;
    for line in headers.lines() {
        if let Some((name,value))=line.split_once(':') {
            if name.eq_ignore_ascii_case("X-Breenix-Picture-Reference-Accepted") {
                if value.trim()!="1" { return false; }
                count+=1;
            }
        }
    }
    count==1
}
