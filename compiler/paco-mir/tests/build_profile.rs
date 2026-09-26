use paco_diag::Reporter;
use paco_mir::{Body, Profile, TypeRegistry};
use paco_span::SourceMap;
use paco_syntax::ast::Item;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::infer_module;

fn lower_source_with_profile(source: &str, profile: Profile) -> Body {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    assert!(!reporter.has_errors(), "{}", reporter.emit_to_string(&sources));

    let typed = infer_module(&module, &mut reporter).expect("module should type-check");
    let drops =
        paco_borrow::analyze_module(&module, &mut reporter).expect("module should borrow-check");
    let registry = TypeRegistry::from_module(&module);

    let function = module
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(function) if function.name == "main" => Some(function),
            _ => None,
        })
        .expect("expected a `main` function");
    paco_mir::lower_function(function, &typed, &registry, &drops, profile).0
}

#[test]
fn body_carries_the_requested_build_profile() {
    let debug_body = lower_source_with_profile("fn main() -> i64 { 1 }", Profile::Debug);
    let release_body = lower_source_with_profile("fn main() -> i64 { 1 }", Profile::Release);

    assert_eq!(debug_body.profile, Profile::Debug);
    assert_eq!(release_body.profile, Profile::Release);
}

#[test]
fn arithmetic_lowering_is_identical_across_build_profiles() {
    let debug_body = lower_source_with_profile("fn main() -> i64 { 1 + 2 * 3 }", Profile::Debug);
    let release_body =
        lower_source_with_profile("fn main() -> i64 { 1 + 2 * 3 }", Profile::Release);

    assert_eq!(debug_body.locals, release_body.locals);
    assert_eq!(debug_body.blocks, release_body.blocks);
}
