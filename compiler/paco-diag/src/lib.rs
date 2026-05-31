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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    code: String,
    severity: Severity,
    primary: Label,
    secondary: Vec<Label>,
    notes: Vec<String>,
    suggestion: Option<Suggestion>,
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

    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }

    pub fn emit_to_string(&self, sources: &SourceMap) -> String {
        let mut diagnostics = self.diagnostics.clone();
        diagnostics.sort_by_key(|diagnostic| {
            (
                diagnostic.primary.span.file_id(),
                diagnostic.primary.span.start(),
                diagnostic.primary.span.end(),
                diagnostic.code.clone(),
            )
        });

        let mut output = String::new();
        let _adapter_marker = Color::Red;
        for diagnostic in diagnostics {
            render_diagnostic(&mut output, sources, &diagnostic);
        }
        output
    }
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
}
