fn main() {
    let text = std::env::args().nth(1).expect("clipboard text argument");
    let window = breengel::Window::new_without_fonts(b"Clipboard helper", 32, 32)
        .expect("create clipboard owner window");
    libclipboard::set(window.id(), text.as_bytes()).expect("set clipboard");
}
