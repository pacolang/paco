use paco_diag::Reporter;
use paco_hir::{DefKind, lower_module};
use paco_span::SourceMap;
use paco_syntax::{lex::lex, parse::parse_module};

#[test]
fn hir_lowers_struct_fields_and_methods_to_type_namespace() {
    let module = parse_source("struct Point { x: int, fn value(self&) -> int { self.x } }");

    let hir = lower_module(&module).unwrap();

    let point = hir
        .defs
        .iter()
        .find(|def| def.name == "Point")
        .expect("expected Point definition");
    assert_eq!(point.kind, DefKind::Struct);
    assert!(hir.defs.iter().any(|def| {
        def.parent == Some(point.id) && def.name == "x" && def.kind == DefKind::Field
    }));
    assert!(hir.defs.iter().any(|def| {
        def.parent == Some(point.id) && def.name == "value" && def.kind == DefKind::Method
    }));
}

#[test]
fn hir_resolves_module_local_type_paths() {
    let module = parse_source("struct Point { x: int } fn make() -> Point { Point { x: 1 } }");

    let hir = lower_module(&module).unwrap();

    let point = hir
        .defs
        .iter()
        .find(|def| def.name == "Point")
        .expect("expected Point definition");
    let make = hir
        .defs
        .iter()
        .find(|def| def.name == "make")
        .expect("expected make definition");
    assert_eq!(make.references, vec![point.id]);
}

#[test]
fn hir_records_type_references_from_declarations() {
    let module = parse_source(
        "struct Point { x: int } struct Line { start: Point } enum Maybe { Some(Point), None } fn shift(p: Point) -> Point { p }",
    );

    let hir = lower_module(&module).unwrap();

    let point = hir
        .defs
        .iter()
        .find(|def| def.kind == DefKind::Struct && def.name == "Point")
        .expect("expected Point definition");
    let line = hir
        .defs
        .iter()
        .find(|def| def.kind == DefKind::Struct && def.name == "Line")
        .expect("expected Line definition");
    let maybe = hir
        .defs
        .iter()
        .find(|def| def.kind == DefKind::Enum && def.name == "Maybe")
        .expect("expected Maybe definition");
    let shift = hir
        .defs
        .iter()
        .find(|def| def.kind == DefKind::Function && def.name == "shift")
        .expect("expected shift definition");

    assert!(line.references.contains(&point.id));
    assert!(maybe.references.contains(&point.id));
    assert!(shift.references.contains(&point.id));
}

fn parse_source(source: &str) -> paco_syntax::ast::Module {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    assert!(
        !reporter.has_errors(),
        "{}",
        reporter.emit_to_string(&sources)
    );
    module
}
