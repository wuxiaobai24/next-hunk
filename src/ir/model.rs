/// Where a file entry came from in a local working-set review.
///
/// Only set for git worktree/index sources (`diff` / `serve` / `inspect` without
/// a patch path). Commit ranges, patch files, and pager input leave this
/// `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOrigin {
    /// HEAD tree vs index (`git diff --cached`).
    Staged,
    /// Index vs worktree tracked change (`git diff`).
    Modified,
    /// Untracked worktree file (`--include-untracked`).
    Untracked,
}

impl FileOrigin {
    /// Single-character mark for the file rail: `S` / `M` / `?`.
    pub fn mark(self) -> char {
        match self {
            FileOrigin::Staged => 'S',
            FileOrigin::Modified => 'M',
            FileOrigin::Untracked => '?',
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            FileOrigin::Staged => "staged",
            FileOrigin::Modified => "modified",
            FileOrigin::Untracked => "untracked",
        }
    }
}

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
    /// Inserted lines in this file (computed once at parse time).
    pub inserts: u64,
    /// Deleted lines in this file (computed once at parse time).
    pub deletes: u64,
    /// Optional bucket origin for working-set reviews (rail `S`/`M`/`?`).
    pub origin: Option<FileOrigin>,
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
    /// Total inserted lines across all files (computed once at parse time).
    pub inserts: u64,
    /// Total deleted lines across all files (computed once at parse time).
    pub deletes: u64,
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

    /// Attach per-file origins in parse order (best-effort zip).
    ///
    /// Length mismatch is tolerated: extra origins are ignored; missing ones
    /// leave `origin` as `None`. Used after producing a working-set diff so the
    /// file rail can show `S`/`M`/`?` without encoding tags into the patch text.
    pub fn apply_file_origins(&mut self, origins: &[FileOrigin]) {
        for (file, origin) in self.files.iter_mut().zip(origins.iter()) {
            file.origin = Some(*origin);
        }
    }
}
