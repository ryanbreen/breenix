#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::io::drivers::display::text_buffer::print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{} - {}\n", $crate::io::timer::real_time(), format_args!($($arg)*)));
}

#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => ($crate::io::drivers::display::text_buffer::debug(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! debugln {
    () => ($crate::debug!("\n"));
    ($($arg:tt)*) => ($crate::debug!("{}\n", format_args!($($arg)*)));
}