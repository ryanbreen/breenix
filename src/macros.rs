
macro_rules! get_current_pid {
  ($($arg:tt)*) => ({
    $crate::state().scheduler.current
  });
}

macro_rules! bootstrap_println {
    ($fmt:expr) => (print!(concat!("[0] {:?} - ", $fmt, "\n"), $crate::io::timer::time_since_start()));
    ($fmt:expr, $($arg:tt)*) =>
      (print!(concat!("[0] {:?} - ", $fmt, "\n"), $crate::io::timer::time_since_start(), $($arg)*));
}

macro_rules! println {
    ($fmt:expr) => ({
      print!(concat!("[{}] {:?} - ", $fmt, "\n"), get_current_pid!(), $crate::io::timer::real_time());
    });
    ($fmt:expr, $($arg:tt)*) => ({
      print!(concat!("[{}] {:?} - ", $fmt, "\n"), get_current_pid!(), $crate::io::timer::real_time(), $($arg)*);
    });
}

macro_rules! print {
   ($($arg:tt)*) => ({
      $crate::io::drivers::display::text_buffer::print(format_args!($($arg)*));
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
