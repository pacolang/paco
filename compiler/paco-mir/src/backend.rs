//! The interface both codegen backends implement.

use crate::{Body, Profile};

/// What to generate code for: a target triple (`None` for the host) and the
/// build profile, which picks the optimization level.
#[derive(Clone, Debug)]
pub struct Target {
    pub triple: Option<String>,
    pub profile: Profile,
}

/// The bytes of a relocatable object file for [`Target`].
pub type ObjectFile = Vec<u8>;

/// The symbol a compiled program's `main` is exported as; the runtime's
/// own `main` calls it.
pub const ENTRY_SYMBOL: &str = "__paco_entry";

/// An `i8` the code generators emit next to [`ENTRY_SYMBOL`]: 1 when it
/// returns an exit code, 0 when it returns `()`.
pub const ENTRY_RETURNS_VALUE_SYMBOL: &str = "__paco_entry_returns_value";

pub trait Backend {
    fn lower_body(&mut self, name: &str, body: &Body) -> Result<(), String>;
    /// Declares a body another object file defines, so calls to it link.
    fn declare_body(&mut self, name: &str, body: &Body) -> Result<(), String>;
    fn finish(self, target: &Target) -> Result<ObjectFile, String>;
}

/// Resolves spans to `(file, line, column)`, 1-based, with a line index per
/// file.
pub struct SourceLocator<'a> {
    sources: &'a paco_span::SourceMap,
    lines: std::cell::RefCell<std::collections::HashMap<paco_span::FileId, std::rc::Rc<Vec<usize>>>>,
}

impl<'a> SourceLocator<'a> {
    pub fn new(sources: &'a paco_span::SourceMap) -> Self {
        Self { sources, lines: Default::default() }
    }

    pub fn locate(&self, span: paco_span::Span) -> Option<(&'a str, u32, u32)> {
        let file = span.file_id();
        let text = self.sources.source(file)?;
        let name = self.sources.file_name(file)?;
        let starts = self
            .lines
            .borrow_mut()
            .entry(file)
            .or_insert_with(|| {
                std::rc::Rc::new(
                    std::iter::once(0).chain(text.match_indices('\n').map(|(index, _)| index + 1)).collect(),
                )
            })
            .clone();
        let offset = span.start().min(text.len());
        let line = starts.partition_point(|start| *start <= offset);
        let column = text.get(starts[line - 1]..offset)?.chars().count() + 1;
        Some((name, line as u32, column as u32))
    }
}
