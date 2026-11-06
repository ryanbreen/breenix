extern crate self as libbreenix;
pub mod window {
    pub mod abi {
        pub mod win_flags {
            pub const REGISTERED: u32 = 1;
            pub const VISIBLE: u32 = 2;
            pub const MINIMIZED: u32 = 4;
            pub const FOCUSED: u32 = 8;
        }
    }
}
mod session {
    pub struct WindowInfo { pub x: i32, pub y: i32, pub flags: u32 }
}
mod config {
    pub const BORDER_WIDTH: usize = 2;
    pub const TITLE_BAR_HEIGHT: usize = 32;
}
#[path = "../userspace/programs/src/bwm/adopt.rs"]
mod adopt;

#[test]
fn published_content_round_trips_to_frame_and_back() {
    use window::abi::win_flags::*;
    for scale in [1, 2] {
        for chromeless in [false, true] {
            for flags in [VISIBLE, MINIMIZED, VISIBLE | FOCUSED] {
                let info = session::WindowInfo { x: 123, y: 87, flags };
                let (x, y) = adopt::frame_origin(&info, chromeless, scale).unwrap();
                let border = if chromeless { 0 } else { config::BORDER_WIDTH * scale } as i32;
                let title = if chromeless { 0 } else { config::TITLE_BAR_HEIGHT * scale } as i32;
                assert_eq!((x + border, y + title + border), (info.x, info.y));
            }
        }
    }
}

#[test]
fn unpublished_geometry_is_not_adopted() {
    use window::abi::win_flags::*;
    for flags in [0, REGISTERED, REGISTERED | FOCUSED] {
        let info = session::WindowInfo { x: 123, y: 87, flags };
        assert_eq!(adopt::frame_origin(&info, false, 1), None);
    }
}
