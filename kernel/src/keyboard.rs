use spin::Mutex;
use futures_util::stream::StreamExt;

mod scancodes;
mod modifiers;
mod event;
mod stream;

pub use event::KeyEvent;
use modifiers::Modifiers;
use scancodes::KEYS;
pub use stream::ScancodeStream;

// Global keyboard state
static KEYBOARD_STATE: Mutex<KeyboardState> = Mutex::new(KeyboardState::new());

struct KeyboardState {
    modifiers: Modifiers,
    // Track if we're in the middle of an E0 extended sequence
    e0_sequence: bool,
}

impl KeyboardState {
    const fn new() -> Self {
        Self {
            modifiers: Modifiers::new(),
            e0_sequence: false,
        }
    }
}

pub fn init() {
    // Reset keyboard state
    let mut state = KEYBOARD_STATE.lock();
    state.modifiers = Modifiers::new();
    state.e0_sequence = false;
}

/// Called by the keyboard interrupt handler
pub(crate) fn add_scancode(scancode: u8) {
    stream::add_scancode(scancode);
}

/// Process a scancode and return a keyboard event if applicable
pub fn process_scancode(scancode: u8) -> Option<KeyEvent> {
    let mut state = KEYBOARD_STATE.lock();
    
    // Handle E0 extended sequences
    if scancode == 0xE0 {
        state.e0_sequence = true;
        return None;
    }
    
    // Handle extended scancodes (we'll add more later)
    if state.e0_sequence {
        state.e0_sequence = false;
        // For now, ignore extended scancodes
        return None;
    }
    
    // Update modifiers - returns true if this was a modifier
    if state.modifiers.update(scancode) {
        // Log modifier changes for debugging
        log::debug!("Modifier state: shift={}, ctrl={}, alt={}, caps_lock={}", 
            state.modifiers.shift(),
            state.modifiers.ctrl(),
            state.modifiers.alt(),
            state.modifiers.caps_lock
        );
        return None;
    }
    
    // Ignore key releases (high bit set)
    if scancode > 127 {
        return None;
    }
    
    // Look up the key
    if let Some(key) = KEYS[scancode as usize] {
        let character = state.modifiers.apply_to(key);
        let event = KeyEvent::new(scancode, Some(character), &state.modifiers);
        return Some(event);
    }
    
    // Unknown key - create event without character
    Some(KeyEvent::new(scancode, None, &state.modifiers))
}

/// Async keyboard task that processes scancodes and displays typed characters
/// 
/// Special key combinations:
/// - Ctrl+C: Interrupt signal
/// - Ctrl+D: End of input
/// - Ctrl+S: Suspend output
/// - Ctrl+T: Time debug information
/// - Ctrl+M: Memory debug information
pub async fn keyboard_task() {
    log::info!("Keyboard ready! Type to see characters (Ctrl+C/D/S/T/M for special actions)");
    
    let mut scancodes = ScancodeStream::new();
    
    while let Some(scancode) = scancodes.next().await {
        if let Some(event) = process_scancode(scancode) {
            if let Some(character) = event.character {
                // Handle special key combinations
                if event.is_ctrl_c() {
                    log::info!("Ctrl+C pressed - interrupt signal");
                } else if event.is_ctrl_d() {
                    log::info!("Ctrl+D pressed - end of input");
                } else if event.is_ctrl_s() {
                    log::info!("Ctrl+S pressed - suspend output");
                } else if event.is_ctrl_t() {
                    log::info!("Ctrl+T pressed - showing time debug info");
                    crate::time::debug_time_info();
                } else if event.is_ctrl_m() {
                    log::info!("Ctrl+M pressed - showing memory debug info");
                    crate::memory::debug_memory_info();
                } else {
                    // Display the typed character
                    log::info!("Typed: '{}' (scancode: 0x{:02X})", character, scancode);
                }
            }
        }
    }
}

