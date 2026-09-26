//! Diagnostic collection and rendering primitives.

use std::fmt::Write as _;

use ariadne::Color;
use paco_span::{SourceMap, Span};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Severity {
    Error,
    Warning,
    Note,
    Help,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Note => "note",
            Self::Help => "help",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Label {
    pub span: Span,
    pub message: String,
}

impl Label {
    pub fn new(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Suggestion {
    pub span: Span,
    pub replacement: String,
    pub message: String,
}

impl Suggestion {
    pub fn new(span: Span, replacement: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            span,
            replacement: replacement.into(),
            message: message.into(),
        }
    }
}

/// One replacement of the bytes `span` covers, in the unchanged source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Edit {
    pub span: Span,
    pub replacement: String,
}

impl Edit {
    pub fn new(span: Span, replacement: impl Into<String>) -> Self {
        Self { span, replacement: replacement.into() }
    }
}

/// A machine-applicable repair; a diagnostic ranks at most two.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fix {
    pub title: String,
    pub edits: Vec<Edit>,
}

pub const MAX_FIXES: usize = 2;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    code: String,
    severity: Severity,
    primary: Label,
    secondary: Vec<Label>,
    notes: Vec<String>,
    suggestion: Option<Suggestion>,
    fixes: Vec<Fix>,
}

