/// Pending validation shares the most recently presented scene. Client requests
/// are IDs, so replacing a scene does not accumulate snapshots per client.
pub struct ValidationQueue<S> {
    scene: Option<S>,
    full_scene: bool,
    first_frames: Vec<FirstFrame>,
}

struct FirstFrame {
    id: u64,
    attempted: bool,
}

impl<S> ValidationQueue<S> {
    pub fn new() -> Self {
        Self { scene: None, full_scene: false, first_frames: Vec::new() }
    }

    pub fn presented(&mut self, scene: S) {
        self.scene = Some(scene);
        self.full_scene = true;
        for client in &mut self.first_frames {
            client.attempted = false;
        }
    }

    pub fn scene(&self) -> Option<&S> {
        self.scene.as_ref()
    }

    pub fn first_frame(&mut self, id: u64) {
        if !self.first_frames.iter().any(|client| client.id == id) {
            self.first_frames.push(FirstFrame { id, attempted: false });
        }
    }

    pub fn retain_clients(&mut self, mut live: impl FnMut(u64) -> bool) {
        self.first_frames.retain(|client| live(client.id));
    }

    pub fn complete(&mut self, id: u64) {
        self.first_frames.retain(|client| client.id != id);
    }

    pub fn invalidate_scene(&mut self) {
        self.scene = None;
        self.full_scene = false;
    }

    /// Leave work pending while either source is ready. The caller performs one
    /// comparison, then completes the clients that comparison certified.
    pub fn next_if_idle(
        &mut self,
        mut eligible: impl FnMut(u64) -> bool,
        idle: impl FnOnce() -> bool,
    ) -> Option<(&S, Vec<u64>)> {
        self.scene.as_ref()?;
        let clients: Vec<_> = self.first_frames.iter()
            .filter(|client| !client.attempted && eligible(client.id)).map(|client| client.id).collect();
        if (!self.full_scene && clients.is_empty()) || !idle() {
            return None;
        }
        self.full_scene = false;
        // A moving source is retried against a later presentation, rather than
        // spending its retry allowance on the same sampled scene.
        for client in &mut self.first_frames {
            if clients.contains(&client.id) {
                client.attempted = true;
            }
        }
        Some((self.scene()?, clients))
    }
}
