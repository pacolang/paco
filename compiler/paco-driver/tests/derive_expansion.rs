use paco_diag::Reporter;
use paco_driver::expand_derives;
use paco_span::SourceMap;
use paco_syntax::{ast::Item, lex::lex, parse::parse_module};

fn parse(source: &str) -> paco_syntax::ast::Module {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).expect("source should parse");
    assert!(!reporter.has_errors(), "{}", reporter.emit_to_string(&sources));
    module
}

// `phase-9-comptime` task 6.2: `#[derive(Trait, ...)]` expands, via the
// trait's own `#[derivable(fn_name)]` registration, to a real
// `methods Foo { .. }` block — reachable by normal structural (ADR 0002)
// method resolution, never a nominal `impl` block (Paco has none).

#[test]
fn deriving_a_registered_trait_produces_a_methods_block() {
    let module = parse(
        r#"
#[derivable(derive_greeting)]
trait Greeting { fn greet(&self) -> string; }

comptime fn derive_greeting(t: type) -> Code {
    quote {
        methods #(t) {
            fn greet(&self) -> string {
                "hello"
            }
        }
    }
}

#[derive(Greeting)]
struct User { name: string }
"#,
    );

    let mut reporter = Reporter::new();
    let expanded = expand_derives(&module, &[], &mut reporter)
        .unwrap_or_else(|_| panic!("expected expansion to succeed: {:?}", reporter.diagnostics()));

    let has_methods_block = expanded.items.iter().any(|item| {
        matches!(item, Item::Methods(block) if matches!(&block.target, paco_syntax::ast::Ty::Path(path, _) if path == &["User".to_string()]))
    });
    assert!(has_methods_block, "{expanded:?}");
}

// `phase-9-comptime` task 6.3: deriving an unregistered trait is a
// compile error, not a silent no-op.
#[test]
fn deriving_an_unknown_trait_is_a_compile_error() {
    let module = parse(
        r#"
#[derive(NotRegistered)]
struct User { name: string }
"#,
    );

    let mut reporter = Reporter::new();
    let result = expand_derives(&module, &[], &mut reporter);
    assert!(result.is_err());
    assert!(reporter.diagnostics().iter().any(|d| d.code() == "PACO-E0353"));
}
