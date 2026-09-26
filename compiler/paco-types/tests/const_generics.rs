use paco_diag::Reporter;
use paco_span::SourceMap;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::check_module;

const TENSOR: &str = "
struct Tensor<T, const D: int...> {
    data: []T,
    dyn_dims: []i64,
}
";

fn check_source(source: &str) -> Option<String> {
    let source = format!("{TENSOR}{source}");
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = match parse_module(&tokens, &mut reporter) {
        Ok(module) if !reporter.has_errors() => module,
        _ => return Some(reporter.emit_to_string(&sources)),
    };
    match check_module(&module, &mut reporter) {
        Ok(()) if !reporter.has_errors() => None,
        _ => Some(reporter.emit_to_string(&sources)),
    }
}

fn accepts(source: &str) {
    let error = check_source(source);
    assert!(error.is_none(), "{}", error.unwrap());
}

fn rejects(source: &str) -> String {
    check_source(source).expect("expected a type error")
}

#[test]
fn a_struct_with_a_scalar_const_param_resolves_its_fields_with_the_bound_dim() {
    accepts(
        "
struct Row<const N: int> { cells: Tensor<i64, N> }
fn take(t: Tensor<i64, 4>) {}
fn main(r: Row<4>) { take(r.cells) }
",
    );
    let error = rejects(
        "
struct Row<const N: int> { cells: Tensor<i64, N> }
fn take(t: Tensor<i64, 4>) {}
fn main(r: Row<5>) { take(r.cells) }
",
    );
    assert!(error.contains("PACO-E0336"), "{error}");
}

#[test]
fn matching_static_dimensions_type_check() {
    accepts("fn f(a: &Tensor<i64, 768, 3072>) {} fn main(t: Tensor<i64, 768, 3072>) { f(&t) }");
}

#[test]
fn mismatched_static_dimensions_report_both_values_at_the_call() {
    let error = rejects("fn f(a: &Tensor<i64, 768, 3072>) {}\nfn main(t: Tensor<i64, 512, 3072>) { f(&t) }");
    assert!(error.contains("PACO-E0336"), "{error}");
    assert!(error.contains("dimension 0 expected `768`, found `512`"), "{error}");
    assert!(error.contains("main.paco:7:38:"), "the call expression is the primary span: {error}");
}

#[test]
fn a_rank_mismatch_is_a_shape_error() {
    let error = rejects("fn f(a: &Tensor<i64, 2, 3>) {} fn main(t: Tensor<i64, 2, 3, 4>) { f(&t) }");
    assert!(error.contains("expected rank 2, found rank 3"), "{error}");
}

#[test]
fn packs_of_zero_one_and_many_dimensions_are_distinct_types() {
    accepts("fn f(a: Tensor<i64>) {} fn main(t: Tensor<i64>) { f(t) }");
    accepts("fn f(a: Tensor<i64, 9>) {} fn main(t: Tensor<i64, 9>) { f(t) }");
    accepts("fn f(a: Tensor<i64, 1, 2, 3, 4, 5>) {} fn main(t: Tensor<i64, 1, 2, 3, 4, 5>) { f(t) }");
    rejects("fn f(a: Tensor<i64>) {} fn main(t: Tensor<i64, 1>) { f(t) }");
}

#[test]
fn a_pack_struct_needs_its_fixed_params() {
    let error = rejects("fn main(t: Tensor) {}");
    assert!(error.contains("PACO-E0316"), "{error}");
}

#[test]
fn fixed_arity_generics_are_unaffected() {
    accepts("struct Pair<A, B> { a: A, b: B } fn main(p: Pair<i64, bool>) { let x: i64 = p.a; }");
    let error = rejects("struct Pair<A, B> { a: A, b: B } fn main(p: Pair<i64>) {}");
    assert!(error.contains("expected 2, found 1"), "{error}");
}

#[test]
fn a_type_in_a_dimension_position_is_rejected() {
    let error = rejects("fn main(t: Tensor<i64, bool>) {}");
    assert!(error.contains("PACO-E0338"), "{error}");
}

#[test]
fn a_constant_in_a_type_position_is_rejected() {
    let error = rejects("fn main(t: Tensor<3, 4>) {}");
    assert!(error.contains("PACO-E0338"), "{error}");
}

