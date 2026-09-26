use paco_diag::Reporter;
use paco_span::SourceMap;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::check_module;

const PRELUDE: &str = "
#[builtin(grad)]
fn grad<F, I>(f: F, inputs: I) -> I { panic(\"no autodiff\") }
struct Vec2 {
    x: f64,
    y: f64,
    type Tangent = Vec2;
    fn zero_tangent(&self) -> Vec2 { Vec2 { x: 0.0, y: 0.0 } }
    fn move_by(&mut self, offset: &Vec2) { self.x = self.x + offset.x; self.y = self.y + offset.y; }
    fn add(&self, other: Vec2) -> Vec2 { Vec2 { x: self.x + other.x, y: self.y + other.y } }
}
struct Field<T, const D: int...> {
    data: []T,
    type Tangent = Field<T, D...>;
    fn zero_tangent(&self) -> Field<T, D...> { Field { data: slice_of_zeros<T>(self.data.len()) } }
    fn move_by(&mut self, offset: &Field<T, D...>) {}
}
";

fn check_source(source: &str) -> Option<String> {
    let source = format!("{PRELUDE}{source}");
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

/// `main.paco:<line>:<column>` of the first occurrence of `needle` in `source`.
fn location(source: &str, needle: &str) -> String {
    let full = format!("{PRELUDE}{source}");
    let offset = full.find(needle).unwrap_or_else(|| panic!("`{needle}` not in the source"));
    let line = full[..offset].matches('\n').count() + 1;
    let column = offset - full[..offset].rfind('\n').map_or(0, |newline| newline + 1) + 1;
    format!("main.paco:{line}:{column}")
}

#[test]
fn float_math_methods_type_on_every_float_with_arithmetic() {
    for ty in ["float", "f32", "f16", "bf16"] {
        accepts(&format!(
            "fn main(x: {ty}, y: {ty}) {{
                let a: {ty} = x.sqrt() + x.exp() + x.ln() + x.sin() + x.cos() + x.tanh() + x.abs();
                let b: {ty} = x.powf(y) + x.min(y) + x.max(y);
            }}"
        ));
    }
}

#[test]
fn float_math_methods_are_rejected_on_fp8_and_check_their_arguments() {
    let error = rejects("fn main(x: f8e4m3) { let y = x.sqrt(); }");
    assert!(error.contains("PACO-E0339"), "{error}");
    let error = rejects("fn main(x: f32, y: float) { let z = x.powf(y); }");
    assert!(error.contains("type mismatch"), "{error}");
    let error = rejects("fn main(x: float) { let z = x.sqrt(1.0); }");
    assert!(error.contains("PACO-E0305"), "{error}");
}

#[test]
fn a_type_with_the_differentiable_items_is_accepted_and_its_gradient_is_its_tangent() {
    accepts(
        "
#[differentiable]
fn energy(v: Vec2) -> f64 { 0.5 * (v.x * v.x + v.y * v.y) }
fn main() {
    let (value, gradient) = grad(energy, Vec2 { x: 3.0, y: 4.0 });
    let e: f64 = value;
    let g: Vec2 = gradient;
}
",
    );
}

#[test]
fn a_type_without_the_differentiable_items_is_rejected_naming_them() {
    let source = "
struct Cache<T> { v: T }
#[differentiable]
fn loss(c: Cache<f32>) -> f32 { c.v }
fn main() {}
";
    let error = rejects(source);
    assert!(error.contains("PACO-E0341"), "{error}");
    assert!(error.contains(&location(source, "c: Cache<f32>")), "{error}");
    assert!(error.contains("no `Tangent`, `zero_tangent`, `move_by`"), "{error}");
}

#[test]
fn fp8_and_integer_parameters_are_rejected() {
    let error = rejects("#[differentiable]\nfn bad(w: f8e4m3) -> f32 { 1.0 as f32 }\nfn main() {}");
    assert!(error.contains("PACO-E0341") && error.contains("parameter `w` has type `f8e4m3`"), "{error}");
    let error = rejects("#[differentiable]\nfn bad(w: i32) -> f32 { 1.0 as f32 }\nfn main() {}");
    assert!(error.contains("PACO-E0341") && error.contains("parameter `w` has type `i32`"), "{error}");
    let error = rejects("#[differentiable]\nfn bad(w: f32) -> i64 { 1 }\nfn main() {}");
    assert!(error.contains("returns `i64`"), "{error}");
}

#[test]
fn a_gradient_has_its_inputs_tangent_type_with_the_same_dimensions() {
    accepts(
        "
#[differentiable]
fn loss(w: &Field<f32, 128, 64>) -> f32 { 0.0 as f32 }
fn main(w: Field<f32, 128, 64>) {
    let (value, gradient) = grad(loss, &w);
    let v: f32 = value;
    let g: Field<f32, 128, 64> = gradient;
}
",
    );
    let error = rejects(
        "
#[differentiable]
fn loss(w: &Field<f32, 128, 64>) -> f32 { 0.0 as f32 }
fn main(w: Field<f32, 128, 64>) {
    let (value, gradient) = grad(loss, &w);
    let g: Field<f32, 64, 128> = gradient;
}
",
    );
    assert!(error.contains("PACO-E0336"), "{error}");
}

#[test]
fn a_two_input_gradient_takes_and_returns_a_tuple() {
    accepts(
        "
#[differentiable]
fn loss(v: &Vec2, b: f32) -> f32 { b }
fn main(v: Vec2, b: f32) {
    let (value, (dv, db)) = grad(loss, (&v, b));
    let gv: Vec2 = dv;
    let gb: f32 = db;
}
",
    );
    let error = rejects(
        "
#[differentiable]
fn loss(v: &Vec2, b: f32) -> f32 { b }
fn main(v: Vec2, b: f32) { let (value, (dv, db)) = grad(loss, (b, &v)); }
",
    );
    assert!(error.contains("type mismatch"), "{error}");
}

#[test]
fn grad_requires_a_differentiable_function() {
    let error = rejects("fn plain(x: f32) -> f32 { x }\nfn main(x: f32) { let r = grad(plain, x); }");
    assert!(error.contains("`plain` is not `#[differentiable]`"), "{error}");
}

#[test]
fn a_unit_function_with_one_in_out_parameter_returns_its_final_value() {
    accepts(
        "
#[differentiable]
fn step(x: &mut float) { *x = *x * *x }
fn main() {
    let mut x = 3.0;
    let (value, gradient) = grad(step, &mut x);
    let v: float = value;
    let g: float = gradient;
}
",
    );
}

#[test]
fn a_unit_function_with_two_in_out_parameters_is_ambiguous() {
    let source = "
#[differentiable]
fn step(x: &mut float, y: &mut float) { *x = *y }
fn main() {}
";
    let error = rejects(source);
    assert!(error.contains("PACO-E0813"), "{error}");
    assert!(error.contains(&location(source, "fn step")), "{error}");
    assert!(error.contains("`x` and `y`"), "{error}");
}

#[test]
fn grad_of_a_struct_valued_function_needs_a_scalar() {
    let source = "
#[differentiable]
fn scaled(v: Vec2) -> Vec2 { v }
fn main() { let r = grad(scaled, Vec2 { x: 1.0, y: 2.0 }); }
";
    let error = rejects(source);
    assert!(error.contains("PACO-E0813"), "{error}");
    assert!(error.contains("scalar"), "{error}");
}

const NORM: &str = "
fn norm(v: &Vec2) -> f64 { (v.x * v.x + v.y * v.y).sqrt() }
struct NormPullback {
    v: Vec2,
    n: f64,
    type Seed = f64;
    type Gradients = Vec2;
    fn pullback(self, seed: f64) -> Vec2 { Vec2 { x: seed * self.v.x / self.n, y: seed * self.v.y / self.n } }
}
";

#[test]
fn a_pullback_struct_type_checks() {
    accepts(&format!(
        "{NORM}
fn main() {{
    let p = NormPullback {{ v: Vec2 {{ x: 3.0, y: 4.0 }}, n: 5.0 }};
    let g: Vec2 = p.pullback(1.0);
}}
"
    ));
}

#[test]
fn a_well_formed_derivative_is_accepted() {
    accepts(&format!(
        "{NORM}
#[derivative(of = norm)]
fn norm_derivative(v: &Vec2) -> (f64, NormPullback) {{
    let n = norm(v);
    (n, NormPullback {{ v: Vec2 {{ x: v.x, y: v.y }}, n: n }})
}}
fn main() {{}}
"
    ));
}

fn derivative_error(derivative: &str) -> (String, String) {
    let source = format!("{NORM}{derivative}\nfn main() {{}}\n");
    let error = rejects(&source);
    assert!(error.contains("PACO-E0814"), "{error}");
    (error, location(&source, "#[derivative"))
}

#[test]
fn a_derivative_of_an_unknown_function_is_rejected() {
    let (error, at) = derivative_error("#[derivative(of = nope)]\nfn d(v: &Vec2) -> (f64, NormPullback) { (0.0, NormPullback { v: Vec2 { x: 0.0, y: 0.0 }, n: 1.0 }) }");
    assert!(error.contains(&at) && error.contains("no function or method named `nope`"), "{error}");
}

#[test]
fn a_derivative_with_different_parameters_names_both_signatures() {
    let (error, at) = derivative_error("#[derivative(of = norm)]\nfn d(v: Vec2) -> (f64, NormPullback) { (0.0, NormPullback { v: v, n: 1.0 }) }");
    assert!(error.contains(&at), "{error}");
    assert!(error.contains("`(Vec2) -> (float, NormPullback)`") && error.contains("`(&Vec2) -> float`"), "{error}");
}

#[test]
fn a_derivative_must_return_the_result_and_a_pullback() {
    let (error, at) = derivative_error("#[derivative(of = norm)]\nfn d(v: &Vec2) -> f64 { 0.0 }");
    assert!(error.contains(&at) && error.contains("must return `(float, P)`"), "{error}");
    let (error, _) = derivative_error("#[derivative(of = norm)]\nfn d(v: &Vec2) -> (f64, Vec2) { (0.0, Vec2 { x: 0.0, y: 0.0 }) }");
    assert!(error.contains("does not satisfy `stdlib::autodiff::Pullback`"), "{error}");
}

#[test]
fn a_derivative_whose_pullback_types_do_not_match_is_rejected() {
    let (error, at) = derivative_error(
        "struct Wrong { type Seed = f64; type Gradients = f64; fn pullback(self, seed: f64) -> f64 { seed } }
#[derivative(of = norm)]
fn d(v: &Vec2) -> (f64, Wrong) { (0.0, Wrong {}) }",
    );
    assert!(error.contains(&at) && error.contains("`Gradients = Vec2`"), "{error}");
}

#[test]
fn a_second_derivative_for_the_same_function_is_rejected() {
    let one = "#[derivative(of = norm)]\nfn d1(v: &Vec2) -> (f64, NormPullback) { (0.0, NormPullback { v: Vec2 { x: 0.0, y: 0.0 }, n: 1.0 }) }";
    let two = "#[derivative(of = norm)]\nfn d2(v: &Vec2) -> (f64, NormPullback) { (0.0, NormPullback { v: Vec2 { x: 0.0, y: 0.0 }, n: 1.0 }) }";
    let source = format!("{NORM}{one}\n{two}\nfn main() {{}}\n");
    let error = rejects(&source);
    assert!(error.contains("PACO-E0814") && error.contains("already has a `#[derivative]`"), "{error}");
    let second = format!("{PRELUDE}{source}").rfind("#[derivative").unwrap();
    let full = format!("{PRELUDE}{source}");
    let line = full[..second].matches('\n').count() + 1;
    assert!(error.contains(&format!("main.paco:{line}:1")), "{error}");
}
