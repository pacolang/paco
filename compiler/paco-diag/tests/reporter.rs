use paco_diag::{Diagnostic, Reporter, Severity};
use paco_span::{SourceMap, Span};

#[test]
fn reporter_collects_diagnostics_without_emitting() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "fn main() {}");
    let span = Span::new(file, 0, 2);

    let mut reporter = Reporter::new();
    reporter.push(
        Diagnostic::new("PACO-E0001", Severity::Error, span, "expected an item")
            .with_note("items start with declarations such as fn"),
    );

    assert!(reporter.has_errors());
    assert_eq!(reporter.diagnostics().len(), 1);
    assert_eq!(reporter.diagnostics()[0].code(), "PACO-E0001");
}

#[test]
fn reporter_emits_diagnostics_only_when_requested() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "fn main() {}");
    let span = Span::new(file, 0, 2);

    let mut reporter = Reporter::new();
    reporter.push(Diagnostic::error("PACO-E0002", span, "invalid declaration"));

    let output = reporter.emit_to_string(&sources);

    assert!(output.contains("PACO-E0002"));
    assert!(output.contains("invalid declaration"));
    assert!(output.contains("main.paco:1:1"));
}
