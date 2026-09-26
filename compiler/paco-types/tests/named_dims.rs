use paco_diag::Reporter;
use paco_span::SourceMap;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::check_module;

const GRID: &str = "
enum Result<T, E> { Ok(T), Err(E) }
struct DimError { axis: i64, expected: string, origin: string, bound: i64, observed: i64 }
struct Grid<T, const D: int...> {
    data: []T,
    dims: []i64,
    fn extent(&self, axis: i64) -> i64 { self.dims[axis] }
    fn add(&self, other: &Grid<T, D...>) -> Grid<T, D...> { Grid { data: slice_of_zeros<T>(0), dims: slice_of_zeros<i64>(0) } }
    fn checked_add<const E: int...>(&self, other: &Grid<T, E...>) -> Result<Grid<T, D...>, DimError> {
        Result::Ok(Grid { data: slice_of_zeros<T>(0), dims: slice_of_zeros<i64>(0) })
    }
    fn zeros() -> Self { Grid { data: slice_of_zeros<T>(0), dims: slice_of_zeros<i64>(0) } }
    type Tangent = Grid<T, D...>;
    fn zero_tangent(&self) -> Grid<T, D...> { Grid { data: slice_of_zeros<T>(0), dims: slice_of_zeros<i64>(0) } }
    fn move_by(&mut self, offset: &Grid<T, D...>) {}
}
struct Plain<const D: int...> { x: i64 }
";

