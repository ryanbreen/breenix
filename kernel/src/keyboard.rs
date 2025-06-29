use spin::Mutex;
use core::sync::atomic::{AtomicUsize, Ordering};

mod scancodes;
mod modifiers;
mod event;

pub use event::KeyEvent;
use modifiers::Modifiers;
use scancodes::KEYS;

const QUEUE_SIZE: usize = 100;

static SCANCODE_QUEUE: Mutex<[u8; QUEUE_SIZE]> = Mutex::new([0; QUEUE_SIZE]);
static QUEUE_HEAD: AtomicUsize = AtomicUsize::new(0);
static QUEUE_TAIL: AtomicUsize = AtomicUsize::new(0);

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
    // Reset queue pointers
    QUEUE_HEAD.store(0, Ordering::Release);
    QUEUE_TAIL.store(0, Ordering::Release);
    
    // Reset keyboard state
    let mut state = KEYBOARD_STATE.lock();
    state.modifiers = Modifiers::new();
    state.e0_sequence = false;
}

/// Called by the keyboard interrupt handler
pub(crate) fn add_scancode(scancode: u8) {
    let mut queue = SCANCODE_QUEUE.lock();
    let head = QUEUE_HEAD.load(Ordering::Acquire);
    let tail = QUEUE_TAIL.load(Ordering::Acquire);
    
    let next_tail = (tail + 1) % QUEUE_SIZE;
    if next_tail != head {
        queue[tail] = scancode;
        QUEUE_TAIL.store(next_tail, Ordering::Release);
    } else {
        // Queue is full, drop the scancode
        log::warn!("Keyboard scancode queue full; dropping input");
    }
}

/// Read a raw scancode from the queue
pub fn read_scancode() -> Option<u8> {
    let queue = SCANCODE_QUEUE.lock();
    let head = QUEUE_HEAD.load(Ordering::Acquire);
    let tail = QUEUE_TAIL.load(Ordering::Acquire);
    
    if head != tail {
        let scancode = queue[head];
        QUEUE_HEAD.store((head + 1) % QUEUE_SIZE, Ordering::Release);
        Some(scancode)
    } else {
        None
    }
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

/// Read and process the next keyboard event
pub fn read_key() -> Option<KeyEvent> {
    while let Some(scancode) = read_scancode() {
        if let Some(event) = process_scancode(scancode) {
            return Some(event);
        }
    }
    None
}

/// Get current modifier state
pub fn get_modifiers() -> Modifiers {
    KEYBOARD_STATE.lock().modifiers
}