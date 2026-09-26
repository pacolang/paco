use paco_diag::Reporter;
use paco_mir::{BinOp, Body, Profile, Rvalue, Statement, Terminator, TypeRegistry};
use paco_span::SourceMap;
use paco_syntax::ast::Item;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::infer_module;

const PROGRAM: &str = r#"
enum Result<T, E> { Ok(T), Err(E) }
struct DimError { axis: i64, expected: string, origin: string, bound: i64, observed: i64 }
struct Grid<T: Numeric + Add, const D: int...> {
    data: []T,
    dims: []i64,
    fn extent(&self, axis: i64) -> i64 { self.dims[axis] }
    fn add(&self, other: &Grid<T, D...>) -> Grid<T, D...> { Grid { data: slice_of_zeros<T>(0), dims: slice_of_zeros<i64>(0) } }
}
fn step(a: Grid<f32, Dyn, 784>, b: Grid<f32, Dyn, 784>) -> Result<i64, DimError> {
    let n = a.dim(0);
    let a: Grid<f32, n, 784> = a.with_dims()?;
    let b: Grid<f32, n, 784> = b.with_dims()?;
    let mut c = a.add(&b);
    let mut i = 0;
    while i < 1000 {
        c = c + &b;
        i = i + 1;
    }
    Result::Ok(n)
}
fn rows<dim B>(g: &Grid<f32, B, 784>) -> i64 { B }
fn main() {}
"#;

fn lower(name: &str) -> Body {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", PROGRAM);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    let typed = infer_module(&module, &mut reporter).unwrap_or_else(|_| panic!("{}", reporter.emit_to_string(&sources)));
    let drops = paco_borrow::analyze_module(&module, &mut reporter).unwrap_or_else(|_| panic!("{}", reporter.emit_to_string(&sources)));
    let registry = TypeRegistry::from_module(&module);
    let function = module
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(function) if function.name == name => Some(function),
            _ => None,
        })
        .unwrap();
    paco_mir::lower_function(function, &typed, &registry, &drops, Profile::Release).0
}

fn calls(body: &Body, prefix: &str) -> Vec<usize> {
    body.blocks
        .iter()
        .enumerate()
        .filter(|(_, block)| matches!(&block.terminator, Terminator::Call { target, .. } if target.0.starts_with(prefix)))
        .map(|(index, _)| index)
        .collect()
}

fn comparisons(body: &Body, op: BinOp) -> usize {
    body.blocks
        .iter()
        .flat_map(|block| &block.statements)
        .filter(|statement| matches!(statement, Statement::Assign(_, Rvalue::BinaryOp(found, ..)) if *found == op))
        .count()
}

#[test]
fn refined_operands_are_compared_once_at_the_boundary_and_never_in_the_loop() {
    let body = lower("step");
    assert_eq!(calls(&body, "Grid::extent").len(), 2, "one extent read per opened parameter");
    assert_eq!(comparisons(&body, BinOp::Ne), 1, "only `b` is compared: `a` is refined by its own witness");
    let loop_adds = calls(&body, "Grid::add");
    assert_eq!(loop_adds.len(), 2);
    let first_extent = calls(&body, "Grid::extent")[1];
    assert!(loop_adds.iter().all(|block| *block > first_extent), "the extents are read before any addition");
    for block in &body.blocks {
        if let Terminator::Call { target, args, .. } = &block.terminator
            && target.0.starts_with("Grid::add")
        {
            assert_eq!(args.len(), 3, "one hidden extent, the receiver and the operand: {args:?}");
        }
    }
}

#[test]
fn a_dim_parameter_is_one_hidden_leading_argument() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", PROGRAM);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    let typed = infer_module(&module, &mut reporter).unwrap();
    let drops = paco_borrow::analyze_module(&module, &mut reporter).unwrap();
    let registry = TypeRegistry::from_module(&module);
    let Some(Item::Fn(rows)) = module.items.iter().find(|item| matches!(item, Item::Fn(function) if function.name == "rows")) else {
        panic!("rows")
    };
    let args = vec![paco_types::Type::Dim(paco_types::Dim::Dyn)];
    let key = paco_mir::dims::instance_args(&[], Some(rows), &args);
    let slots = paco_mir::dims::hidden_slots(&[], Some(rows), &key);
    assert_eq!(slots.len(), 1);
    let hidden = paco_types::fresh_atom("B");
    let substitutions = [("B".to_string(), paco_types::Type::Generic(hidden.clone()))].into_iter().collect();
    let instances = paco_mir::InstantiationRegistry::new();
    let (body, _) =
        paco_mir::lower_instance(rows, &typed, &registry, &drops, Profile::Release, &substitutions, &[hidden], &instances, &Default::default());
    assert_eq!(body.param_count, 2);
    assert_eq!(body.locals[0].ty, paco_types::Type::Int(paco_types::IntWidth::I64));
    assert!(matches!(&body.blocks[0].terminator, Terminator::Return(paco_mir::Operand::Copy(paco_mir::Place::Local(local))) if local.0 == 0));
}
