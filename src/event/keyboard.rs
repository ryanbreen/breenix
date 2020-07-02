//use debug;

use crate::constants;
use crate::event::EventType;

use crate::io::drivers::display::text_buffer;

use crate::state;

#[derive(Clone, Copy)]
pub struct ControlKeyState {
    pub ctrl: bool,
    pub cmd: bool,
    pub alt: bool,
    pub shift: bool,
    pub caps_lock: bool,
    pub scroll_lock: bool,
    pub num_lock: bool,
}

#[derive(Clone, Copy)]
pub struct KeyEvent {
    pub event_type: EventType,
    pub scancode: u8,
    pub character: char,
    pub controls: ControlKeyState,
}

pub struct KeyEventHandler {
    pub handles_event: &'static (dyn Fn(&KeyEvent) -> bool + Sync),
    pub notify: &'static (dyn Fn(&KeyEvent) + Sync),
}

const KEY_EVENT_SCREEN_WRITER:KeyEventHandler = KeyEventHandler {
    handles_event: &|ev:&KeyEvent| -> bool {
        !ev.controls.ctrl && !ev.controls.alt
    },
    
    notify: &|ev:&KeyEvent| {
        if ev.scancode == constants::keyboard::ENTER_KEY.scancode {
            text_buffer::KEYBOARD_BUFFER.lock().new_line();
            return;
        }

        if ev.scancode == constants::keyboard::DELETE_KEY.scancode {
            text_buffer::KEYBOARD_BUFFER.lock().delete_byte();
            return;
        }

        if ev.character as u8 != 0 {
            text_buffer::KEYBOARD_BUFFER.lock().write_byte(ev.character as u8);
        }
    },
};

const KEY_EVENT_TOGGLE_WATCHER:KeyEventHandler = KeyEventHandler {
    handles_event: &|ev:&KeyEvent| -> bool {
        ev.scancode == constants::keyboard::S_KEY.scancode && (ev.controls.ctrl || ev.controls.cmd)
    },

    notify: &|ev:&KeyEvent| {
        // Switch buffers
        text_buffer::toggle();
    }
};

const DEBUG_WATCHER:KeyEventHandler = KeyEventHandler {
    handles_event: &|ev:&KeyEvent| -> bool {
        ev.scancode == constants::keyboard::D_KEY.scancode && (ev.controls.ctrl || ev.controls.cmd)
    },

    notify: &|ev:&KeyEvent| {
        // FIXME: this deadlocks
        //state::debug();
    }
};

pub fn initialize() {    
    state::register_key_event_listener(KEY_EVENT_SCREEN_WRITER);
    state::register_key_event_listener(KEY_EVENT_TOGGLE_WATCHER);
    state::register_key_event_listener(DEBUG_WATCHER);
}