impl Diagnostic {
    pub fn new(
        code: impl Into<String>,
        severity: Severity,
        span: Span,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            severity,
            primary: Label::new(span, message),
            secondary: Vec::new(),
            notes: Vec::new(),
            suggestion: None,
            fixes: Vec::new(),
        }
    }

    pub fn error(code: impl Into<String>, span: Span, message: impl Into<String>) -> Self {
        Self::new(code, Severity::Error, span, message)
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn severity(&self) -> Severity {
        self.severity
    }

    pub fn primary(&self) -> &Label {
        &self.primary
    }

    pub fn secondary(&self) -> &[Label] {
        &self.secondary
    }

    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    pub fn suggestion(&self) -> Option<&Suggestion> {
        self.suggestion.as_ref()
    }

    pub fn fixes(&self) -> &[Fix] {
        &self.fixes
    }

    pub fn with_fix(mut self, title: impl Into<String>, edits: Vec<Edit>) -> Self {
        if self.fixes.len() < MAX_FIXES {
            self.fixes.push(Fix { title: title.into(), edits });
        }
        self
    }

    pub fn set_message(&mut self, message: impl Into<String>) {
        self.primary.message = message.into();
    }

    pub fn with_secondary(mut self, span: Span, message: impl Into<String>) -> Self {
        self.secondary.push(Label::new(span, message));
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    pub fn with_suggestion(mut self, suggestion: Suggestion) -> Self {
        self.suggestion = Some(suggestion);
        self
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Reporter {
    diagnostics: Vec<Diagnostic>,
}

impl Reporter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn diagnostics_mut(&mut self) -> &mut [Diagnostic] {
        &mut self.diagnostics
    }

    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }

    pub fn emit_to_string(&self, sources: &SourceMap) -> String {
        let mut output = String::new();
        let _adapter_marker = Color::Red;
        for diagnostic in self.sorted() {
            render_diagnostic(&mut output, sources, &diagnostic);
        }
        output
    }

    /// One JSON object per diagnostic and line, for tools and agents.
    pub fn emit_json(&self, sources: &SourceMap) -> String {
        let mut output = String::new();
        for diagnostic in self.sorted() {
            render_json(&mut output, sources, &diagnostic);
        }
        output
    }

    fn sorted(&self) -> Vec<Diagnostic> {
        let mut diagnostics = self.diagnostics.clone();
        diagnostics.sort_by_key(|diagnostic| {
            (
                diagnostic.primary.span.file_id(),
                diagnostic.primary.span.start(),
                diagnostic.primary.span.end(),
                diagnostic.code.clone(),
            )
        });
        diagnostics
    }
}

fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_location(sources: &SourceMap, span: Span) -> String {
    match sources.location(span) {
        Ok(location) => format!(
            "\"file\":{},\"line\":{},\"column\":{}",
            json_string(&location.file_name),
            location.start.line,
            location.start.column
        ),
        Err(_) => "\"file\":null,\"line\":null,\"column\":null".to_string(),
    }
}

fn render_json(output: &mut String, sources: &SourceMap, diagnostic: &Diagnostic) {
    let mut notes: Vec<String> = diagnostic
        .secondary
        .iter()
        .map(|label| format!("{{\"message\":{},{}}}", json_string(&label.message), json_location(sources, label.span)))
        .collect();
    notes.extend(
        diagnostic
            .notes
            .iter()
            .map(|note| format!("{{\"message\":{},{}}}", json_string(note), json_location(sources, diagnostic.primary.span))),
    );
    let fixes: Vec<String> = diagnostic
        .fixes
        .iter()
        .enumerate()
        .map(|(index, fix)| {
            let edits: Vec<String> = fix
                .edits
                .iter()
                .map(|edit| {
                    let file = sources.file_name(edit.span.file_id()).map_or("null".to_string(), json_string);
                    format!(
                        "{{\"file\":{file},\"start\":{},\"end\":{},\"replacement\":{}}}",
                        edit.span.start(),
                        edit.span.end(),
                        json_string(&edit.replacement)
                    )
                })
                .collect();
            format!("{{\"rank\":{},\"title\":{},\"edits\":[{}]}}", index + 1, json_string(&fix.title), edits.join(","))
        })
        .collect();
    let _ = writeln!(
        output,
        "{{\"code\":{},\"severity\":{},\"message\":{},{},\"notes\":[{}],\"fixes\":[{}]}}",
        json_string(&diagnostic.code),
        json_string(diagnostic.severity.as_str()),
        json_string(&diagnostic.primary.message),
        json_location(sources, diagnostic.primary.span),
        notes.join(","),
        fixes.join(",")
    );
}

fn render_diagnostic(output: &mut String, sources: &SourceMap, diagnostic: &Diagnostic) {
    let location = sources.location(diagnostic.primary.span).ok();
    let _ = write!(
        output,
        "{}[{}]",
        diagnostic.severity.as_str(),
        diagnostic.code
    );

    if let Some(location) = location {
        let _ = write!(
            output,
            " {}:{}:{}",
            location.file_name, location.start.line, location.start.column
        );
    }

    let _ = writeln!(output, ": {}", diagnostic.primary.message);

    for label in &diagnostic.secondary {
        if let Ok(location) = sources.location(label.span) {
            let _ = writeln!(
                output,
                "  note at {}:{}:{}: {}",
                location.file_name, location.start.line, location.start.column, label.message
            );
        }
    }

    for note in &diagnostic.notes {
        let _ = writeln!(output, "  note: {note}");
    }

    if let Some(suggestion) = &diagnostic.suggestion {
        let _ = writeln!(
            output,
            "  help: {} -> `{}`",
            suggestion.message, suggestion.replacement
        );
    }

    for (index, fix) in diagnostic.fixes.iter().enumerate() {
        let _ = writeln!(output, "  fix {}: {}", index + 1, fix.title);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paco_span::SourceMap;

    #[test]
    fn comptime_error_renders_with_the_failing_expressions_source_span() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("main.paco", "fn main() {\n    comptime { read_file(\"x\") }\n}\n");
        let span = Span::new(file, 16, 34);

        let mut reporter = Reporter::new();
        reporter.push(Diagnostic::error(
            "PACO-E0350",
            span,
            "comptime evaluation failed: I/O is not allowed inside `comptime`",
        ));

        let output = reporter.emit_to_string(&sources);
        assert!(output.contains("PACO-E0350"));
        assert!(output.contains("main.paco:2:5"));
        assert!(output.contains("comptime evaluation failed"));
    }
}
