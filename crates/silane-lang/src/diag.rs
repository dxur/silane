//! Source files, spans and rustc-style diagnostics.

use std::collections::HashSet;
use std::fmt::Write;
use std::path::{Path, PathBuf};

/// A byte range in one source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Span {
    pub file: u32,
    pub lo: u32,
    pub hi: u32,
}

impl Span {
    pub fn new(file: u32, lo: usize, hi: usize) -> Self {
        Span {
            file,
            lo: lo as u32,
            hi: hi as u32,
        }
    }

    /// Smallest span covering both `self` and `other` (which must be in the same file).
    pub fn to(self, other: Span) -> Span {
        Span {
            file: self.file,
            lo: self.lo.min(other.lo),
            hi: self.hi.max(other.hi),
        }
    }
}

pub struct SourceFile {
    pub path: PathBuf,
    pub src: String,
    line_starts: Vec<usize>,
}

impl SourceFile {
    fn new(path: PathBuf, src: String) -> Self {
        let line_starts = std::iter::once(0)
            .chain(src.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        SourceFile {
            path,
            src,
            line_starts,
        }
    }

    /// Zero-based line and column of a byte offset.
    pub fn line_col(&self, offset: usize) -> (usize, usize) {
        let line = match self.line_starts.binary_search(&offset) {
            Ok(l) => l,
            Err(l) => l - 1,
        };
        let col = self.src[self.line_starts[line]..offset].chars().count();
        (line, col)
    }

    fn line_text(&self, line: usize) -> &str {
        let start = self.line_starts[line];
        let end = self
            .line_starts
            .get(line + 1)
            .copied()
            .unwrap_or(self.src.len());
        self.src[start..end].trim_end_matches(['\n', '\r'])
    }
}

#[derive(Default)]
pub struct SourceMap {
    pub files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn add(&mut self, path: PathBuf, src: String) -> u32 {
        self.files.push(SourceFile::new(path, src));
        (self.files.len() - 1) as u32
    }

    pub fn find(&self, path: &Path) -> Option<u32> {
        self.files
            .iter()
            .position(|f| f.path == path)
            .map(|i| i as u32)
    }

    pub fn file(&self, id: u32) -> &SourceFile {
        &self.files[id as usize]
    }

    /// `path:line:col` of the start of a span (one-based line and column).
    pub fn location(&self, span: Span) -> String {
        let file = self.file(span.file);
        let (line, col) = file.line_col(span.lo as usize);
        format!("{}:{}:{}", file.path.display(), line + 1, col + 1)
    }

    pub fn snippet(&self, span: Span) -> &str {
        &self.file(span.file).src[span.lo as usize..span.hi as usize]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Level {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Diagnostic {
    pub level: Level,
    pub message: String,
    pub primary: Option<(Span, String)>,
    pub secondary: Vec<(Span, String)>,
    pub notes: Vec<String>,
}

impl Diagnostic {
    pub fn error(message: impl Into<String>) -> Self {
        Diagnostic {
            level: Level::Error,
            message: message.into(),
            primary: None,
            secondary: Vec::new(),
            notes: Vec::new(),
        }
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Diagnostic {
            level: Level::Warning,
            ..Diagnostic::error(message)
        }
    }

    pub fn at(mut self, span: Span, label: impl Into<String>) -> Self {
        self.primary = Some((span, label.into()));
        self
    }

    pub fn with(mut self, span: Span, label: impl Into<String>) -> Self {
        self.secondary.push((span, label.into()));
        self
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    pub fn render(&self, sources: &SourceMap) -> String {
        let mut out = String::new();
        let level = match self.level {
            Level::Error => "error",
            Level::Warning => "warning",
        };
        let _ = writeln!(out, "{level}: {}", self.message);

        let mut labels: Vec<(Span, &str, char)> = Vec::new();
        if let Some((span, label)) = &self.primary {
            labels.push((*span, label, '^'));
        }
        for (span, label) in &self.secondary {
            labels.push((*span, label, '-'));
        }

        let gutter = labels
            .iter()
            .map(|(span, _, _)| {
                let (line, _) = sources.file(span.file).line_col(span.lo as usize);
                (line + 1).to_string().len()
            })
            .max()
            .unwrap_or(1);
        let pad = " ".repeat(gutter);

        // Group labels by source line, keeping the primary label's line first.
        let mut groups: Vec<((u32, usize), Vec<(usize, usize, &str, char)>)> = Vec::new();
        for (span, label, marker) in &labels {
            let file = sources.file(span.file);
            let (line, col) = file.line_col(span.lo as usize);
            let text_len = file.line_text(line).chars().count();
            let (end_line, end_col) = file.line_col((span.hi as usize).max(span.lo as usize));
            let width = if end_line == line {
                end_col.saturating_sub(col).max(1)
            } else {
                text_len.saturating_sub(col).max(1)
            };
            let key = (span.file, line);
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some((_, items)) => items.push((col, width, label, *marker)),
                None => groups.push((key, vec![(col, width, label, *marker)])),
            }
        }
        let mut last_file = None;
        for (i, ((file_id, line), items)) in groups.iter().enumerate() {
            let file = sources.file(*file_id);
            if last_file != Some(*file_id) || i == 0 {
                let (col, ..) = items[0];
                let _ = writeln!(out, "{pad}--> {}:{}:{}", file.path.display(), line + 1, col + 1);
                last_file = Some(*file_id);
            }
            let _ = writeln!(out, "{pad} |");
            let _ = writeln!(out, "{:>gutter$} | {}", line + 1, file.line_text(*line));
            for (col, width, label, marker) in items {
                let marks = marker.to_string().repeat(*width);
                let line = format!("{pad} | {}{marks} {label}", " ".repeat(*col));
                let _ = writeln!(out, "{}", line.trim_end());
            }
        }
        for note in &self.notes {
            let _ = writeln!(out, "{pad} = note: {note}");
        }
        out
    }
}

/// An ordered, de-duplicated collection of diagnostics.
#[derive(Default)]
pub struct Diagnostics {
    list: Vec<Diagnostic>,
    seen: HashSet<(String, Option<Span>)>,
}

impl Diagnostics {
    pub fn push(&mut self, diag: Diagnostic) {
        let key = (diag.message.clone(), diag.primary.as_ref().map(|(s, _)| *s));
        if self.seen.insert(key) {
            self.list.push(diag);
        }
    }

    pub fn has_errors(&self) -> bool {
        self.list.iter().any(|d| d.level == Level::Error)
    }

    pub fn error_count(&self) -> usize {
        self.list.iter().filter(|d| d.level == Level::Error).count()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.list.iter()
    }

    pub fn render(&self, sources: &SourceMap) -> String {
        let mut out = String::new();
        for diag in &self.list {
            out.push_str(&diag.render(sources));
            out.push('\n');
        }
        out
    }
}