fn check_source(source: &str) -> Option<String> {
    let source = format!("{GRID}{source}");
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
fn an_immutable_dyn_binding_equals_itself() {
    accepts("fn main(x: Grid<f32, Dyn, 784>) { let y = x + &x; let z = y.add(&y); }");
    accepts("fn main() { let x = Grid<f32, Dyn>::zeros(); let y = x + &x; }");
}

#[test]
fn two_dyn_bindings_cannot_be_proved_equal() {
    let error = rejects("fn main(a: Grid<f32, Dyn, 784>, b: Grid<f32, Dyn, 784>) { let c = a + &b; }");
    assert!(error.contains("PACO-E0342"), "{error}");
    assert!(error.contains("`a.dim0`") && error.contains("`b.dim0`"), "{error}");
    assert!(error.contains("note at main.paco"), "{error}");
}

#[test]
fn a_mutable_dyn_binding_is_a_fresh_extent_at_each_use() {
    let error = rejects("fn main() { let mut m = Grid<f32, Dyn>::zeros(); let c = m + &m; }");
    assert!(error.contains("PACO-E0342"), "{error}");
}

#[test]
fn a_witness_makes_two_types_equal() {
    accepts(
        "
fn step(a: Grid<f32, Dyn>, b: Grid<f32, Dyn>) -> Result<i64, DimError> {
    let n = a.dim(0);
    let a: Grid<f32, n> = a.with_dims()?;
    let b: Grid<f32, n> = b.with_dims()?;
    let c = a + &b;
    let d: Grid<f32, n> = c;
    Result::Ok(n)
}
fn main() {}
",
    );
}

#[test]
fn a_witness_on_a_static_axis_is_the_constant() {
    accepts("fn main(x: Grid<f32, Dyn, 784>) { let f = x.dim(1); let g: Grid<f32, 3, f> = Grid<f32, 3, 784>::zeros(); }");
}

#[test]
fn witnesses_are_scoped_to_their_block() {
    let error = rejects("fn main(x: Grid<f32, Dyn>) { { let n = x.dim(0); } let g: Grid<f32, n> = x; }");
    assert!(error.contains("PACO-E0338"), "{error}");
}

#[test]
fn dim_rejects_mutable_bindings_computed_axes_out_of_range_axes_and_unshaped_types() {
    let error = rejects("fn main(x: Grid<f32, Dyn>) { let mut n = x.dim(0); let g: Grid<f32, n> = Grid<f32, 3>::zeros(); }");
    assert!(error.contains("PACO-E0345") && error.contains("let mut"), "{error}");
    let error = rejects("fn main(x: Grid<f32, Dyn>, i: i64) { let n = x.dim(i); }");
    assert!(error.contains("PACO-E0345") && error.contains("integer literal"), "{error}");
    let error = rejects("fn main(x: Grid<f32, Dyn>) { let n = x.dim(1); }");
    assert!(error.contains("PACO-E0345") && error.contains("out of range"), "{error}");
    let error = rejects("fn main(x: Plain<Dyn>) { let n = x.dim(0); }");
    assert!(error.contains("PACO-E0345") && error.contains("Shaped"), "{error}");
}

#[test]
fn with_dims_needs_a_target_and_keeps_static_and_dynamic_positions_apart() {
    let error = rejects("fn main(x: Grid<f32, Dyn>) -> Result<i64, DimError> { let y = x.with_dims()?; Result::Ok(0) }");
    assert!(error.contains("PACO-E0348") && error.contains("target"), "{error}");
    let error = rejects("fn main(x: Grid<f32, Dyn>) -> Result<i64, DimError> { let y: Grid<f32, 4> = x.with_dims()?; Result::Ok(0) }");
    assert!(error.contains("PACO-E0348"), "{error}");
    let error = rejects(
        "fn main(x: Grid<f32, 4>) -> Result<i64, DimError> { let n = x.dim(0); let y: Grid<f32, Dyn> = x.with_dims()?; Result::Ok(0) }",
    );
    assert!(error.contains("PACO-E0348"), "{error}");
    let error = rejects("fn main(x: Grid<f32, 4>) -> Result<i64, DimError> { let y: Grid<f32, 5> = x.with_dims()?; Result::Ok(0) }");
    assert!(error.contains("PACO-E0336"), "{error}");
}

#[test]
fn as_dims_borrows_and_erase_dims_forgets() {
    accepts(
        "
fn view(w: &Grid<f32, Dyn>, n: i64) -> Result<i64, DimError> {
    let k = w.dim(0);
    let v: &Grid<f32, k> = w.as_dims()?;
    let e: Grid<f32, Dyn> = Grid<f32, k>::zeros().erase_dims();
    Result::Ok(k)
}
fn main() {}
",
    );
}

#[test]
fn assume_dims_is_unsafe() {
    let error = rejects("fn main(x: Grid<f32, Dyn>, y: Grid<f32, Dyn>) { let n = x.dim(0); let z: Grid<f32, n> = y.assume_dims(); }");
    assert!(error.contains("PACO-E0355") || error.contains("unsafe"), "{error}");
    accepts("fn main(x: Grid<f32, Dyn>, y: Grid<f32, Dyn>) { let n = x.dim(0); let z: Grid<f32, n> = unsafe { y.assume_dims() }; }");
}

#[test]
fn dim_parameters_accept_static_symbolic_and_dyn_arguments() {
    accepts(
        "
fn rows<dim B>(g: &Grid<f32, B, 4>) -> i64 { B }
fn main(x: Grid<f32, Dyn, 4>, y: Grid<f32, 7, 4>) {
    let n = x.dim(0);
    let a = rows(&x) + rows(&y) + rows(&Grid<f32, Dyn, 4>::zeros());
}
",
    );
}

#[test]
fn a_dim_parameter_is_rigid_in_its_body() {
    let error = rejects("fn f<dim B, dim C>(a: &Grid<f32, B>, b: &Grid<f32, C>) { let c = a.add(b); } fn main() {}");
    assert!(error.contains("PACO-E0342"), "{error}");
    let error = rejects("fn f<dim B>(a: &Grid<f32, B>) { let c: Grid<f32, 4> = Grid<f32, B>::zeros(); } fn main() {}");
    assert!(error.contains("PACO-E0336"), "{error}");
}

#[test]
fn a_const_parameter_rejects_a_named_dimension_with_the_dim_fix() {
    let error = rejects("fn f<const N: int>(g: &Grid<f32, N, 4>) {} fn main(x: Grid<f32, Dyn, 4>) { let n = x.dim(0); let y: Grid<f32, n, 4> = Grid<f32, 3, 4>::zeros(); f(&x); }");
    assert!(error.contains("PACO-E0347"), "{error}");
    assert!(error.contains("fix 1: declare it `dim N`"), "{error}");
    accepts("fn f<const N: int>(g: &Grid<f32, N, 4>) {} fn main(x: Grid<f32, Dyn, 4>) { f(&x); }");
}

#[test]
fn inference_needs_a_lone_or_offset_parameter() {
    accepts("fn shrink<const N: int>(g: Grid<f32, N + 1>) -> Grid<f32, N> { Grid<f32, N>::zeros() } fn main() { let g: Grid<f32, 7> = shrink(Grid<f32, 8>::zeros()); }");
}

#[test]
fn existential_results_are_opened_with_a_fresh_name() {
    accepts(
        "
fn nonzero(v: Grid<f32, Dyn>) -> Grid<f32, ?n> { v }
fn main(v: Grid<f32, Dyn>, w: Grid<f32, Dyn>) {
    let kept = nonzero(v);
    let twice = kept + &kept;
}
",
    );
    let error = rejects(
        "
fn nonzero(v: Grid<f32, Dyn>) -> Grid<f32, ?n> { v }
fn main(v: Grid<f32, Dyn>, w: Grid<f32, Dyn>) {
    let a = nonzero(v);
    let b = nonzero(w);
    let c = a + &b;
}
",
    );
    assert!(error.contains("PACO-E0342") && error.contains("`a.n`") && error.contains("`b.n`"), "{error}");
}

#[test]
fn struct_fields_share_one_existential() {
    accepts(
        "
struct Batch { x: Grid<f32, ?b, 784>, y: Grid<i64, ?b> }
fn per_row<dim B>(x: &Grid<f32, B, 784>, y: &Grid<i64, B>) -> i64 { B }
fn load() -> Batch { Batch { x: Grid<f32, 2, 784>::zeros().erase_dims(), y: Grid<i64, 2>::zeros().erase_dims() } }
fn main() {
    let Batch { x, y } = load();
    let r = per_row(&x, &y);
    let batch = load();
    let s = per_row(&batch.x, &batch.y);
}
",
    );
    let error = rejects(
        "
struct Batch { x: Grid<f32, ?b, 784>, y: Grid<i64, ?b> }
fn build(a: Grid<f32, Dyn, 784>, b: Grid<i64, Dyn>) -> Batch {
    let n = a.dim(0);
    let m = b.dim(0);
    Batch { x: a, y: b }
}
fn main() {}
",
    );
    assert!(error.contains("PACO-E0342"), "{error}");
}

#[test]
fn a_named_dimension_cannot_become_dyn_silently() {
    let error = rejects(
        "
fn f(x: Grid<f32, Dyn>) -> Result<Grid<f32, Dyn>, DimError> {
    let n = x.dim(0);
    let y: Grid<f32, n> = x.with_dims()?;
    Result::Ok(y)
}
fn g(x: Grid<f32, Dyn>) -> Grid<f32, Dyn> {
    let n = x.dim(0);
    let y: Grid<f32, n> = unsafe { x.assume_dims() };
    y
}
fn main() {}
",
    );
    assert!(error.contains("PACO-E0343"), "{error}");
    assert!(error.contains("fix 1: name the extent in the signature: `?n`"), "{error}");
    assert!(error.contains("fix 2: forget the name explicitly with `.erase_dims()`"), "{error}");
    accepts(
        "
fn g(x: Grid<f32, Dyn>) -> Grid<f32, Dyn> {
    let n = x.dim(0);
    let y: Grid<f32, n> = unsafe { x.assume_dims() };
    y.erase_dims()
}
fn main() {}
",
    );
}

#[test]
fn a_name_does_not_survive_its_iteration() {
    let error = rejects(
        "
fn nonzero(v: &Grid<f32, Dyn>) -> Grid<f32, ?n> { Grid<f32, Dyn>::zeros() }
fn main(v: Grid<f32, Dyn>) {
    let mut keep = nonzero(&v);
    let mut i = 0;
    while i < 3 {
        let fresh = nonzero(&v);
        keep = fresh;
        i = i + 1;
    }
}
",
    );
    assert!(error.contains("PACO-E0343"), "{error}");
}

#[test]
fn broadcasting_needs_equal_extents_or_a_literal_one() {
    let source = "
struct Wide<T, const D: int...> {
    data: []T,
    fn extent(&self, axis: i64) -> i64 { 0 }
    #[broadcasts(D, R)]
    fn broadcast_to<const R: int...>(&self) -> Wide<T, R...> { Wide<T, R...> { data: slice_of_zeros<T>(0) } }
    fn checked_broadcast_to<const R: int...>(&self) -> Result<Wide<T, R...>, DimError> { Result::Ok(Wide<T, R...> { data: slice_of_zeros<T>(0) }) }
}
";
    accepts(&format!(
        "{source}
fn main(x: Wide<f32, 1, 784>, y: Wide<f32, Dyn, 784>) {{
    let n = y.dim(0);
    let b: Wide<f32, n, 784> = x.broadcast_to();
    let c: Wide<f32, 3, n, 784> = y.broadcast_to();
}}
"
    ));
    let error = rejects(&format!(
        "{source}
fn main(x: Wide<f32, Dyn, 784>, y: Wide<f32, Dyn, 784>) {{
    let n = y.dim(0);
    let b: Wide<f32, n, 784> = x.broadcast_to();
}}
"
    ));
    assert!(error.contains("PACO-E0346") && error.contains("checked_broadcast_to"), "{error}");
}

#[test]
fn a_gradient_keeps_the_primal_dimension_names() {
    accepts(
        "
#[builtin(grad)]
fn grad<F, I>(f: F, inputs: I) -> I { panic(\"no autodiff\") }
#[differentiable]
fn loss<dim B>(x: &Grid<f32, B, 784>) -> f32 { 0.0 as f32 }
fn main(x: Grid<f32, Dyn, 784>) {
    let n = x.dim(0);
    let (value, gradient) = grad(loss, &x);
    let g: Grid<f32, n, 784> = gradient;
}
",
    );
}

#[test]
fn growing_consumes_the_value_and_opens_a_fresh_name() {
    let source = "
methods<T: Numeric + Add, const R: int...> Grid<T, Dyn, R...> {
    fn push_row(self, row: &Grid<T, R...>) -> Grid<T, ?m, R...> { Grid<T, Dyn, R...>::zeros() }
}
";
    accepts(&format!("{source}\nfn main(x: Grid<f32, Dyn, 4>, r: Grid<f32, 4>) {{ let y = x.push_row(&r); let z = y + &y; }}"));
    let error = rejects(&format!(
        "{source}\nfn main(x: Grid<f32, Dyn, 4>, r: Grid<f32, 4>) {{ let n = x.dim(0); let y = x.push_row(&r); let z: Grid<f32, n, 4> = y; }}"
    ));
    assert!(error.contains("PACO-E0342") && error.contains("`y.m`"), "{error}");
}

#[test]
fn a_named_value_may_be_passed_where_dyn_is_declared_but_a_static_one_may_not() {
    accepts(
        "
fn count(g: &Grid<f32, Dyn, 784>) -> i64 { 0 }
fn main(x: Grid<f32, Dyn, 784>) -> Result<i64, DimError> {
    let n = x.dim(0);
    let y: Grid<f32, n, 784> = x.with_dims()?;
    Result::Ok(count(&y))
}
",
    );
    let error = rejects("fn count(g: &Grid<f32, Dyn, 784>) -> i64 { 0 } fn main(x: Grid<f32, 4, 784>) { let c = count(&x); }");
    assert!(error.contains("PACO-E0336"), "{error}");
}

#[test]
fn dyn_that_needs_no_equality_keeps_the_first_arguments_name() {
    accepts(
        "
fn matmul<dim M, dim K, dim N>(a: &Grid<f32, M, K>, b: &Grid<f32, K, N>) -> Grid<f32, M, N> { Grid<f32, M, N>::zeros() }
fn main(x: Grid<f32, Dyn, 784>, w: Grid<f32, 784, 10>) {
    let n = x.dim(0);
    let r: Grid<f32, n, 10> = matmul(&x, &w);
}
",
    );
}
