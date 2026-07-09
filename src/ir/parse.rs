use super::model::{DiffLine, DiffLineKind, FileDiff, Hunk, Review};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("empty diff input")]
    Empty,
}

/// Parse a unified diff into a compact [`Review`].
///
/// This is intentionally streaming-friendly: one pass, no regex engine, no
/// per-character allocations beyond the shared text arena.
pub fn parse_unified_diff(input: &str) -> Result<Review, ParseError> {
    if input.is_empty() {
        return Err(ParseError::Empty);
    }

    let mut review = Review {
        text_arena: String::with_capacity(input.len()),
        files: Vec::new(),
        stream_len: 0,
    };

    let mut current: Option<FileBuilder> = None;
    let mut stream_row = 0usize;

    for raw_line in input.split_inclusive('\n') {
        let line = raw_line.strip_suffix('\n').unwrap_or(raw_line);
        let line = line.strip_suffix('\r').unwrap_or(line);

        if let Some(path) = strip_prefix_path(line, "diff --git ") {
            flush_file(&mut review, &mut current, &mut stream_row);
            let (old, new) = parse_git_paths(path);
            current = Some(FileBuilder::new(old, new));
            continue;
        }

        if line.starts_with("--- ") {
            let path = parse_a_b_path(&line[4..]);
            if current.is_none() {
                current = Some(FileBuilder::new(path.clone(), None));
            }
            if let Some(file) = current.as_mut() {
                file.old_path = path;
                file.touch_display();
            }
            continue;
        }

        if line.starts_with("+++ ") {
            let path = parse_a_b_path(&line[4..]);
            if current.is_none() {
                current = Some(FileBuilder::new(None, path.clone()));
            }
            if let Some(file) = current.as_mut() {
                file.new_path = path;
                file.touch_display();
            }
            continue;
        }

        // Skip common git headers that are not stream content.
        if line.starts_with("index ")
            || line.starts_with("new file mode ")
            || line.starts_with("deleted file mode ")
            || line.starts_with("old mode ")
            || line.starts_with("new mode ")
            || line.starts_with("similarity index ")
            || line.starts_with("rename from ")
            || line.starts_with("rename to ")
            || line.starts_with("copy from ")
            || line.starts_with("copy to ")
            || line.starts_with("Binary files ")
            || line.starts_with("GIT binary patch")
        {
            continue;
        }

        if line.starts_with("@@ ") {
            if current.is_none() {
                current = Some(FileBuilder::new(None, Some("unknown".into())));
            }
            let file = current.as_mut().expect("just inserted");
            // File header row once per file, before first hunk.
            if file.hunks.is_empty() && !file.emitted_header {
                file.stream_start = stream_row;
                stream_row += 1; // file header row
                file.emitted_header = true;
            }
            let header_range = push_text(&mut review.text_arena, line);
            let (old_start, old_count, new_start, new_count) = parse_hunk_header(line);
            stream_row += 1; // hunk header row
            file.hunks.push(HunkBuilder {
                header: header_range,
                old_start,
                old_count,
                new_start,
                new_count,
                lines: Vec::new(),
            });
            continue;
        }

        if let Some(file) = current.as_mut() {
            if file.hunks.is_empty() {
                continue;
            }
            let kind = match line.chars().next() {
                Some('+') => DiffLineKind::Add,
                Some('-') => DiffLineKind::Delete,
                Some(' ') | None => DiffLineKind::Context,
                Some('\\') => DiffLineKind::Meta,
                _ => DiffLineKind::Context,
            };
            let body = if matches!(kind, DiffLineKind::Meta) {
                line
            } else if line.is_empty() {
                ""
            } else {
                &line[1..]
            };
            let text = push_text(&mut review.text_arena, body);
            file.hunks
                .last_mut()
                .expect("hunk exists")
                .lines
                .push(DiffLine { kind, text });
            stream_row += 1;
        }
    }

    flush_file(&mut review, &mut current, &mut stream_row);
    review.stream_len = stream_row;

    if review.files.is_empty() {
        return Err(ParseError::Empty);
    }

    Ok(review)
}

struct HunkBuilder {
    header: std::ops::Range<usize>,
    old_start: u32,
    old_count: u32,
    new_start: u32,
    new_count: u32,
    lines: Vec<DiffLine>,
}

struct FileBuilder {
    old_path: Option<String>,
    new_path: Option<String>,
    display_path: String,
    hunks: Vec<HunkBuilder>,
    stream_start: usize,
    emitted_header: bool,
}

impl FileBuilder {
    fn new(old: Option<String>, new: Option<String>) -> Self {
        let mut s = Self {
            old_path: old,
            new_path: new,
            display_path: String::new(),
            hunks: Vec::new(),
            stream_start: 0,
            emitted_header: false,
        };
        s.touch_display();
        s
    }

