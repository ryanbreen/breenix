
macro_rules! println {
    ($fmt:expr) => (print!(concat!("{:?} - ", $fmt, "\n"), $crate::io::timer::time_since_start()));
    ($fmt:expr, $($arg:tt)*) =>
      (print!(concat!("{:?} - ", $fmt, "\n"), $crate::io::timer::time_since_start(), $($arg)*));
}

macro_rules! print {
   ($($arg:tt)*) => ({
     use core::fmt::Write;
     $crate::writers::print(format_args!($($arg)*));
     //$crate::writers::VGA_WRITER.lock().write_fmt(format_args!($($arg)*)).unwrap();
   });
}

macro_rules! format {
  ($($arg:tt)*) => ({
    use collections::string::String;
    use core::fmt;
    let mut output = String::new();
    fmt::write(&mut output, format_args!($($arg)*));
    output
  });
}

macro_rules! debug {
  ($($arg:tt)*) => ({
    $crate::debug::debug();
  });
}
