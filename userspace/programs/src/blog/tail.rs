//! Pure state for following a line-oriented file.

pub const MAX_FILE_SIZE: usize = 256 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Read,
    Reload,
    Reopen,
}

pub struct Tail {
    inode: Option<u64>,
    offset: usize,
    content: Vec<u8>,
    lines: Vec<(usize, usize)>,
    first_number: usize,
}

impl Tail {
    pub fn new() -> Self {
        Self { inode: None, offset: 0, content: Vec::new(), lines: Vec::new(), first_number: 1 }
    }

    pub fn observe(&mut self, inode: u64, size: usize) -> Action {
        let action = if self.inode.is_some_and(|old| old != inode) {
            Action::Reopen
        } else if size < self.offset {
            Action::Reload
        } else {
            Action::Read
        };
        if action != Action::Read {
            self.offset = 0;
            self.content.clear();
            self.lines.clear();
            self.first_number = 1;
        }
        self.inode = Some(inode);
        action
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.offset += bytes.len();
        self.content.extend_from_slice(bytes);
        self.lines.clear();
        let mut start = 0;
        for (i, &byte) in self.content.iter().enumerate() {
            if byte == b'\n' {
                self.lines.push((start, i));
                start = i + 1;
            }
        }
        let mut dropped = 0;
        let mut end = 0;
        while self.content.len() - end > MAX_FILE_SIZE && dropped < self.lines.len() {
            end = self.lines[dropped].1 + 1;
            dropped += 1;
        }
        if dropped > 0 {
            self.content.drain(..end);
            self.lines.drain(..dropped);
            for (start, finish) in &mut self.lines {
                *start -= end;
                *finish -= end;
            }
            self.first_number += dropped;
        }
    }

    pub fn line_count(&self) -> usize { self.lines.len() }
    pub fn line(&self, index: usize) -> &[u8] {
        self.lines.get(index).map_or(b"", |&(start, end)| &self.content[start..end])
    }
    pub fn line_number(&self, index: usize) -> usize { self.first_number + index }
    pub fn offset(&self) -> usize { self.offset }
}