    fn touch_display(&mut self) {
        self.display_path = self
            .new_path
            .clone()
            .filter(|p| p != "/dev/null")
            .or_else(|| self.old_path.clone().filter(|p| p != "/dev/null"))
            .unwrap_or_else(|| "unknown".into());
    }
}

fn flush_file(review: &mut Review, current: &mut Option<FileBuilder>, stream_row: &mut usize) {
    let Some(file) = current.take() else {
        return;
    };
    if file.hunks.is_empty() {
        return;
    }

    let stream_start = if file.emitted_header {
        file.stream_start
    } else {
        let start = *stream_row;
        *stream_row += 1;
        start
    };

    let hunks: Vec<Hunk> = file
        .hunks
        .into_iter()
        .map(|h| Hunk {
            header: h.header,
            old_start: h.old_start,
            old_count: h.old_count,
            new_start: h.new_start,
            new_count: h.new_count,
            lines: h.lines,
        })
        .collect();

    let body_rows: usize = hunks
        .iter()
        .map(|h| 1 + h.lines.len()) // hunk header + lines
        .sum();
    let stream_len = 1 + body_rows; // file header + body

    review.files.push(FileDiff {
        old_path: file.old_path,
        new_path: file.new_path,
        display_path: file.display_path,
        hunks,
        stream_start,
        stream_len,
    });
}

fn push_text(arena: &mut String, s: &str) -> std::ops::Range<usize> {
    let start = arena.len();
    arena.push_str(s);
    start..arena.len()
}

fn strip_prefix_path<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    line.strip_prefix(prefix)
}

fn parse_git_paths(rest: &str) -> (Option<String>, Option<String>) {
    // `a/foo b/foo` (paths may contain spaces rarely; keep simple split)
    let parts: Vec<&str> = rest.split_whitespace().collect();
    if parts.len() >= 2 {
        (
            Some(strip_ab_prefix(parts[0])),
            Some(strip_ab_prefix(parts[1])),
        )
    } else if parts.len() == 1 {
        (Some(strip_ab_prefix(parts[0])), None)
    } else {
        (None, None)
    }
}

fn strip_ab_prefix(path: &str) -> String {
    path.strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .unwrap_or(path)
        .to_string()
}

fn parse_a_b_path(rest: &str) -> Option<String> {
    let path = rest.split('\t').next().unwrap_or(rest).trim();
    if path == "/dev/null" {
        return Some("/dev/null".into());
    }
    Some(strip_ab_prefix(path))
}

fn parse_hunk_header(line: &str) -> (u32, u32, u32, u32) {
    // @@ -l,s +l,s @@ optional
    let mut old_start = 0u32;
    let mut old_count = 1u32;
    let mut new_start = 0u32;
    let mut new_count = 1u32;

    let Some(body) = line.strip_prefix("@@ ") else {
        return (old_start, old_count, new_start, new_count);
    };
    let end = body.find(" @@").unwrap_or(body.len());
    let specs = &body[..end];
    for part in specs.split_whitespace() {
        if let Some(spec) = part.strip_prefix('-') {
            let (s, c) = parse_range(spec);
            old_start = s;
            old_count = c;
        } else if let Some(spec) = part.strip_prefix('+') {
            let (s, c) = parse_range(spec);
            new_start = s;
            new_count = c;
        }
    }
    (old_start, old_count, new_start, new_count)
}

fn parse_range(spec: &str) -> (u32, u32) {
    if let Some((a, b)) = spec.split_once(',') {
        (
            a.parse().unwrap_or(0),
            b.parse().unwrap_or(1).max(0),
        )
    } else {
        (spec.parse().unwrap_or(0), 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
diff --git a/src/a.rs b/src/a.rs
index 111..222 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,3 +1,4 @@
 fn main() {
-    println!(\"hi\");
+    println!(\"hello\");
+    println!(\"world\");
 }
diff --git a/src/b.rs b/src/b.rs
--- a/src/b.rs
+++ b/src/b.rs
@@ -1 +1 @@
-old
+new
";

    #[test]
    fn parses_two_files() {
        let review = parse_unified_diff(SAMPLE).unwrap();
        assert_eq!(review.file_count(), 2);
        assert_eq!(review.files[0].display_path, "src/a.rs");
        assert_eq!(review.files[1].display_path, "src/b.rs");
        assert!(review.stream_len > 0);
        // first file: header + hunk header + 5 lines = 7, etc.
        assert!(review.files[0].hunks[0].lines.len() >= 4);
    }

    #[test]
    fn empty_errors() {
        assert!(parse_unified_diff("").is_err());
    }
}
