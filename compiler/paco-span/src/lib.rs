//! Source identity and byte-span utilities shared by compiler crates.

use std::fmt;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FileId(u32);

impl FileId {
    pub const ROOT: Self = Self(0);

    pub fn new(index: u32) -> Self {
        Self(index)
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Span {
    file_id: FileId,
    start: usize,
    end: usize,
}

impl Span {
    pub fn new(file_id: FileId, start: usize, end: usize) -> Self {
        Self {
            file_id,
            start,
            end,
        }
    }

    pub fn new_root(start: usize, end: usize) -> Self {
        Self::new(FileId::ROOT, start, end)
    }

    pub fn file_id(self) -> FileId {
        self.file_id
    }

    pub fn start(self) -> usize {
        self.start
    }

    pub fn end(self) -> usize {
        self.end
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LineColumn {
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Location {
    pub file_id: FileId,
    pub file_name: String,
    pub start: LineColumn,
    pub end: LineColumn,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceMapError {
    UnknownFile(FileId),
    InvalidSpan {
        start: usize,
        end: usize,
        len: usize,
    },
}

impl fmt::Display for SourceMapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownFile(file_id) => write!(f, "unknown file id {}", file_id.index()),
            Self::InvalidSpan { start, end, len } => {
                write!(f, "invalid span {start}..{end} for source length {len}")
            }
        }
    }
}

impl std::error::Error for SourceMapError {}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SourceFile {
    name: String,
    text: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_file(&mut self, name: impl Into<String>, text: impl Into<String>) -> FileId {
        let id = FileId::new(self.files.len() as u32);
        self.files.push(SourceFile {
            name: name.into(),
            text: text.into(),
        });
        id
    }

    pub fn source(&self, file_id: FileId) -> Option<&str> {
        self.files
            .get(file_id.index())
            .map(|file| file.text.as_str())
    }

    pub fn file_name(&self, file_id: FileId) -> Option<&str> {
        self.files
            .get(file_id.index())
            .map(|file| file.name.as_str())
    }

    pub fn location(&self, span: Span) -> Result<Location, SourceMapError> {
        let file = self
            .files
            .get(span.file_id.index())
            .ok_or(SourceMapError::UnknownFile(span.file_id))?;
        let len = file.text.len();
        if span.start > span.end || span.end > len {
            return Err(SourceMapError::InvalidSpan {
                start: span.start,
                end: span.end,
                len,
            });
        }

        Ok(Location {
            file_id: span.file_id,
            file_name: file.name.clone(),
            start: line_column(&file.text, span.start),
            end: line_column(&file.text, span.end),
        })
    }
}

fn line_column(source: &str, offset: usize) -> LineColumn {
    let mut line = 1;
    let mut column = 1;

    for (index, ch) in source.char_indices() {
        if index >= offset {
            break;
        }

        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }

    LineColumn { line, column }
}
