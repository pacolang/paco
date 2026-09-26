use paco_borrow::check_module;
use paco_diag::Reporter;
use paco_span::SourceMap;
use paco_syntax::{lex::lex, parse::parse_module};

fn check_source(source: &str) -> Option<String> {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).expect("source should parse");

    if check_module(&module, &mut reporter).is_err() {
        Some(reporter.emit_to_string(&sources))
    } else {
        None
    }
}

#[test]
fn use_after_move_inside_an_unsafe_block_is_still_rejected() {
    let source = r#"
    fn consume(value: string) { }
    fn main() {
        let a = "hi";
        unsafe {
            consume(a);
            consume(a);
        }
    }
    "#;

    let error = check_source(source).expect("expected a borrow-check error");
    assert!(error.contains("use-after-move"), "{error}");
}
