
macro_rules! println {
    ($fmt:expr) => (print!(concat!($fmt, "\n")));
    ($fmt:expr, $($arg:tt)*) => (print!(concat!($fmt, "\n"), $($arg)*));
}

macro_rules! print {
  ($($arg:tt)*) => ({
    use core::fmt::Write;
    $crate::buffers::PRINT_BUFFER.lock().write_fmt(format_args!($($arg)*)).unwrap();
  });
}

macro_rules! format {
  ($($arg:tt)*) => ({
    use core::fmt::Write;
    let mut output = collections::string::String::new();
    #[allow(unused_must_use)]
    let _ = output.write_fmt(format_args!($($arg)*));
    output
  });
}

macro_rules! debug {
  ($($arg:tt)*) => ({
    $crate::buffers::debug();
  });
}