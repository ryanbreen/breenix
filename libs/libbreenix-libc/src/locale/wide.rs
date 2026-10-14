use core::ptr::null_mut;
use super::{classify, set_errno};
#[no_mangle]
pub unsafe extern "C" fn wcslen(s: *const u32) -> usize { wcsnlen(s,usize::MAX) }
#[no_mangle]
pub unsafe extern "C" fn wcsnlen(s: *const u32, n: usize) -> usize {
    let mut i = 0; while i<n && *s.add(i)!=0 { i+=1; } i
}
#[no_mangle]
pub unsafe extern "C" fn wmemcpy(d: *mut u32, s: *const u32, n: usize) -> *mut u32 {
    core::ptr::copy_nonoverlapping(s,d,n); d
}
#[no_mangle]
pub unsafe extern "C" fn wmemmove(d: *mut u32, s: *const u32, n: usize) -> *mut u32 { core::ptr::copy(s,d,n); d }
#[no_mangle]
pub unsafe extern "C" fn wmemset(d: *mut u32, c: u32, n: usize) -> *mut u32 { for i in 0..n { *d.add(i)=c; } d }
#[no_mangle]
pub unsafe extern "C" fn wmemchr(s: *const u32, c: u32, n: usize) -> *mut u32 {
    for i in 0..n { if *s.add(i)==c { return s.add(i) as *mut u32; } } null_mut()
}
fn compare(a: u32,b: u32) -> i32 {
    let order = if cfg!(target_arch="aarch64") { a.cmp(&b) } else { (a as i32).cmp(&(b as i32)) };
    match order { core::cmp::Ordering::Less => -1, core::cmp::Ordering::Equal => 0, core::cmp::Ordering::Greater => 1 }
}
#[no_mangle]
pub unsafe extern "C" fn wmemcmp(a: *const u32,b: *const u32,n: usize) -> i32 {
    for i in 0..n { let r=compare(*a.add(i),*b.add(i)); if r!=0 {return r;} } 0
}
#[no_mangle]
pub unsafe extern "C" fn wcscpy(d: *mut u32,s: *const u32) -> *mut u32 { wmemcpy(d,s,wcslen(s)+1) }
#[no_mangle]
pub unsafe extern "C" fn wcsncpy(d: *mut u32,s: *const u32,n: usize) -> *mut u32 {
    let count=wcsnlen(s,n); wmemcpy(d,s,count); wmemset(d.add(count),0,n-count); d
}
#[no_mangle]
pub unsafe extern "C" fn wcscat(d: *mut u32,s: *const u32) -> *mut u32 { wcscpy(d.add(wcslen(d)),s); d }
#[no_mangle]
pub unsafe extern "C" fn wcsncat(d: *mut u32,s: *const u32,n: usize) -> *mut u32 {
    let end=d.add(wcslen(d)); let count=wcsnlen(s,n); wmemcpy(end,s,count); *end.add(count)=0; d
}
#[no_mangle]
pub unsafe extern "C" fn wcsncmp(a: *const u32,b: *const u32,n: usize) -> i32 {
    for i in 0..n { let r=compare(*a.add(i),*b.add(i)); if r!=0 {return r;} if *a.add(i)==0 {break;} } 0
}
#[no_mangle]
pub unsafe extern "C" fn wcscmp(a: *const u32,b: *const u32) -> i32 { wcsncmp(a,b,usize::MAX) }
#[no_mangle]
pub unsafe extern "C" fn wcscoll(a: *const u32,b: *const u32) -> i32 { wcscmp(a,b) }
#[no_mangle]
pub unsafe extern "C" fn wcsxfrm(d: *mut u32,s: *const u32,n: usize) -> usize { let len=wcslen(s); if n!=0 { wcsncpy(d,s,n); } len }
#[no_mangle]
pub unsafe extern "C" fn wcschr(s: *const u32,c: u32) -> *mut u32 {
    let mut i=0; loop { if *s.add(i)==c {return s.add(i) as *mut u32;} if *s.add(i)==0 {return null_mut();} i+=1; }
}
#[no_mangle]
pub unsafe extern "C" fn wcsrchr(s: *const u32,c: u32) -> *mut u32 {
    let mut p=null_mut(); for i in 0..=wcslen(s) { if *s.add(i)==c {p=s.add(i) as *mut u32;} } p
}
#[no_mangle]
pub unsafe extern "C" fn wcsstr(s: *const u32,t: *const u32) -> *mut u32 {
    let n=wcslen(t); let len=wcslen(s); if n>len {return null_mut();}
    for i in 0..=len-n {if wmemcmp(s.add(i),t,n)==0 {return s.add(i) as *mut u32;}} null_mut()
}
#[no_mangle]
pub unsafe extern "C" fn wcsspn(s: *const u32,set: *const u32) -> usize {
    let mut i=0; while *s.add(i)!=0 && !wcschr(set,*s.add(i)).is_null() {i+=1;} i
}
#[no_mangle]
pub unsafe extern "C" fn wcscspn(s: *const u32,set: *const u32) -> usize {
    let mut i=0; while *s.add(i)!=0 && wcschr(set,*s.add(i)).is_null() {i+=1;} i
}
#[no_mangle]
pub unsafe extern "C" fn wcspbrk(s: *const u32,set: *const u32) -> *mut u32 {
    let p=s.add(wcscspn(s,set)); if *p==0 {null_mut()} else {p as *mut u32}
}
#[no_mangle]
pub unsafe extern "C" fn wcstok(s: *mut u32,set: *const u32,save: *mut *mut u32) -> *mut u32 {
    let s=if s.is_null() {*save} else {s}; if s.is_null() {return null_mut();}
    let p=s.add(wcsspn(s,set)); if *p==0 {*save=null_mut();return null_mut();}
    let end=p.add(wcscspn(p,set)); if *end==0 {*save=null_mut();} else {*end=0;*save=end.add(1);} p
}
#[no_mangle]
pub unsafe extern "C" fn wcsdup(s: *const u32) -> *mut u32 {
    let n=wcslen(s)+1; let Some(size)=n.checked_mul(4) else {set_errno(12);return null_mut();};
    let d=crate::malloc(size) as *mut u32; if d.is_null() {set_errno(12);return d;} wmemcpy(d,s,n)
}
#[no_mangle]
pub unsafe extern "C" fn wcsncasecmp(a: *const u32,b: *const u32,n: usize) -> i32 {
    for i in 0..n {let r=compare(classify::towlower(*a.add(i)),classify::towlower(*b.add(i))); if r!=0 {return r;} if *a.add(i)==0 {break;}} 0
}
#[no_mangle]
pub unsafe extern "C" fn wcscasecmp(a: *const u32,b: *const u32) -> i32 {wcsncasecmp(a,b,usize::MAX)}