#[test]
fn const_params_are_inferred_from_argument_types() {
    accepts(
        "
fn matmul<const M: int, const K: int, const N: int>(a: &Tensor<i64, M, K>, b: &Tensor<i64, K, N>) -> Tensor<i64, M, N> {
    Tensor { data: slice_of_zeros<i64>(M * N), dyn_dims: slice_of_zeros<i64>(0) }
}
fn main(a: Tensor<i64, 2, 3>, b: Tensor<i64, 3, 4>) {
    let c: Tensor<i64, 2, 4> = matmul(&a, &b);
}
",
    );
    let error = rejects(
        "
fn matmul<const M: int, const K: int, const N: int>(a: &Tensor<i64, M, K>, b: &Tensor<i64, K, N>) -> Tensor<i64, M, N> {
    Tensor { data: slice_of_zeros<i64>(M * N), dyn_dims: slice_of_zeros<i64>(0) }
}
fn main(a: Tensor<i64, 2, 3>, b: Tensor<i64, 5, 4>) {
    let c = matmul(&a, &b);
}
",
    );
    assert!(error.contains("PACO-E0336"), "{error}");
}

#[test]
fn commutative_reordering_unifies() {
    accepts(
        "
fn f<const M: int>(a: Tensor<i64, M * 2>) -> Tensor<i64, 2 * M> { a }
fn main() {}
",
    );
}

#[test]
fn repeated_addition_equals_multiplication() {
    accepts(
        "
fn f<const M: int>(a: Tensor<i64, M + M>) -> Tensor<i64, 2 * M> { a }
fn g<const N: int>(a: Tensor<i64, (N + 1) * 2>) -> Tensor<i64, 2 * N + 2> { a }
fn h(a: Tensor<i64, 28 * 28>) -> Tensor<i64, 784> { a }
fn main() {}
",
    );
}

#[test]
fn division_is_opaque_and_cannot_be_proved() {
    let error = rejects("fn f<const N: int>(a: Tensor<i64, (2 * N) / 2>) -> Tensor<i64, N> { a } fn main() {}");
    assert!(error.contains("PACO-E0342"), "{error}");
    assert!(!error.contains("PACO-E0336"), "{error}");
}

#[test]
fn provably_different_dimensions_are_incompatible() {
    let error = rejects("fn f<const N: int>(a: Tensor<i64, 2 * N>) -> Tensor<i64, 2 * N + 1> { a } fn main() {}");
    assert!(error.contains("PACO-E0336"), "{error}");
}

#[test]
fn a_parameter_is_inferred_from_an_offset() {
    accepts("fn shrink<const N: int>(g: Tensor<i64, N + 1>) -> Tensor<i64, N> { Tensor { data: slice_of_zeros<i64>(N), dyn_dims: slice_of_zeros<i64>(0) } } fn main(t: Tensor<i64, 8>) { let s: Tensor<i64, 7> = shrink(t); }");
}

#[test]
fn a_non_invertible_position_needs_an_explicit_argument() {
    let source = "fn half<const N: int>(g: Tensor<i64, 2 * N>) -> Tensor<i64, N> { Tensor { data: slice_of_zeros<i64>(N), dyn_dims: slice_of_zeros<i64>(0) } }";
    let error = rejects(&format!("{source} fn main(t: Tensor<i64, 8>) {{ let h = half(t); }}"));
    assert!(error.contains("PACO-E0344"), "{error}");
    assert!(error.contains("`N`") && error.contains("half<4>"), "{error}");
    accepts(&format!("{source} fn main(t: Tensor<i64, 8>) {{ let h: Tensor<i64, 4> = half<4>(t); }}"));
}

#[test]
fn constant_folding_unifies_literal_expressions() {
    accepts("fn f(a: Tensor<i64, 1 + 1>) -> Tensor<i64, 2> { a } fn main() {}");
    accepts("fn f(a: Tensor<i64, 3 * 4 - 2>) -> Tensor<i64, 10> { a } fn main() {}");
}

#[test]
fn a_bound_param_substitutes_into_an_expression() {
    accepts(
        "
fn double<const M: int>(a: &Tensor<i64, M>) -> Tensor<i64, M * 2> {
    Tensor { data: slice_of_zeros<i64>(M * 2), dyn_dims: slice_of_zeros<i64>(0) }
}
fn main(a: Tensor<i64, 384>) { let b: Tensor<i64, 768> = double(&a); }
",
    );
}

#[test]
fn dyn_dimensions_type_check_regardless_of_runtime_value() {
    accepts("fn f(a: &Tensor<i64, Dyn, 768>, b: &Tensor<i64, Dyn, 768>) {} fn main(x: Tensor<i64, Dyn, 768>, y: Tensor<i64, Dyn, 768>) { f(&x, &y) }");
}

#[test]
fn dyn_never_unifies_with_a_constant() {
    let error = rejects("fn f(a: &Tensor<i64, Dyn>) {} fn main(x: Tensor<i64, 4>) { f(&x) }");
    assert!(error.contains("expected `Dyn`, found `4`"), "{error}");
    let error = rejects("fn f(a: &Tensor<i64, 4>) {} fn main(x: Tensor<i64, Dyn>) { f(&x) }");
    assert!(error.contains("PACO-E0336") && error.contains("expected `4`, found `x.dim0`"), "{error}");
}

