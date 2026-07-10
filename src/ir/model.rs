/// One side of a changed line, or context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Add,
    Delete,
    /// `\ No newline at end of file` and similar meta lines.
    Meta,
}

/// A single line in a hunk. Text is interned in the parent [`Review`] arena.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    /// Byte range into `Review::text_arena`.
    pub text: std::ops::Range<usize>,
}

/// One unified-diff hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// Raw `@@ ... @@` header text range into the arena.
    pub header: std::ops::Range<usize>,
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    pub lines: Vec<DiffLine>,
}

/// One file in the review stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    /// Preferred display path (new path, else old).
    pub display_path: String,
    pub hunks: Vec<Hunk>,
    /// Inclusive stream-row range occupied by this file (header + body).
    pub stream_start: usize,
    pub stream_len: usize,
}

/// Full review: shared text arena + files.
#[derive(Debug, Clone, Default)]
pub struct Review {
    /// All line/header text concatenated (or retained slices from original input).
    pub text_arena: String,
    pub files: Vec<FileDiff>,
    /// Total virtual stream rows (file headers + hunk headers + lines).
    pub stream_len: usize,
    /// Absolute stream rows of every hunk header across all files, ascending.
    ///
    /// Built at parse time so hunk-to-hunk jumps are a cheap binary search
    /// instead of re-scanning the file tree (architecture §2.3: index ≠ content).
    /// Does **not** include standalone binary-meta body rows (files with no
    /// hunks have nothing to jump to).
    pub hunk_starts: Vec<usize>,
}

impl Review {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn text(&self, range: std::ops::Range<usize>) -> &str {
        self.text_arena.get(range).unwrap_or("")
    }

    pub fn display_path(&self, file_idx: usize) -> &str {
        self.files
            .get(file_idx)
            .map(|f| f.display_path.as_str())
            .unwrap_or("<unknown>")
    }
}
