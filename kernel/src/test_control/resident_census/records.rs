use core::fmt;

const MAX_ROWS: usize = 32;
const NAME_BYTES: usize = 32;

#[derive(Clone, Copy)]
pub(super) struct Pages {
    pub user: u32,
    pub stack: u32,
    pub tables: u32,
}

#[derive(Clone, Copy)]
struct Row {
    pid: u64,
    name: [u8; NAME_BYTES],
    name_len: usize,
    pages: Pages,
}

impl Row {
    const EMPTY: Self = Self {
        pid: 0,
        name: [0; NAME_BYTES],
        name_len: 0,
        pages: Pages { user: 0, stack: 0, tables: 0 },
    };
}

pub(super) struct Census {
    rows: [Row; MAX_ROWS],
    len: usize,
    truncated: bool,
    pub incomplete: bool,
}

impl Census {
    pub const fn new() -> Self {
        Self { rows: [Row::EMPTY; MAX_ROWS], len: 0, truncated: false, incomplete: false }
    }

    pub fn has_room(&mut self) -> bool {
        if self.len == self.rows.len() {
            self.truncated = true;
            return false;
        }
        true
    }

    pub fn push(&mut self, pid: u64, path: &str, pages: Pages) -> bool {
        if !self.has_room() {
            return false;
        }
        let name = path.rsplit('/').next().filter(|part| !part.is_empty()).unwrap_or("?");
        let row = &mut self.rows[self.len];
        row.pid = pid;
        row.pages = pages;
        row.name_len = name.len().min(NAME_BYTES);
        for (dst, byte) in row.name.iter_mut().zip(name.bytes()) {
            *dst = if byte.is_ascii_graphic() { byte } else { b'_' };
        }
        self.len += 1;
        true
    }

    pub fn report(&self, phase: &str, mut emit: impl FnMut(fmt::Arguments<'_>)) {
        emit(format_args!(
            "SUITE_MEMORY_PID gui-apps {phase} rows={} truncated={} incomplete={}",
            self.len, u8::from(self.truncated), u8::from(self.incomplete),
        ));
        for row in &self.rows[..self.len] {
            let name = core::str::from_utf8(&row.name[..row.name_len]).unwrap_or("?");
            emit(format_args!(
                "SUITE_MEMORY_PID gui-apps {phase} pid={} name={name} user_pages={} user_stacks={} page_tables={}",
                row.pid, row.pages.user, row.pages.stack, row.pages.tables,
            ));
        }
    }
}