#[test]
fn a_generic_bound_to_dyn_from_two_arguments_is_rejected() {
    let error = rejects(
        "
fn matmul<const M: int, const K: int, const N: int>(a: &Tensor<i64, M, K>, b: &Tensor<i64, K, N>) -> Tensor<i64, M, N> {
    Tensor { data: slice_of_zeros<i64>(0), dyn_dims: slice_of_zeros<i64>(0) }
}
fn main(a: Tensor<i64, 4, Dyn>, b: Tensor<i64, Dyn, 3>) { let c = matmul(&a, &b); }
",
    );
    assert!(error.contains("PACO-E0342"), "{error}");
    assert!(!error.contains("PACO-E0336"), "{error}");
    let error = rejects("fn same<const N: int>(a: &Tensor<i64, N>, b: &Tensor<i64, N>) {} fn main(x: Tensor<i64, Dyn>, y: Tensor<i64, Dyn>) { same(&x, &y) }");
    assert!(error.contains("PACO-E0342"), "{error}");
}

#[test]
fn an_expression_over_a_generic_bound_to_dyn_is_not_proved() {
    let error = rejects("fn f<const N: int>(a: &Tensor<i64, N>, b: &Tensor<i64, N * 2>) {} fn main(x: Tensor<i64, Dyn>, y: Tensor<i64, Dyn>) { f(&x, &y) }");
    assert!(error.contains("PACO-E0342"), "{error}");
}

#[test]
fn a_pack_bound_with_dyn_does_not_prove_a_second_operand() {
    let error = rejects(
        "
methods<T, const D: int...> Tensor<T, D...> {
    fn join(&self, other: &Self) -> i64 { 0 }
}
fn main(x: Tensor<i64, Dyn, 3>, y: Tensor<i64, Dyn, 3>) { let n = x.join(&y); }
",
    );
    assert!(error.contains("PACO-E0342"), "{error}");
    accepts(
        "
methods<T, const R: int...> Tensor<T, Dyn, R...> {
    fn stack(&self, other: &Self) -> i64 { 0 }
}
fn main(x: Tensor<i64, Dyn, 3>, y: Tensor<i64, Dyn, 3>) { let n = x.stack(&y); }
",
    );
}

#[test]
fn a_type_parameter_holding_a_dyn_type_needs_no_dimension_equality() {
    accepts("fn pick<T>(a: T, b: T) -> T { a } fn main(x: Tensor<i64, Dyn>, y: Tensor<i64, Dyn>) { let z = pick(x, y); }");
}

#[test]
fn a_generic_dim_carries_dyn_through() {
    accepts(
        "
fn id<const M: int>(a: Tensor<i64, M, 3>) -> Tensor<i64, M, 3> { a }
fn main(x: Tensor<i64, Dyn, 3>) { let y: Tensor<i64, Dyn, 3> = id(x); }
",
    );
}

#[test]
fn const_params_are_readable_as_values() {
    accepts(
        "
fn count<const N: int>(t: &Tensor<i64, N>) -> i64 { N }
struct Shape<const D: int...> { x: i64,
    fn rank(&self) -> i64 { D.len() }
}
fn main() {}
",
    );
}

#[test]
fn exceeding_the_instantiation_limit_reports_the_offending_constants() {
    let error = rejects(
        "
#[instantiation_limit(2)]
fn first<const N: int>(t: &Tensor<i64, N>) -> i64 { N }
fn main(a: Tensor<i64, 1>, b: Tensor<i64, 2>, c: Tensor<i64, 3>) {
    first(&a);
    first(&b);
    first(&a);
    first(&c)
}
",
    );
    assert!(error.contains("PACO-E0337"), "{error}");
    assert!(error.contains("instantiation limit of 2"), "{error}");
    assert!(error.contains("<N = 1>, <N = 2>, <N = 3>"), "{error}");
}

#[test]
fn repeated_shapes_do_not_count_against_the_limit() {
    accepts(
        "
#[instantiation_limit(2)]
fn first<const N: int>(t: &Tensor<i64, N>) -> i64 { N }
fn main(a: Tensor<i64, 1>, b: Tensor<i64, 2>) {
    first(&a);
    first(&b);
    first(&a);
    let last = first(&b);
}
",
    );
}

#[test]
fn the_limit_follows_instantiations_through_generic_callers() {
    let error = rejects(
        "
#[instantiation_limit(1)]
fn leaf<const N: int>(t: &Tensor<i64, N>) -> i64 { N }
fn middle<const M: int>(t: &Tensor<i64, M>) -> i64 { leaf(t) }
fn main(a: Tensor<i64, 4>, b: Tensor<i64, 5>) {
    middle(&a);
    middle(&b)
}
",
    );
    assert!(error.contains("`leaf` exceeds"), "{error}");
    assert!(error.contains("<N = 4>, <N = 5>"), "{error}");
}

