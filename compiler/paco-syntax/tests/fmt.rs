use paco_diag::Reporter;
use paco_span::{SourceMap, Span};
use paco_syntax::ast::{
    AssocTypeDecl, Block, FnSignature, Item, Module, Param, Pat, TraitDecl, Ty,
};
use paco_syntax::fmt::format_module;
use paco_syntax::{lex::lex, parse::parse_module};

#[test]
fn formats_a_trait_with_abstract_method_default_method_and_assoc_type() {
    let span = Span::new_root(0, 0);
    let module = Module {
        name: None,
        items: vec![Item::Trait(TraitDecl {
            name: "Shape".to_string(),
            generics: Vec::new(),
            methods: vec![
                FnSignature {
                    name: "area".to_string(),
                    generics: Vec::new(),
                    params: vec![Param {
                        pattern: Pat::Ident("self".to_string(), span),
                        ty: Ty::Path(vec!["Self".to_string()], span),
                        span,
                    }],
                    return_ty: Some(Ty::Path(vec!["float".to_string()], span)),
                    body: None,
                    attrs: Vec::new(),
                    span,
                },
                FnSignature {
                    name: "describe".to_string(),
                    generics: Vec::new(),
                    params: Vec::new(),
                    return_ty: None,
                    body: Some(Block {
                        stmts: Vec::new(),
                        tail: None,
                        span,
                    }),
                    attrs: Vec::new(),
                    span,
                },
            ],
            consts: Vec::new(),
            assoc_types: vec![AssocTypeDecl {
                name: "Output".to_string(),
                default: None,
                span,
            }],
            is_pub: false,
            attrs: Vec::new(),
            span,
        })],
        span,
    };

    let output = format_module(&module, None);

    assert!(output.contains("trait Shape {"));
    assert!(output.contains("fn area(self) -> float;"));
    assert!(output.contains("fn describe() {}"));
    assert!(output.contains("type Output;"));
}

fn parse_source(source: &str) -> Module {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    assert!(!reporter.has_errors());
    module
}

#[test]
fn iter_fn_round_trips_through_parse_and_fmt() {
    let source = "iter fn counter() {\n    yield 1\n}\n";
    let module = parse_source(source);

    let Item::Fn(function) = &module.items[0] else {
        panic!("expected fn item");
    };
    assert!(function.is_iter);

    let formatted = format_module(&module, Some(source));
    assert!(formatted.contains("iter fn counter()"));

    let reformatted = parse_source(&formatted);
    let Item::Fn(function) = &reformatted.items[0] else {
        panic!("expected fn item");
    };
    assert!(function.is_iter);
}

#[test]
fn closures_round_trip_through_parse_and_fmt() {
    let source = "fn main() {\n    let f = |x: i64, y| x + y;\n    let g = || {\n        1\n    };\n}\n";
    let formatted = format_module(&parse_source(source), Some(source));
    assert!(formatted.contains("|x: i64, y| x + y"), "{formatted}");
    assert!(formatted.contains("|| {"), "{formatted}");
    assert_eq!(format_module(&parse_source(&formatted), Some(&formatted)), formatted);
}

#[test]
fn function_types_round_trip_through_parse_and_fmt() {
    let source = "struct S { op: fn(i64) -> i64, }\nfn make(n: i64) -> fn(i64, bool) -> fn() {\n    let f: fn(Vec<fn(string) -> string>) = |v| ();\n    f\n}\n";
    let module = parse_source(source);
    let Item::Fn(function) = &module.items[1] else { panic!("expected a function") };
    assert!(matches!(
        &function.return_ty,
        Some(Ty::Fn { params, return_ty: Some(ret), .. })
            if params.len() == 2 && matches!(ret.as_ref(), Ty::Fn { params, return_ty: None, .. } if params.is_empty())
    ));
    let formatted = format_module(&module, Some(source));
    assert!(formatted.contains("op: fn(i64) -> i64,"), "{formatted}");
    assert!(formatted.contains("-> fn(i64, bool) -> fn() {"), "{formatted}");
    assert!(formatted.contains("let f: fn(Vec<fn(string) -> string>) = |v| ();"), "{formatted}");
    assert_eq!(format_module(&parse_source(&formatted), Some(&formatted)), formatted);
}

