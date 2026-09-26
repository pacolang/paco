use paco_diag::{Diagnostic, Edit, Reporter, Severity};
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

#[test]
fn fixes_are_ranked_capped_at_two_and_rendered() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "fn main() {\n    let c = a + b;\n}\n");
    let span = Span::new(file, 24, 29);
    let mut reporter = Reporter::new();
    reporter.push(
        Diagnostic::error("PACO-E0342", span, "cannot prove these dimensions are equal")
            .with_secondary(Span::new(file, 20, 21), "`c` bound here")
            .with_fix("first", vec![Edit::new(Span::new(file, 16, 16), "let n = 1; ")])
            .with_fix("second", vec![Edit::new(span, "a.checked_add(&b)")])
            .with_fix("third", Vec::new()),
    );
    let diagnostic = &reporter.diagnostics()[0];
    assert_eq!(diagnostic.fixes().len(), 2);
    let text = reporter.emit_to_string(&sources);
    assert!(text.contains("  fix 1: first\n  fix 2: second\n"), "{text}");
    assert!(!text.contains("third"));
}

#[test]
fn json_output_is_one_object_per_line_with_notes_and_fix_edits() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "fn main() {\n    let c = \"a\" + b;\n}\n");
    let span = Span::new(file, 24, 31);
    let mut reporter = Reporter::new();
    reporter.push(
        Diagnostic::error("PACO-E0342", span, "cannot \"prove\"")
            .with_secondary(Span::new(file, 20, 21), "bound here")
            .with_note("plain note")
            .with_fix("use checked_add", vec![Edit::new(span, "x\ny")]),
    );
    reporter.push(Diagnostic::error("PACO-E0336", Span::new(file, 0, 2), "second"));
    let json = reporter.emit_json(&sources);
    let lines: Vec<&str> = json.lines().collect();
    assert_eq!(lines.len(), 2, "{json}");
    assert_eq!(
        lines[0],
        r#"{"code":"PACO-E0336","severity":"error","message":"second","file":"main.paco","line":1,"column":1,"notes":[],"fixes":[]}"#
    );
    assert_eq!(
        lines[1],
        r#"{"code":"PACO-E0342","severity":"error","message":"cannot \"prove\"","file":"main.paco","line":2,"column":13,"notes":[{"message":"bound here","file":"main.paco","line":2,"column":9},{"message":"plain note","file":"main.paco","line":2,"column":13}],"fixes":[{"rank":1,"title":"use checked_add","edits":[{"file":"main.paco","start":24,"end":31,"replacement":"x\ny"}]}]}"#
    );
}
