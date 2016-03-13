
macro_rules! println {
    ($fmt:expr) => (print!(concat!($fmt, "\n")));
    ($fmt:expr, $($arg:tt)*) => (print!(concat!($fmt, "\n"), $($arg)*));
}

macro_rules! print {
  ($($arg:tt)*) => ({
    use core::fmt::Write;
    $crate::vga_buffer::PRINT_WRITER.lock().write_fmt(format_args!($($arg)*)).unwrap();
  });
}

macro_rules! format {
  ($($arg:tt)*) => ({
    use core::fmt::Write;
    let mut output = collections::string::String::new();
    #[allow(unused_must_use)]
    output.write_fmt(format_args!($($arg)*));
    output
  });
}

macro_rules! debug {
  ($($arg:tt)*) => ({
    $crate::vga_buffer::debug();
  });
}