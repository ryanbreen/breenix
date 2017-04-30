
#[inline(never)]
pub fn handle(num:u64, a:u64, b:u64, c:u64, d:u64, e:u64, f:u64) -> u64 {

  match num {
    16 => test(a, b, c, d, e, f),
    _ => err(num, a, b, c, d, e, f),
  }
}

fn test(a:u64, b:u64, c:u64, d:u64, e:u64, f:u64) -> u64 {
  a + b + c + d + e + f
}

fn err(num: u64, a:u64, b:u64, c:u64, d:u64, e:u64, f:u64) -> u64 {
  println!("Unknown syscall of type {}", num);
  0
}