#[test]
fn pack_shapes_are_listed_when_a_method_exceeds_its_limit() {
    let error = rejects(
        "
methods<T, const D: int...> Tensor<T, D...> {
    #[instantiation_limit(1)]
    fn rank(&self) -> i64 { D.len() }
}
fn main(a: Tensor<i64, 2, 3>, b: Tensor<i64, 4>) {
    a.rank();
    let r = b.rank();
}
",
    );
    assert!(error.contains("`Tensor::rank` exceeds"), "{error}");
    assert!(error.contains("<D = [2, 3]>, <D = [4]>"), "{error}");
}

#[test]
fn bounded_type_params_allow_their_operators_in_generic_bodies() {
    accepts("fn sum<T: Add>(a: T, b: T) -> T { a + b } fn main() { let x = sum(1, 2); }");
    accepts("fn neg<T: Neg + Ord>(a: T, b: T) -> bool { -a < b } fn main() {}");
    let error = rejects("fn sum<T>(a: T, b: T) -> T { a + b } fn main() {}");
    assert!(error.contains("PACO-E0301") || error.contains("PACO-E0334"), "{error}");
}

#[test]
fn a_call_whose_argument_violates_a_bound_is_rejected() {
    let error = rejects(
        "fn sum<T: Add>(a: T, b: T) -> T { a + b }\nfn main() { let a: f8e4m3 = 1.0;\n let x = sum(a, a); }",
    );
    assert!(error.contains("PACO-E0340"), "{error}");
    assert!(error.contains("`f8e4m3` does not satisfy `Add`"), "{error}");
}

#[test]
fn an_element_type_bound_is_checked_where_the_type_is_written() {
    let error = rejects("struct Grid<T: Numeric, const D: int...> { data: []T }\nfn main(g: Grid<bool, 2>) {}");
    assert!(error.contains("`bool` does not satisfy `Numeric`"), "{error}");
    accepts("struct Grid<T: Numeric, const D: int...> { data: []T }\nfn main(g: Grid<bf16, 2>, h: Grid<f8e5m2, 4>) {}");
}

#[test]
fn a_generic_numeric_param_can_be_cast() {
    accepts("fn widen<S: Numeric, T: Numeric>(x: S, y: T) -> T { x as T } fn main() {}");
    let error = rejects("fn widen<S, T: Numeric>(x: S, y: T) -> T { x as T } fn main() {}");
    assert!(error.contains("PACO-E0330"), "{error}");
}

#[test]
fn a_pack_prefix_with_a_spread_matches_the_leading_dimensions() {
    accepts(
        "
fn batch<const R: int...>(t: &Tensor<i64, Dyn, R...>) -> Tensor<i64, R...> {
    Tensor { data: slice_of_zeros<i64>(0), dyn_dims: slice_of_zeros<i64>(0) }
}
fn main(x: Tensor<i64, Dyn, 3, 4>) { let y: Tensor<i64, 3, 4> = batch(&x); }
",
    );
    let error = rejects(
        "
fn batch<const R: int...>(t: &Tensor<i64, Dyn, R...>) -> i64 { 0 }
fn main(x: Tensor<i64, 5, 3, 4>) { let y = batch(&x); }
",
    );
    assert!(error.contains("expected `Dyn`, found `5`") || error.contains("PACO-E0302"), "{error}");
}

const TENSOR_OPS: &str = "
methods<T: Numeric + Add, const D: int...> Tensor<T, D...> {
    fn add(&self, other: &Self) -> Self {
        Tensor { data: slice_of_zeros<T>(0), dyn_dims: slice_of_zeros<i64>(0) }
    }
}
";

#[test]
fn elementwise_arithmetic_on_an_fp8_tensor_is_rejected() {
    let error = rejects(&format!("{TENSOR_OPS}\nfn main(a: Tensor<f8e4m3, 128>, b: Tensor<f8e4m3, 128>) {{ let c = a + &b; }}"));
    assert!(error.contains("PACO-E0340"), "{error}");
    assert!(error.contains("`f8e4m3` does not satisfy `Add`"), "{error}");
    accepts(&format!("{TENSOR_OPS}\nfn main(a: Tensor<bf16, 128>, b: Tensor<bf16, 128>) {{ let c = a + &b; }}"));
}

#[test]
fn mixed_element_type_operations_require_an_explicit_conversion() {
    let error = rejects(&format!("{TENSOR_OPS}\nfn main(a: Tensor<bf16, 128>, b: Tensor<f32, 128>) {{ let c = a + &b; }}"));
    assert!(error.contains("expects &Tensor<bf16, 128>, found &Tensor<f32, 128>"), "{error}");
}

