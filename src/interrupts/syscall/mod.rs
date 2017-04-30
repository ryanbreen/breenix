
pub fn handle(num:u64, a:u64, b:u64, c:u64, d:u64, e:u64, f:u64) -> u64 {

  let res = match num {
    16 => a + b + c + d + e + f,
    _ => 0
  };

  println!("syscall params {} {} {} {} {} {} {}, res {}", num, a, b, c, d, e, f, res);

  res
}