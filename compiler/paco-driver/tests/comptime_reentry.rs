use paco_diag::Reporter;
use paco_driver::check_generated_item;
use paco_span::SourceMap;
use paco_syntax::{ast::Item, lex::lex, parse::parse_module};

fn parse(source: &str) -> paco_syntax::ast::Module {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    parse_module(&tokens, &mut reporter).expect("source should parse")
}

// `phase-9-comptime` task 5.5: a `Code` value's own generated `Item`,
// spliced into a module by `check_generated_item`, is name-resolved and
// type-checked exactly like hand-written code — not a special-cased,
// less-checked path.

#[test]
fn a_well_formed_generated_methods_block_type_checks_cleanly() {
    let module = parse("struct User { name: string }\nfn main() {}");
    let generated_module = parse(
        "methods User { fn greeting(&self) -> string { self.name } }",
    );
    let Item::Methods(_) = &generated_module.items[0] else {
        panic!("expected a methods item");
    };
    let generated = generated_module.items.into_iter().next().unwrap();

    let mut reporter = Reporter::new();
    let result = check_generated_item(&module, generated, &[], &mut reporter);
    assert!(result.is_ok(), "{:?}", reporter.diagnostics());
}

#[test]
fn a_type_error_in_generated_code_is_reported_as_an_ordinary_diagnostic() {
    let module = parse("struct User { name: string }\nfn main() {}");
    // Deliberately broken: declares `-> i64` but returns a `bool`.
    let generated_module = parse("methods User { fn broken(&self) -> i64 { true } }");
    let generated = generated_module.items.into_iter().next().unwrap();

    let mut reporter = Reporter::new();
    let result = check_generated_item(&module, generated, &[], &mut reporter);
    assert!(result.is_err());
    assert!(reporter.diagnostics().iter().any(|d| d.code() == "PACO-E0302"));
}
