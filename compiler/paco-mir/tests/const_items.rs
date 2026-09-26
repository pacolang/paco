use paco_diag::Reporter;
use paco_mir::{Body, Constant, Operand, Profile, Statement, TypeRegistry};
use paco_span::SourceMap;
use paco_syntax::ast::Item;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::{IntWidth, infer_module};

fn lower_source(source: &str) -> Body {
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
    paco_mir::lower_function(function, &typed, &registry, &drops, Profile::Debug).0
}

fn constant_operands(body: &Body) -> Vec<Constant> {
    let operand_constant = |operand: &Operand| match operand {
        Operand::Constant(constant) => Some(constant.clone()),
        _ => None,
    };
    body.blocks
        .iter()
        .flat_map(|block| &block.statements)
        .filter_map(|statement| match statement {
            Statement::Assign(_, rvalue) => Some(rvalue),
            _ => None,
        })
        .flat_map(|rvalue| match rvalue {
            paco_mir::Rvalue::Use(operand) | paco_mir::Rvalue::UnaryOp(_, operand) => {
                operand_constant(operand).into_iter().collect::<Vec<_>>()
            }
            paco_mir::Rvalue::BinaryOp(_, left, right) => {
                [operand_constant(left), operand_constant(right)]
                    .into_iter()
                    .flatten()
                    .collect()
            }
            _ => Vec::new(),
        })
        .collect()
}

#[test]
fn const_reference_lowers_to_its_literal_value() {
    let body = lower_source("const TILE: i64 = 64;\nfn main() -> i64 { TILE }");

    assert!(matches!(body.blocks.last().unwrap().terminator,
        paco_mir::Terminator::Return(Operand::Constant(Constant::Int(64, IntWidth::I64)))));
}

#[test]
fn const_arithmetic_is_folded_at_compile_time() {
    let body = lower_source("const TILE: i64 = 64;\nconst DOUBLE_TILE: i64 = TILE * 2;\nfn main() -> i64 { DOUBLE_TILE }");

    assert!(matches!(body.blocks.last().unwrap().terminator,
        paco_mir::Terminator::Return(Operand::Constant(Constant::Int(128, IntWidth::I64)))));
}

#[test]
fn const_used_in_an_arithmetic_expression_has_no_local_declared_for_it() {
    let body = lower_source("const TILE: i64 = 64;\nfn main() -> i64 { TILE + 1 }");

    assert!(
        constant_operands(&body).contains(&Constant::Int(64, IntWidth::I64)),
        "expected the const's literal value to appear directly: {body:#?}"
    );
    assert!(
        body.locals.iter().all(|local| local.name.as_deref() != Some("TILE")),
        "a const use should not declare a named local for it: {body:#?}"
    );
}
