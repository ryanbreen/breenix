//! Peer information and management.

/// Information about a connected peer.
#[derive(Clone)]
pub struct PeerInfo {
    /// Unique peer ID (0 = host, 1-15 = clients)
    pub peer_id: u8,
    /// Display name (up to 32 bytes)
    pub name: [u8; 32],
    /// Actual length of the name
    pub name_len: u8,
    /// Last known cursor position
    pub cursor_x: i16,
    pub cursor_y: i16,
    /// Whether the cursor is visible
    pub cursor_visible: bool,
    /// Current tool
    pub tool: u8,
    /// Current brush size
    pub brush_size: u8,
    /// Current color
    pub color_r: u8,
    pub color_g: u8,
    pub color_b: u8,
}

impl PeerInfo {
    pub fn new(peer_id: u8, name: &[u8]) -> Self {
        let mut n = [0u8; 32];
        let len = name.len().min(32);
        n[..len].copy_from_slice(&name[..len]);
        Self {
            peer_id,
            name: n,
            name_len: len as u8,
            cursor_x: 0,
            cursor_y: 0,
            cursor_visible: false,
            tool: 0,
            brush_size: 5,
            color_r: 0,
            color_g: 0,
            color_b: 0,
        }
    }

    /// Get the display name as a byte slice.
    pub fn name_bytes(&self) -> &[u8] {
        &self.name[..self.name_len as usize]
    }
}
