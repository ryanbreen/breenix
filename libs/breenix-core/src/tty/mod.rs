//! TTY primitives — termios settings and line discipline.

pub mod line_discipline;
pub mod termios;

pub use line_discipline::LineDiscipline;
pub use termios::Termios;
