//! Gregorian calendar arithmetic derived from the 4/100/400-year leap rule.
//! Epoch is 1970-01-01; supported TZ settings are UTC and POSIX fixed offsets.
use super::set_errno;
use core::ptr::{null_mut,addr_of_mut};
#[derive(Clone,Copy,Debug,Default)]
#[repr(C)]
pub struct Tm {
    pub sec:i32,pub min:i32,pub hour:i32,pub mday:i32,pub mon:i32,pub year:i32,
    pub wday:i32,pub yday:i32,pub isdst:i32,pub gmtoff:i64,pub zone:*const u8,
}
#[repr(C)]
#[derive(Default)]
pub struct Timespec { pub sec:i64,pub nsec:i64 }
static mut RESULT: Tm = Tm {sec:0,min:0,hour:0,mday:0,mon:0,year:0,wday:0,yday:0,isdst:0,gmtoff:0,zone:core::ptr::null()};
static mut TEXT: [u8;26]=[0;26];
static mut ZONE: [u8;64]=[0;64];
#[no_mangle]
pub static mut timezone:i64=0;
#[no_mangle]
pub static mut daylight:i32=0;
#[no_mangle]
pub static mut tzname:[*const u8;2]=[b"UTC\0".as_ptr();2];
fn leap(y:i64)->bool { y.rem_euclid(4)==0 && (y.rem_euclid(100)!=0 || y.rem_euclid(400)==0) }
fn year_days(y:i64)->i64 {let p=y-1;365*p+p.div_euclid(4)-p.div_euclid(100)+p.div_euclid(400)}
fn month_days(y:i64,m:usize)->i64 { [31,if leap(y){29}else{28},31,30,31,30,31,31,30,31,30,31][m] }
fn day_number(y:i64,m:usize,d:i64)->i64 {
    year_days(y)-719162+(0..m).map(|i| month_days(y,i)).sum::<i64>()+d-1
}
fn broken(sec:i64,offset:i64,zone:*const u8)->Option<Tm> {
    let seconds=sec.checked_add(offset)?;
    let days=seconds.div_euclid(86400);
    let within=seconds.rem_euclid(86400);
    let day=days.checked_add(719162)?;
    let era=day.div_euclid(146097);
    let mut y=era*400+1;
    let mut yd=day.rem_euclid(146097);
    while yd >= if leap(y){366}else{365} {yd-=if leap(y){366}else{365};y+=1;}
    let year=i32::try_from(y-1900).ok()?;
    let yday=yd as i32;
    let mut mon=0;
    while yd>=month_days(y,mon) {yd-=month_days(y,mon);mon+=1;}
    Some(Tm {sec:(within%60)as i32,min:((within/60)%60)as i32,hour:(within/3600)as i32,
        mday:yd as i32+1,mon:mon as i32,year,wday:(days+4).rem_euclid(7)as i32,yday,isdst:0,gmtoff:offset,zone})
}
#[no_mangle]
pub unsafe extern "C" fn time(out:*mut i64)->i64 {
    let mut ts=Timespec::default();
    if crate::clock_gettime(0,&mut ts as *mut _ as *mut u8)!=0 {return -1;}
    if !out.is_null(){*out=ts.sec;} ts.sec
}
#[no_mangle]
pub extern "C" fn difftime(a:i64,b:i64)->f64 { (a as i128-b as i128) as f64 }
#[no_mangle]
pub unsafe extern "C" fn clock()->i64 {
    let mut ts=Timespec::default();
    if crate::clock_gettime(2,&mut ts as *mut _ as *mut u8)!=0 {return -1;}
    ts.sec.checked_mul(1_000_000).and_then(|n|n.checked_add(ts.nsec/1000)).unwrap_or(-1)
}
#[no_mangle]
pub unsafe extern "C" fn timespec_get(ts:*mut Timespec,base:i32)->i32 {
    if base!=1 || crate::clock_gettime(0,ts as *mut u8)!=0 {0}else{base}
}
#[no_mangle]
pub unsafe extern "C" fn gmtime_r(t:*const i64,out:*mut Tm)->*mut Tm {
    match broken(*t,0,b"UTC\0".as_ptr()) {Some(tm)=>{*out=tm;out},None=>{set_errno(75);null_mut()}}
}
#[no_mangle]
pub unsafe extern "C" fn gmtime(t:*const i64)->*mut Tm {gmtime_r(t,addr_of_mut!(RESULT))}
unsafe fn fixed_zone()->(i64,*const u8) {
    tzset(); (-timezone,tzname[0])
}
#[no_mangle]
pub unsafe extern "C" fn localtime_r(t:*const i64,out:*mut Tm)->*mut Tm {
    let (offset,zone)=fixed_zone();
    match broken(*t,offset,zone) {Some(tm)=>{*out=tm;out},None=>{set_errno(75);null_mut()}}
}
#[no_mangle]
pub unsafe extern "C" fn localtime(t:*const i64)->*mut Tm {localtime_r(t,addr_of_mut!(RESULT))}
unsafe fn make(tm:*mut Tm,offset:i64,zone:*const u8)->i64 {
    let t=&*tm;
    let year=t.year as i64+1900+(t.mon as i64).div_euclid(12);
    let mon=t.mon.rem_euclid(12)as usize;
    let seconds=day_number(year,mon,t.mday as i64)*86400+t.hour as i64*3600+t.min as i64*60+t.sec as i64-offset;
    match broken(seconds,offset,zone){Some(out)=>{*tm=out;seconds},None=>{set_errno(75);-1}}
}
#[no_mangle]
pub unsafe extern "C" fn timegm(tm:*mut Tm)->i64 {make(tm,0,b"UTC\0".as_ptr())}
#[no_mangle]
pub unsafe extern "C" fn mktime(tm:*mut Tm)->i64 {let(offset,zone)=fixed_zone();make(tm,offset,zone)}
#[no_mangle]
pub unsafe extern "C" fn tzset() {
    timezone=0;daylight=0;tzname=[b"UTC\0".as_ptr();2];
    let Some(b)=crate::env::getenv_bytes(b"TZ") else {return;};
    if b.len()>128 {return;}
    let mut i=0;
    let (start,end)=if b.first()==Some(&b'<') {
        i=1;while i<b.len() && b[i]!=b'>' {i+=1;}
        if i==b.len(){return;} let end=i;i+=1;(1,end)
    } else {
        while i<b.len() && b[i].is_ascii_alphabetic(){i+=1;} (0,i)
    };
    if end-start<3 || end-start>=64 {return;}
    let mut sign=1;
    if b.get(i)==Some(&b'-'){sign=-1;i+=1;}else if b.get(i)==Some(&b'+'){i+=1;}
    let mut part=[0i64;3];
    for field in 0..3 {
        let begin=i;
        while i<b.len() && b[i].is_ascii_digit(){part[field]=part[field]*10+(b[i]-b'0')as i64;i+=1;if part[field]>167{return;}}
        if i==begin && (field!=0 || i!=b.len()){return;}
        if b.get(i)!=Some(&b':'){break;} i+=1;
    }
    if i!=b.len() || part[0]>24 || part[1]>59 || part[2]>59{return;}
    let zone=&mut *addr_of_mut!(ZONE);
    zone[..end-start].copy_from_slice(&b[start..end]);zone[end-start]=0;
    timezone=sign*(part[0]*3600+part[1]*60+part[2]);tzname=[zone.as_ptr();2];
}
const DAYS:[&[u8];7]=[b"Sunday",b"Monday",b"Tuesday",b"Wednesday",b"Thursday",b"Friday",b"Saturday"];
const MONTHS:[&[u8];12]=[b"January",b"February",b"March",b"April",b"May",b"June",b"July",b"August",b"September",b"October",b"November",b"December"];
pub(super) fn day_name(i:usize,full:bool)->&'static [u8] {let s=DAYS[i%7];if full{s}else{&s[..3]}}
pub(super) fn month_name(i:usize,full:bool)->&'static [u8] {let s=MONTHS[i%12];if full{s}else{&s[..3]}}
struct Output { narrow:*mut u8,wide:*mut u32,cap:usize,n:usize }
impl Output {
    unsafe fn put(&mut self,c:u8)->bool {
        if self.n>=self.cap {return false;}
        if !self.narrow.is_null(){*self.narrow.add(self.n)=c;}else{*self.wide.add(self.n)=c as u32;}
        self.n+=1;true
    }
    unsafe fn text(&mut self,s:&[u8])->bool {for &c in s {if !self.put(c){return false;}}true}
    unsafe fn number(&mut self,n:i64,width:usize,pad:u8)->bool {
        let mut buf=[0u8;24];let mut i=buf.len();let mut v=n.unsigned_abs();
        loop {i-=1;buf[i]=b'0'+(v%10)as u8;v/=10;if v==0{break;}}
        let sign=usize::from(n<0);let len=buf.len()-i+sign;
        if n<0 && pad==b'0' && !self.put(b'-'){return false;}
        for _ in len..width {if !self.put(pad){return false;}}
        if n<0 && pad!=b'0' && !self.put(b'-'){return false;}
        self.text(&buf[i..])
    }
}
fn iso_week(t:&Tm)->(i64,i64) {
    let y=t.year as i64+1900;
    let weekday=if t.wday==0{7}else{t.wday}as i64;
    let thursday=t.yday as i64+4-weekday;
    if thursday<0 {let last=broken(day_number(y,0,1)*86400-86400,0,core::ptr::null()).unwrap();return iso_week(&last);}
    let length=if leap(y){366}else{365};
    if thursday>=length {(y+1,1)}else{(y,thursday/7+1)}
}
unsafe fn format(out:&mut Output,fmt:*const u8,t:&Tm)->bool {
    let mut i=0;
    while *fmt.add(i)!=0 {
        let c=*fmt.add(i);i+=1;
        if c!=b'%' {if !out.put(c){return false;}continue;}
        let mut c=*fmt.add(i);if c==0{return false;}i+=1;
        let mut pad=b'0';let mut width:usize=0;
        if c==b'_'||c==b'0'||c==b'-' {pad=if c==b'_'{b' '}else if c==b'-'{0}else{b'0'};c=*fmt.add(i);i+=1;}
        while c.is_ascii_digit(){width=width.saturating_mul(10).saturating_add((c-b'0')as usize);if width>4096{return false;}c=*fmt.add(i);i+=1;}
        if c==b'E'||c==b'O'{c=*fmt.add(i);i+=1;}
        let y=t.year as i64+1900;
        let numeric=match c {
            b'd'|b'e'=>Some((t.mday as i64,2)),b'H'=>Some((t.hour as i64,2)),b'I'=>Some(((t.hour+11).rem_euclid(12)as i64+1,2)),
            b'j'=>Some((t.yday as i64+1,3)),b'm'=>Some((t.mon as i64+1,2)),b'M'=>Some((t.min as i64,2)),b'S'=>Some((t.sec as i64,2)),
            b'y'=>Some((y.rem_euclid(100),2)),b'Y'=>Some((y,4)),b'C'=>Some((y.div_euclid(100),2)),b'w'=>Some((t.wday as i64,1)),
            b'u'=>Some((if t.wday==0{7}else{t.wday}as i64,1)),b'U'=>Some(((t.yday+7-t.wday)as i64/7,2)),
            b'W'=>Some(((t.yday+7-(t.wday+6)%7)as i64/7,2)),b'G'=>Some((iso_week(t).0,4)),b'g'=>Some((iso_week(t).0.rem_euclid(100),2)),
            b'V'=>Some((iso_week(t).1,2)),_=>None,
        };
        if let Some((v,w))=numeric {if c==b'e' && pad==b'0'{pad=b' ';}if !out.number(v,if pad==0{0}else if width!=0{width}else{w},pad){return false;}continue;}
        let composite:&[u8]=match c {b'c'=>b"%a %b %e %H:%M:%S %Y\0",b'D'|b'x'=>b"%m/%d/%y\0",b'F'=>b"%Y-%m-%d\0",b'r'=>b"%I:%M:%S %p\0",b'R'=>b"%H:%M\0",b'T'|b'X'=>b"%H:%M:%S\0",_=>b"\0"};
        if composite.len()>1 {if !format(out,composite.as_ptr(),t){return false;}continue;}
        let okay=match c {
            b'a'|b'A'=>out.text(day_name(t.wday.rem_euclid(7)as usize,c==b'A')),
            b'b'|b'h'|b'B'=>out.text(month_name(t.mon.rem_euclid(12)as usize,c==b'B')),
            b'p'=>out.text(if t.hour<12{b"AM"}else{b"PM"}),b'n'=>out.put(b'\n'),b't'=>out.put(b'\t'),b'%'=>out.put(b'%'),
            b'Z'=>{let mut n=0;if !t.zone.is_null(){while *t.zone.add(n)!=0{n+=1;}out.text(core::slice::from_raw_parts(t.zone,n))}else{true}},
            b'z'=>{let v=t.gmtoff;out.put(if v<0{b'-'}else{b'+'}) && out.number(v.abs()/3600,2,b'0') && out.number((v.abs()/60)%60,2,b'0')},
            b's'=>{let mut copy=*t;out.number(make(&mut copy,t.gmtoff,t.zone),0,b'0')},_=>false,
        };
        if !okay{return false;}
    } true
}
#[no_mangle]
pub unsafe extern "C" fn strftime(s:*mut u8,n:usize,fmt:*const u8,t:*const Tm)->usize {
    let mut out=Output{narrow:s,wide:null_mut(),cap:n,n:0};
    if !format(&mut out,fmt,&*t) || !out.put(0){return 0;}out.n-1
}
#[no_mangle]
pub unsafe extern "C" fn wcsftime(s:*mut u32,n:usize,fmt:*const u32,t:*const Tm)->usize {
    // Calendar names/formats in our supported locales are ASCII; preserve all
    // non-format wide literals without transcoding or allocating.
    let mut out=Output{narrow:null_mut(),wide:s,cap:n,n:0};let mut i=0;
    while *fmt.add(i)!=0 {
        if *fmt.add(i)!=b'%' as u32 {if out.n>=n{return 0;}*s.add(out.n)=*fmt.add(i);out.n+=1;i+=1;continue;}
        let mut spec=[0u8;32];let mut k=0;
        loop {let c=*fmt.add(i);if c>127||c==0||k>=31{return 0;}spec[k]=c as u8;k+=1;i+=1;
            if k>1 && c!=b'E' as u32 && c!=b'O' as u32 && c!=b'_' as u32 && c!=b'-' as u32 && !(c as u8).is_ascii_digit(){break;}
        }
        if !format(&mut out,spec.as_ptr(),&*t){return 0;}
    }
    if !out.put(0){0}else{out.n-1}
}
#[no_mangle]
pub unsafe extern "C" fn asctime_r(t:*const Tm,s:*mut u8)->*mut u8 {
    if !(-1900..=8099).contains(&(*t).year){set_errno(75);return null_mut();}
    if strftime(s,26,b"%a %b %e %H:%M:%S %Y\n\0".as_ptr(),t)!=25{set_errno(75);return null_mut();}s
}
#[no_mangle]
pub unsafe extern "C" fn asctime(t:*const Tm)->*mut u8 {asctime_r(t,addr_of_mut!(TEXT) as *mut u8)}
#[no_mangle]
pub unsafe extern "C" fn ctime_r(t:*const i64,s:*mut u8)->*mut u8 {let mut tm=Tm::default();if localtime_r(t,&mut tm).is_null(){null_mut()}else{asctime_r(&tm,s)}}
#[no_mangle]
pub unsafe extern "C" fn ctime(t:*const i64)->*mut u8 {ctime_r(t,addr_of_mut!(TEXT) as *mut u8)}
