/// Prompt evidence must finish inside the desktop mode's 300-second window.
/// This is a verdict deadline, independent of client commits or readback count.
pub const PROMPT_TIMEOUT_MS: u64 = 60_000;

/// Pending validation shares the most recently presented scene. Client requests
/// are IDs, so replacing a scene does not accumulate snapshots per client.
pub struct ValidationQueue<S> {
    scene: Option<S>,
    first_frames: Vec<FirstFrame>,
    prompts: Vec<Prompt>,
}

pub enum PromptCheck {
    Text,
    Empty,
    Moved,
    Failed,
}

struct Prompt {
    id: u64,
    in_flight: Option<u64>,
    attempts: u32,
    last_commit: Option<u64>,
    deadline_ms: u64,
}

struct FirstFrame {
    id: u64,
    attempted: bool,
}

impl<S> ValidationQueue<S> {
    pub fn new() -> Self {
        Self { scene: None, first_frames: Vec::new(), prompts: Vec::new() }
    }

    /// Update the saved scene in place and return the working scene for reuse.
    pub fn presented(&mut self, scene: S) -> S where S: Clone {
        if let Some(saved) = &mut self.scene {
            saved.clone_from(&scene);
        } else {
            self.scene = Some(scene.clone());
        }
        for client in &mut self.first_frames {
            client.attempted = false;
        }
        scene
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
        self.prompts.retain(|client| live(client.id));
    }

    /// Request a separate prompt check, without replacing an existing request.
    pub fn request(&mut self, id: u64, now_ms: u64) {
        if !self.prompts.iter().any(|prompt| prompt.id == id) {
            self.prompts.push(Prompt { id, in_flight: None, attempts: 0, last_commit: None,
                deadline_ms: now_ms.saturating_add(PROMPT_TIMEOUT_MS) });
        }
    }

    pub fn prompt_pending(&self, id: u64) -> bool {
        self.prompts.iter().any(|prompt| prompt.id == id)
    }

    pub fn prompt_deadline_ms(&self) -> Option<u64> {
        self.prompts.iter().map(|prompt| prompt.deadline_ms).min()
    }

    /// Expiration needs neither a presented scene nor an idle readback slot.
    pub fn expire_prompts(&mut self, now_ms: u64) -> Vec<u64> {
        let expired = self.prompts.iter().filter(|prompt| now_ms >= prompt.deadline_ms)
            .map(|prompt| prompt.id).collect();
        self.prompts.retain(|prompt| now_ms < prompt.deadline_ms);
        expired
    }

    /// A reader attach or tree publication becomes visible through a new commit.
    /// Desktop repaints alone cannot change the client's captured prompt.
    pub fn prompt_ready(&self, id: u64, commit: Option<u64>) -> bool {
        self.scene.is_some() && commit.is_some() && self.prompts.iter().any(|prompt|
            prompt.id == id && prompt.in_flight.is_none() && prompt.last_commit != commit)
    }

    /// Reserve at most one readback of each sampled client commit.
    pub fn check_prompt(&mut self, id: u64, commit: u64) -> bool {
        if !self.prompt_ready(id, Some(commit)) { return false; }
        let Some(prompt) = self.prompts.iter_mut().find(|prompt| prompt.id == id) else { return false; };
        if prompt.in_flight.is_some() { return false; }
        prompt.in_flight = Some(commit);
        prompt.last_commit = Some(commit);
        true
    }

    /// Complete only a reserved check; return its commit on text or one failure at the limit.
    pub fn prompt_result(&mut self, id: u64, commit: u64, result: PromptCheck, limit: u32) -> Option<Result<u64, ()>> {
        let index = self.prompts.iter().position(|prompt| prompt.id == id && prompt.in_flight == Some(commit))?;
        let prompt = &mut self.prompts[index];
        prompt.in_flight = None;
        prompt.attempts += 1;
        let outcome = match result {
            PromptCheck::Text => Some(Ok(commit)),
            PromptCheck::Failed => Some(Err(())),
            PromptCheck::Empty | PromptCheck::Moved if prompt.attempts >= limit => Some(Err(())),
            PromptCheck::Empty | PromptCheck::Moved => None,
        };
        if outcome.is_some() { self.prompts.remove(index); }
        outcome
    }

    /// Reserve a prompt's sampled commit only when validation can run.
    pub fn next_prompt_if_idle(
        &mut self,
        mut eligible: impl FnMut(u64) -> Option<u64>,
        idle: impl FnOnce() -> bool,
    ) -> Option<(&S, u64, u64)> {
        self.scene.as_ref()?;
        let (id, commit) = self.prompts.iter().find_map(|prompt| {
            let commit = eligible(prompt.id)?;
            self.prompt_ready(prompt.id, Some(commit)).then_some((prompt.id, commit))
        })?;
        if !idle() || !self.check_prompt(id, commit) { return None; }
        Some((self.scene()?, id, commit))
    }

    pub fn pending(&self, id: u64) -> bool {
        self.scene.is_some() && (self.first_frames.iter().any(|c| c.id == id && !c.attempted)
            || self.prompts.iter().any(|prompt| prompt.id == id && prompt.last_commit.is_none()))
    }

    pub fn complete(&mut self, id: u64) {
        self.first_frames.retain(|client| client.id != id);
    }

    pub fn retry(&mut self, ids: &[u64]) {
        for client in &mut self.first_frames {
            if ids.contains(&client.id) { client.attempted = false; }
        }
    }

    pub fn invalidate_scene(&mut self) {
        self.scene = None;
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
        if clients.is_empty() || !idle() {
            return None;
        }
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
