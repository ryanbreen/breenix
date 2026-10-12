//! Checkpoint-based test infrastructure for signal-driven testing
//!
//! Replaces time-based polling with checkpoint detection for fast, reliable tests.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::time::{Duration, Instant};

/// Tracks kernel checkpoints in serial output with O(1) reading via file offset
pub struct CheckpointTracker {
    file_path: String,
    file_offset: u64,                  // Track position for O(1) reading
    pending_line: String,             // A serial write may end partway through a line
    current_index: usize,              // Current checkpoint in sequence
    checkpoint_time: Instant,          // When last checkpoint reached
    sequence: Vec<(String, Duration)>, // Expected checkpoints with timeouts
}

impl CheckpointTracker {
    /// Create a new checkpoint tracker with a sequence of expected checkpoints
    ///
    /// Each checkpoint has a name and a timeout duration.
    /// The tracker will fail if a checkpoint isn't reached within its timeout.
    pub fn new(file_path: &str, sequence: Vec<(String, Duration)>) -> Self {
        Self {
            file_path: file_path.to_string(),
            file_offset: 0,
            pending_line: String::new(),
            current_index: 0,
            checkpoint_time: Instant::now(),
            sequence,
        }
    }

    /// Check for new checkpoints by reading only new file content
    ///
    /// Returns:
    /// - Ok(true) if the next expected checkpoint was found
    /// - Ok(false) if no new checkpoint found (keep waiting)
    /// - Err(e) if file read failed
    pub fn check_for_new_checkpoints(&mut self) -> Result<bool, std::io::Error> {
        // If we've completed all checkpoints, return true
        if self.current_index >= self.sequence.len() {
            return Ok(true);
        }

        let mut file = File::open(&self.file_path)?;

        // Seek to our last known position (O(1) operation)
        file.seek(SeekFrom::Start(self.file_offset))?;

        // Read only new content
        let mut new_content = String::new();
        let bytes_read = file.read_to_string(&mut new_content)?;

        if bytes_read == 0 {
            // No new content
            return Ok(false);
        }

        // Update our offset for next read
        self.file_offset += bytes_read as u64;

        self.pending_line.push_str(&new_content);
        while let Some(end) = self.pending_line.find('\n') {
            let line = self.pending_line[..end].trim_end_matches('\r');
            let (expected, _) = &self.sequence[self.current_index];
            // The kernel logger prints this line. An announcement containing the
            // marker, or a partial serial write, is not a reached checkpoint.
            if line.ends_with(&format!("kernel: [CHECKPOINT:{expected}]")) {
                self.current_index += 1;
                self.checkpoint_time = Instant::now();
                if self.current_index >= self.sequence.len() {
                    return Ok(true);
                }
            }
            self.pending_line.drain(..=end);
        }

        // Checkpoint not found yet
        Ok(false)
    }

    /// Check if the current checkpoint has timed out
    pub fn is_timed_out(&self) -> bool {
        if self.current_index >= self.sequence.len() {
            return false; // All checkpoints complete
        }

        let (_name, timeout) = &self.sequence[self.current_index];
        self.checkpoint_time.elapsed() > *timeout
    }

    /// Get timeout information for error messages
    pub fn timeout_info(&self) -> String {
        if self.current_index >= self.sequence.len() {
            return "All checkpoints complete".to_string();
        }

        let (name, timeout) = &self.sequence[self.current_index];
        format!(
            "Waiting for checkpoint '{}' (timeout: {:?}, elapsed: {:?})",
            name,
            timeout,
            self.checkpoint_time.elapsed()
        )
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    #[test]
    fn test_checkpoint_detection() {
        let test_file = "target/test_checkpoint.txt";
        let _ = fs::remove_file(test_file);

        // Create a checkpoint sequence
        let checkpoints = vec![
            ("INIT".to_string(), Duration::from_secs(5)),
            ("POST_COMPLETE".to_string(), Duration::from_secs(5)),
        ];

        let mut tracker = CheckpointTracker::new(test_file, checkpoints);

        // Write first checkpoint
        let mut file = fs::File::create(test_file).unwrap();
        writeln!(file, "[ INFO] kernel: [CHECKPOINT:INIT]").unwrap();
        file.flush().unwrap();

        // Check for first checkpoint
        assert_eq!(tracker.check_for_new_checkpoints().unwrap(), false);
        assert_eq!(tracker.current_index, 1); // Moved to next checkpoint

        // Write second checkpoint
        writeln!(file, "[ INFO] kernel: [CHECKPOINT:POST_COMPLETE]").unwrap();
        file.flush().unwrap();

        // Check for second checkpoint
        assert_eq!(tracker.check_for_new_checkpoints().unwrap(), true);
        assert_eq!(tracker.current_index, 2); // All checkpoints complete

        // Cleanup
        let _ = fs::remove_file(test_file);
    }

    #[test]
    fn test_checkpoint_timeout() {
        let test_file = "target/test_timeout.txt";
        let _ = fs::remove_file(test_file);
        fs::File::create(test_file).unwrap();

        // Create a checkpoint with 100ms timeout
        let checkpoints = vec![("FAST".to_string(), Duration::from_millis(100))];

        let tracker = CheckpointTracker::new(test_file, checkpoints);

        // Wait for timeout
        std::thread::sleep(Duration::from_millis(150));

        // Should be timed out
        assert!(tracker.is_timed_out());

        // Cleanup
        let _ = fs::remove_file(test_file);
    }

    #[test]
    fn announcement_and_partial_line_do_not_complete_checkpoint() {
        let test_file = "target/test_checkpoint_origin.txt";
        let _ = fs::remove_file(test_file);
        let mut tracker = CheckpointTracker::new(
            test_file,
            vec![("POST_COMPLETE".to_string(), Duration::from_secs(5))],
        );
        let mut file = fs::File::create(test_file).unwrap();
        writeln!(file, "kernel::test_exec: will emit [CHECKPOINT:POST_COMPLETE]").unwrap();
        write!(file, "[ INFO] kernel: [CHECKPOINT:POST_").unwrap();
        file.flush().unwrap();
        assert!(!tracker.check_for_new_checkpoints().unwrap());
        writeln!(file, "COMPLETE]").unwrap();
        file.flush().unwrap();
        assert!(tracker.check_for_new_checkpoints().unwrap());
        fs::remove_file(test_file).unwrap();
    }
}
