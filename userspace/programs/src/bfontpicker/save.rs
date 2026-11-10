use super::conf;

pub struct SaveState {
    pub path: std::path::PathBuf,
    pub save: Option<&'static str>,
    pub error: Option<String>,
}

impl SaveState {
    pub fn update(&mut self, key: &str, value: &str) -> bool {
        let contents = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) => { self.save = Some("failed"); self.error = Some(e.to_string()); return false; }
        };
        // This editor owns one setting; preserve fields the font reader does
        // not understand. Font selection and size controls validate the value.
        let updated = conf::set(&contents, key, value);
        match libbreenix::atomic_file::save(&self.path, updated.as_bytes()) {
            libbreenix::atomic_file::SaveResult::Saved => { self.save = Some("saved"); self.error = None; true }
            libbreenix::atomic_file::SaveResult::Unconfirmed(e) => { self.save = Some("unconfirmed"); self.error = Some(e.to_string()); true }
            libbreenix::atomic_file::SaveResult::Failed(e) => { self.save = Some("failed"); self.error = Some(e.to_string()); false }
        }
    }
    pub fn message(&self) -> String {
        match (&self.save, &self.error) {
            (Some("unconfirmed"), Some(e)) => format!("Applied, unconfirmed: {}", e),
            (_, Some(e)) => e.clone(),
            _ => "Saved".into(),
        }
    }
}