#[test]
fn c_function_pointer_types_round_trip_through_parse_and_fmt() {
    let source = "extern \"C\" {\n    fn qsort(base: *mut i64, n: u64, size: u64, cmp: extern \"C\" fn(*const u8, *const u8) -> i32);\n}\nfn take(cb: unsafe extern \"C\" fn(i64) -> i64) -> i64 {\n    0\n}\n";
    let module = parse_source(source);
    let Item::Extern(block) = &module.items[0] else { panic!("expected an extern block") };
    let cmp_ty = &block.functions[0].params.last().unwrap().ty;
    assert!(matches!(cmp_ty, Ty::Fn { extern_abi: Some(abi), is_unsafe: false, .. } if abi == "C"));
    let Item::Fn(take) = &module.items[1] else { panic!("expected a function") };
    assert!(matches!(&take.params[0].ty, Ty::Fn { extern_abi: Some(abi), is_unsafe: true, .. } if abi == "C"));
    let formatted = format_module(&module, Some(source));
    assert!(formatted.contains("cmp: extern \"C\" fn(*const u8, *const u8) -> i32"), "{formatted}");
    assert!(formatted.contains("cb: unsafe extern \"C\" fn(i64) -> i64"), "{formatted}");
    assert_eq!(format_module(&parse_source(&formatted), Some(&formatted)), formatted);
}

#[test]
fn reference_patterns_round_trip_through_parse_and_fmt() {
    let source = "fn main() -> i64 {\n    match pair {\n        &(a, b) => a,\n        &mut Maybe::Some(x) => x,\n        _ => 0,\n    }\n}\n";
    let module = parse_source(source);
    let Item::Fn(function) = &module.items[0] else { panic!("expected a function") };
    let Some(tail) = &function.body.tail else { panic!("expected tail expression") };
    let paco_syntax::ast::Expr::Match { arms, .. } = tail.as_ref() else { panic!("expected match expression") };
    assert!(matches!(&arms[0].pattern, Pat::Reference { mutable: false, .. }));
    assert!(matches!(&arms[1].pattern, Pat::Reference { mutable: true, .. }));
    let formatted = format_module(&module, Some(source));
    assert!(formatted.contains("&(a, b)"), "{formatted}");
    assert!(formatted.contains("&mut Maybe::Some(x)"), "{formatted}");
    assert_eq!(format_module(&parse_source(&formatted), Some(&formatted)), formatted);
}

#[test]
fn formatter_emits_semicolons_and_is_idempotent() {
    let source = "module m;\nuse stdlib::io as sio;\nconst A: i64 = 1;\nstruct P { x: i64, const K: i64 = 2; }\nfn main() {\n    let mut y = 1;\n    let r = &mut y;\n    *r = 5;\n    if y > 0 { print(y); }\n    while false {}\n    loop { break; };\n    y\n}\n";
    let formatted = format_module(&parse_source(source), Some(source));
    assert!(formatted.starts_with("module m;\n"));
    assert!(formatted.contains("use stdlib::io as sio;"));
    assert!(formatted.contains("const A: i64 = 1;"));
    assert!(formatted.contains("    let r = &mut y;\n    *r = 5;\n"));
    assert!(formatted.contains("    while false {}\n"));
    assert!(formatted.contains("break;"));
    assert_eq!(format_module(&parse_source(&formatted), Some(&formatted)), formatted);
}

#[test]
fn bitwise_operators_round_trip_through_parse_and_fmt() {
    let source = "fn main() {\n    let m = (a | b) & ~c ^ d << 2 >> e;\n    let n = a & b == 0;\n    let o = (a + b) << 1;\n}\n";
    let formatted = format_module(&parse_source(source), Some(source));
    assert!(formatted.contains("(a | b) & ~c ^ d << 2 >> e"), "{formatted}");
    assert!(formatted.contains("a & b == 0"), "{formatted}");
    assert!(formatted.contains("a + b << 1"), "{formatted}");
    assert_eq!(format_module(&parse_source(&formatted), Some(&formatted)), formatted);
}
