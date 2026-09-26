//! Dimensions: integer expressions over literals and const parameters, or
//! `Dyn`. Expressions are kept as canonical polynomials, hash-consed so that
//! equal polynomials share one `PolyId`.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use crate::Type;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub struct DimVarId(u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub struct PolyId(u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum DimOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

impl DimOp {
    fn symbol(self) -> &'static str {
        match self {
            DimOp::Add => "+",
            DimOp::Sub => "-",
            DimOp::Mul => "*",
            DimOp::Div => "/",
            DimOp::Rem => "%",
        }
    }

    fn precedence(self) -> u8 {
        match self {
            DimOp::Add | DimOp::Sub => 1,
            DimOp::Mul | DimOp::Div | DimOp::Rem => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Verdict {
    Equal,
    Different,
    CannotProve,
}

#[derive(Clone, Copy, Debug)]
enum Atom {
    Param(&'static str),
    Opaque(DimOp, PolyId, PolyId),
}

type Monomial = Vec<(DimVarId, u32)>;
type Poly = BTreeMap<Monomial, i64>;

#[derive(Default)]
struct Arena {
    atoms: Vec<Atom>,
    params: HashMap<&'static str, DimVarId>,
    opaque: HashMap<(DimOp, PolyId, PolyId), DimVarId>,
    polys: Vec<(Poly, Option<i64>)>,
    poly_ids: HashMap<Poly, PolyId>,
    lits: HashMap<i64, PolyId>,
}

static ARENA: LazyLock<Mutex<Arena>> = LazyLock::new(Default::default);

static NEXT_ATOM: AtomicU32 = AtomicU32::new(0);
static DISPLAY: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);

/// A rigid dimension name that is equal only to itself: a witness, an opened
/// `Dyn` or an opened existential. The `@` suffix keeps it unique.
pub fn fresh_atom(display: &str) -> String {
    format!("{display}@{}", NEXT_ATOM.fetch_add(1, Ordering::Relaxed))
}

pub fn is_atom(name: &str) -> bool {
    name.contains('@')
}

/// `?b` as written in a return or field type, before a consumer opens it.
pub fn is_existential(name: &str) -> bool {
    name.starts_with('?')
}

pub fn display_name(name: &str) -> String {
    if !is_atom(name) {
        return name.to_string();
    }
    if let Some(display) = DISPLAY.lock().unwrap_or_else(PoisonError::into_inner).get(name) {
        return display.clone();
    }
    name.split('@').next().unwrap_or(name).to_string()
}

pub fn rename_atom(name: &str, display: &str) {
    DISPLAY.lock().unwrap_or_else(PoisonError::into_inner).insert(name.to_string(), display.to_string());
}

fn with<R>(f: impl FnOnce(&mut Arena) -> R) -> R {
    f(&mut ARENA.lock().unwrap_or_else(PoisonError::into_inner))
}

enum Binding<'a> {
    Keep,
    Dyn,
    To(PolyId),
    Rename(&'a str),
}

fn binding<'a>(name: &str, substitutions: &'a HashMap<String, Type>) -> Binding<'a> {
    match substitutions.get(name) {
        Some(Type::Dim(Dim::Const(expr))) => Binding::To(expr.poly),
        Some(Type::Dim(Dim::Dyn)) => Binding::Dyn,
        Some(Type::Generic(other)) if other != name => Binding::Rename(other),
        _ => Binding::Keep,
    }
}

fn leak(name: &str) -> &'static str {
    Box::leak(name.to_string().into_boxed_str())
}

fn is_unbound(name: &str, substitutions: &HashMap<String, Type>) -> bool {
    matches!(substitutions.get(name), Some(Type::Generic(other)) if other == name)
}

impl Arena {
    fn intern(&mut self, poly: Poly) -> PolyId {
        if let Some(&id) = self.poly_ids.get(&poly) {
            return id;
        }
        let lit = match poly.len() {
            0 => Some(0),
            1 => poly.get(&Vec::new()).copied(),
            _ => None,
        };
        let id = PolyId(self.polys.len() as u32);
        if let Some(value) = lit {
            self.lits.insert(value, id);
        }
        self.poly_ids.insert(poly.clone(), id);
        self.polys.push((poly, lit));
        id
    }

    fn lit_of(&self, id: PolyId) -> Option<i64> {
        self.polys[id.0 as usize].1
    }

    fn lit(&mut self, value: i64) -> PolyId {
        if let Some(&id) = self.lits.get(&value) {
            return id;
        }
        let mut poly = Poly::new();
        if value != 0 {
            poly.insert(Vec::new(), value);
        }
        self.intern(poly)
    }

    fn atom_poly(&mut self, atom: DimVarId) -> PolyId {
        self.intern(Poly::from([(vec![(atom, 1)], 1)]))
    }

    fn param_atom(&mut self, name: &str) -> DimVarId {
        if let Some(&id) = self.params.get(name) {
            return id;
        }
        let name = leak(name);
        let id = DimVarId(self.atoms.len() as u32);
        self.atoms.push(Atom::Param(name));
        self.params.insert(name, id);
        id
    }

    fn param(&mut self, name: &str) -> PolyId {
        let atom = self.param_atom(name);
        self.atom_poly(atom)
    }

    fn opaque(&mut self, op: DimOp, left: PolyId, right: PolyId) -> PolyId {
        let atom = match self.opaque.get(&(op, left, right)) {
            Some(&id) => id,
            None => {
                let id = DimVarId(self.atoms.len() as u32);
                self.atoms.push(Atom::Opaque(op, left, right));
                self.opaque.insert((op, left, right), id);
                id
            }
        };
        self.atom_poly(atom)
    }

    fn apply(&mut self, op: DimOp, left: PolyId, right: PolyId) -> PolyId {
        if let (Some(a), Some(b)) = (self.lit_of(left), self.lit_of(right)) {
            let folded = match op {
                DimOp::Add => a.checked_add(b),
                DimOp::Sub => a.checked_sub(b),
                DimOp::Mul => a.checked_mul(b),
                DimOp::Div => a.checked_div(b),
                DimOp::Rem => a.checked_rem(b),
            };
            return match folded {
                Some(value) => self.lit(value),
                None => self.opaque(op, left, right),
            };
        }
        let result = match op {
            DimOp::Add => self.combine(left, right, 1),
            DimOp::Sub => self.combine(left, right, -1),
            DimOp::Mul => self.product(left, right),
            DimOp::Div | DimOp::Rem => None,
        };
        result.unwrap_or_else(|| self.opaque(op, left, right))
    }

    fn combine(&mut self, left: PolyId, right: PolyId, sign: i64) -> Option<PolyId> {
        let mut out = self.polys[left.0 as usize].0.clone();
        for (mono, coeff) in &self.polys[right.0 as usize].0 {
            let entry = out.entry(mono.clone()).or_insert(0);
            *entry = entry.checked_add(coeff.checked_mul(sign)?)?;
            if *entry == 0 {
                out.remove(mono);
            }
        }
        Some(self.intern(out))
    }

    fn product(&mut self, left: PolyId, right: PolyId) -> Option<PolyId> {
        let mut out = Poly::new();
        for (left_mono, left_coeff) in &self.polys[left.0 as usize].0 {
            for (right_mono, right_coeff) in &self.polys[right.0 as usize].0 {
                let entry = out.entry(merge(left_mono, right_mono)?).or_insert(0);
                *entry = entry.checked_add(left_coeff.checked_mul(*right_coeff)?)?;
            }
        }
        out.retain(|_, coeff| *coeff != 0);
        Some(self.intern(out))
    }

    fn touches(&self, id: PolyId, substitutions: &HashMap<String, Type>) -> bool {
        self.polys[id.0 as usize].0.keys().flatten().any(|&(atom, _)| match self.atoms[atom.0 as usize] {
            Atom::Param(name) => !matches!(binding(name, substitutions), Binding::Keep),
            Atom::Opaque(_, left, right) => self.touches(left, substitutions) || self.touches(right, substitutions),
        })
    }

    fn eval(&self, id: PolyId, substitutions: &HashMap<String, Type>) -> Option<i64> {
        let mut total: i64 = 0;
        for (mono, coeff) in &self.polys[id.0 as usize].0 {
            let mut term = *coeff;
            for &(atom, exp) in mono {
                let value = match self.atoms[atom.0 as usize] {
                    Atom::Param(name) => match binding(name, substitutions) {
                        Binding::To(bound) => self.lit_of(bound)?,
                        _ => return None,
                    },
                    Atom::Opaque(op, left, right) => {
                        let (a, b) = (self.eval(left, substitutions)?, self.eval(right, substitutions)?);
                        match op {
                            DimOp::Add => a.checked_add(b)?,
                            DimOp::Sub => a.checked_sub(b)?,
                            DimOp::Mul => a.checked_mul(b)?,
                            DimOp::Div => a.checked_div(b)?,
                            DimOp::Rem => a.checked_rem(b)?,
                        }
                    }
                };
                term = term.checked_mul(value.checked_pow(exp)?)?;
            }
            total = total.checked_add(term)?;
        }
        Some(total)
    }

    /// `None` when a parameter is bound to `Dyn`.
    fn resolve(&mut self, id: PolyId, substitutions: &HashMap<String, Type>) -> Option<PolyId> {
        if !self.touches(id, substitutions) {
            return Some(id);
        }
        if let Some(value) = self.eval(id, substitutions) {
            return Some(self.lit(value));
        }
        let poly = self.polys[id.0 as usize].0.clone();
        let mut total = self.lit(0);
        for (mono, coeff) in poly {
            let mut term = self.lit(coeff);
            for (atom, exp) in mono {
                let value = match self.atoms[atom.0 as usize] {
                    Atom::Param(name) => match binding(name, substitutions) {
                        Binding::Keep => self.atom_poly(atom),
                        Binding::Dyn => return None,
                        Binding::To(bound) => bound,
                        Binding::Rename(other) => self.param(other),
                    },
                    Atom::Opaque(op, left, right) => {
                        let left = self.resolve(left, substitutions)?;
                        let right = self.resolve(right, substitutions)?;
                        self.apply(op, left, right)
                    }
                };
                for _ in 0..exp {
                    term = self.apply(DimOp::Mul, term, value);
                }
            }
            total = self.apply(DimOp::Add, total, term);
        }
        Some(total)
    }

    fn unbound_params(&self, id: PolyId, substitutions: &HashMap<String, Type>, out: &mut Vec<DimVarId>) {
        for &(atom, _) in self.polys[id.0 as usize].0.keys().flatten() {
            match self.atoms[atom.0 as usize] {
                Atom::Param(name) => {
                    if is_unbound(name, substitutions) && !out.contains(&atom) {
                        out.push(atom);
                    }
                }
                Atom::Opaque(_, left, right) => {
                    self.unbound_params(left, substitutions, out);
                    self.unbound_params(right, substitutions, out);
                }
            }
        }
    }

    /// `expected - actual` as `k * N + rest`, for the one unbound parameter
    /// `N` that it mentions, when `N` occurs nowhere else.
    fn linear_in_unbound(
        &mut self,
        expected: PolyId,
        actual: PolyId,
        substitutions: &HashMap<String, Type>,
    ) -> Option<(DimVarId, i64, PolyId)> {
        let difference = self.apply(DimOp::Sub, expected, actual);
        let mut unbound = Vec::new();
        self.unbound_params(difference, substitutions, &mut unbound);
        let [param] = unbound[..] else { return None };
        let mut rest = self.polys[difference.0 as usize].0.clone();
        let coeff = rest.remove(&vec![(param, 1)])?;
        let mut others = Vec::new();
        let rest = self.intern(rest);
        self.unbound_params(rest, substitutions, &mut others);
        others.is_empty().then_some((param, coeff, rest))
    }

    fn param_name(&self, atom: DimVarId) -> &'static str {
        match self.atoms[atom.0 as usize] {
            Atom::Param(name) => name,
            Atom::Opaque(..) => unreachable!("an opaque atom is never a parameter"),
        }
    }

    fn render(&self, id: PolyId) -> String {
        let poly = &self.polys[id.0 as usize].0;
        if poly.is_empty() {
            return "0".to_string();
        }
        let mut terms: Vec<(u32, String, i64)> = poly
            .iter()
            .map(|(mono, coeff)| {
                let degree = mono.iter().map(|&(_, exp)| exp).sum();
                (degree, self.render_monomial(mono, poly.len() == 1 && *coeff == 1), *coeff)
            })
            .collect();
        terms.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        let mut out = String::new();
        for (index, (degree, factors, coeff)) in terms.iter().enumerate() {
            let magnitude = coeff.unsigned_abs();
            let term = match (*degree, magnitude) {
                (0, _) => magnitude.to_string(),
                (_, 1) => factors.clone(),
                _ => format!("{magnitude} * {factors}"),
            };
            match (index, *coeff < 0) {
                (0, false) => out.push_str(&term),
                (0, true) => out.push_str(&format!("-{term}")),
                (_, false) => out.push_str(&format!(" + {term}")),
                (_, true) => out.push_str(&format!(" - {term}")),
            }
        }
        out
    }

    fn render_monomial(&self, mono: &Monomial, alone: bool) -> String {
        let mut factors: Vec<String> = Vec::new();
        for &(atom, exp) in mono {
            let text = match self.atoms[atom.0 as usize] {
                Atom::Param(name) => display_name(name),
                Atom::Opaque(op, left, right) => {
                    let text = format!("{} {} {}", self.render_operand(left), op.symbol(), self.render_operand(right));
                    if alone && mono.len() == 1 && exp == 1 { text } else { format!("({text})") }
                }
            };
            factors.extend(std::iter::repeat_n(text, exp as usize));
        }
        factors.sort();
        factors.join(" * ")
    }

    fn render_operand(&self, id: PolyId) -> String {
        let text = self.render(id);
        if text.contains(' ') || text.starts_with('-') { format!("({text})") } else { text }
    }
}

fn merge(left: &Monomial, right: &Monomial) -> Option<Monomial> {
    let mut out = Vec::with_capacity(left.len() + right.len());
    let (mut i, mut j) = (0, 0);
    while i < left.len() && j < right.len() {
        let (a, b) = (left[i], right[j]);
        match a.0.cmp(&b.0) {
            std::cmp::Ordering::Less => {
                out.push(a);
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                out.push(b);
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                out.push((a.0, a.1.checked_add(b.1)?));
                i += 1;
                j += 1;
            }
        }
    }
    out.extend_from_slice(&left[i..]);
    out.extend_from_slice(&right[j..]);
    Some(out)
}

/// A dimension expression: its canonical polynomial, plus the form the
/// user wrote when it differs. Equality and hashing use the polynomial only.
#[derive(Clone, Debug)]
pub struct ConstExpr {
    poly: PolyId,
    written: Option<(Arc<str>, u8)>,
}

impl PartialEq for ConstExpr {
    fn eq(&self, other: &Self) -> bool {
        self.poly == other.poly
    }
}

impl Eq for ConstExpr {}

impl std::hash::Hash for ConstExpr {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.poly.hash(state);
    }
}

impl ConstExpr {
    pub fn lit(value: i64) -> ConstExpr {
        ConstExpr { poly: with(|arena| arena.lit(value)), written: None }
    }

    pub fn param(name: &str) -> ConstExpr {
        ConstExpr { poly: with(|arena| arena.param(name)), written: None }
    }

    pub fn binary(op: DimOp, left: &ConstExpr, right: &ConstExpr) -> ConstExpr {
        let parent = op.precedence();
        let (left_min, right_min) = match op {
            DimOp::Add | DimOp::Mul => (parent, parent),
            DimOp::Sub => (parent, parent + 1),
            DimOp::Div | DimOp::Rem => (parent + 1, parent + 1),
        };
        let text = format!("{} {} {}", left.operand(left_min), op.symbol(), right.operand(right_min));
        let poly = with(|arena| arena.apply(op, left.poly, right.poly));
        ConstExpr { poly, written: Some((text.into(), parent)) }
    }

    pub fn neg(inner: &ConstExpr) -> ConstExpr {
        let text = format!("-{}", inner.operand(3));
        let poly = with(|arena| {
            let zero = arena.lit(0);
            arena.apply(DimOp::Sub, zero, inner.poly)
        });
        ConstExpr { poly, written: Some((text.into(), 3)) }
    }

    fn operand(&self, min: u8) -> String {
        let (text, precedence) = match &self.written {
            Some((text, precedence)) => (text.to_string(), *precedence),
            None => {
                let text = self.normal_form();
                let precedence = if text.contains(" + ") || text.contains(" - ") {
                    1
                } else if text.contains(' ') || text.starts_with('-') {
                    2
                } else {
                    3
                };
                (text, precedence)
            }
        };
        if precedence < min { format!("({text})") } else { text }
    }

    pub fn poly(&self) -> PolyId {
        self.poly
    }

    pub fn as_lit(&self) -> Option<i64> {
        with(|arena| arena.lit_of(self.poly))
    }

    pub fn normal_form(&self) -> String {
        with(|arena| arena.render(self.poly))
    }

    /// The parameter this expression is, when it is exactly one parameter.
    pub fn as_param(&self) -> Option<&'static str> {
        with(|arena| {
            let poly = &arena.polys[self.poly.0 as usize].0;
            let (mono, coeff) = poly.iter().next().filter(|_| poly.len() == 1)?;
            match (mono.as_slice(), coeff) {
                ([(atom, 1)], 1) => match arena.atoms[atom.0 as usize] {
                    Atom::Param(name) => Some(name),
                    Atom::Opaque(..) => None,
                },
                _ => None,
            }
        })
    }

    /// Replaces bound parameters. `None` means a parameter was bound to
    /// `Dyn`, which makes the whole expression `Dyn`.
    pub fn substitute(&self, substitutions: &HashMap<String, Type>) -> Option<ConstExpr> {
        let poly = with(|arena| arena.resolve(self.poly, substitutions))?;
        if poly == self.poly {
            return Some(self.clone());
        }
        Some(ConstExpr { poly, written: None })
    }

    /// Whether the expression mentions no parameter, so that it denotes one
    /// value (or overflows).
    pub fn is_ground(&self) -> bool {
        fn ground(arena: &Arena, id: PolyId) -> bool {
            arena.polys[id.0 as usize].0.keys().flatten().all(|&(atom, _)| match arena.atoms[atom.0 as usize] {
                Atom::Param(_) => false,
                Atom::Opaque(_, left, right) => ground(arena, left) && ground(arena, right),
            })
        }
        with(|arena| ground(arena, self.poly))
    }

    /// Whether any name the expression mentions satisfies `test`.
    pub fn any_name(&self, test: impl Fn(&str) -> bool) -> bool {
        fn walk(arena: &Arena, id: PolyId, test: &dyn Fn(&str) -> bool) -> bool {
            arena.polys[id.0 as usize].0.keys().flatten().any(|&(atom, _)| match arena.atoms[atom.0 as usize] {
                Atom::Param(name) => test(name),
                Atom::Opaque(_, left, right) => walk(arena, left, test) || walk(arena, right, test),
            })
        }
        with(|arena| walk(arena, self.poly, &test))
    }

    /// Every name the expression mentions, including inside opaque terms.
    pub fn names(&self) -> Vec<&'static str> {
        fn walk(arena: &Arena, id: PolyId, out: &mut Vec<&'static str>) {
            for &(atom, _) in arena.polys[id.0 as usize].0.keys().flatten() {
                match arena.atoms[atom.0 as usize] {
                    Atom::Param(name) => {
                        if !out.contains(&name) {
                            out.push(name);
                        }
                    }
                    Atom::Opaque(_, left, right) => {
                        walk(arena, left, out);
                        walk(arena, right, out);
                    }
                }
            }
        }
        with(|arena| {
            let mut out = Vec::new();
            walk(arena, self.poly, &mut out);
            out
        })
    }

    /// The polynomial as `coefficient * product(factor^exponent)` terms, for
    /// evaluating it at run time.
    pub fn terms(&self) -> Vec<(i64, Vec<(Factor, u32)>)> {
        with(|arena| {
            arena.polys[self.poly.0 as usize]
                .0
                .iter()
                .map(|(mono, coeff)| {
                    let factors = mono
                        .iter()
                        .map(|&(atom, exp)| {
                            let factor = match arena.atoms[atom.0 as usize] {
                                Atom::Param(name) => Factor::Name(name),
                                Atom::Opaque(op, left, right) => Factor::Opaque(
                                    op,
                                    ConstExpr { poly: left, written: None },
                                    ConstExpr { poly: right, written: None },
                                ),
                            };
                            (factor, exp)
                        })
                        .collect();
                    (*coeff, factors)
                })
                .collect()
        })
    }

    /// Const parameters of `substitutions` still unbound in this expression.
    pub fn unbound_params(&self, substitutions: &HashMap<String, Type>) -> Vec<&'static str> {
        with(|arena| {
            let mut atoms = Vec::new();
            arena.unbound_params(self.poly, substitutions, &mut atoms);
            atoms.into_iter().map(|atom| arena.param_name(atom)).collect()
        })
    }
}

pub fn verdict(expected: &ConstExpr, actual: &ConstExpr) -> Verdict {
    if expected.poly == actual.poly {
        return Verdict::Equal;
    }
    with(|arena| {
        let difference = arena.apply(DimOp::Sub, expected.poly, actual.poly);
        match arena.lit_of(difference) {
            Some(0) => Verdict::Equal,
            Some(_) => Verdict::Different,
            None => Verdict::CannotProve,
        }
    })
}

/// Unifies two dimensions, binding an unbound parameter that occurs with
/// coefficient ±1. A declared `Dyn` accepts only `Dyn`; a parameter bound to
/// `Dyn` is never proved equal to anything.
pub fn unify(expected: &Dim, actual: &Dim, substitutions: &mut HashMap<String, Type>) -> bool {
    let (expected, actual) = match (expected, actual) {
        (Dim::Dyn, Dim::Dyn) => return true,
        (Dim::Const(expected), Dim::Const(actual)) => (expected, actual),
        _ => return false,
    };
    let solved = with(|arena| {
        let expected = arena.resolve(expected.poly, substitutions)?;
        let actual = arena.resolve(actual.poly, substitutions)?;
        if expected == actual {
            return Some(None);
        }
        let (param, coeff, rest) = arena.linear_in_unbound(expected, actual, substitutions)?;
        let value = match coeff {
            1 => {
                let zero = arena.lit(0);
                arena.apply(DimOp::Sub, zero, rest)
            }
            -1 => rest,
            _ => return None,
        };
        Some(Some((arena.param_name(param), value)))
    });
    match solved {
        Some(None) => true,
        Some(Some((name, value))) => {
            substitutions.insert(name.to_string(), dim_type(Dim::Const(ConstExpr { poly: value, written: None })));
            true
        }
        None => false,
    }
}

/// The parameter that `expected = actual` fails to infer, with its value
/// when `actual` determines it.
pub fn uninferred(expected: &ConstExpr, actual: &ConstExpr, substitutions: &HashMap<String, Type>) -> Option<(&'static str, Option<i64>)> {
    with(|arena| {
        let expected = arena.resolve(expected.poly, substitutions)?;
        let actual = arena.resolve(actual.poly, substitutions)?;
        let mut unbound = Vec::new();
        arena.unbound_params(expected, substitutions, &mut unbound);
        let first = *unbound.first()?;
        let value = arena.linear_in_unbound(expected, actual, substitutions).and_then(|(_, coeff, rest)| {
            let rest = arena.lit_of(rest)?;
            (rest.checked_rem(coeff)? == 0).then(|| rest.checked_div(coeff)?.checked_neg()).flatten()
        });
        Some((arena.param_name(first), value))
    })
}

#[derive(Clone, Debug)]
pub enum Factor {
    Name(&'static str),
    Opaque(DimOp, ConstExpr, ConstExpr),
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum Dim {
    Const(ConstExpr),
    Dyn,
}

impl fmt::Display for ConstExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.written {
            Some((text, _)) => f.write_str(text),
            None => f.write_str(&self.normal_form()),
        }
    }
}

impl fmt::Display for Dim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Dim::Const(expr) => write!(f, "{expr}"),
            Dim::Dyn => write!(f, "Dyn"),
        }
    }
}

/// A lone parameter reference stays a `Type::Generic`, so ordinary generic
/// binding covers it; anything else is a `Type::Dim`.
pub fn dim_type(dim: Dim) -> Type {
    match dim {
        Dim::Const(expr) => match expr.as_param() {
            Some(name) => Type::Generic(name.to_string()),
            None => Type::Dim(Dim::Const(expr)),
        },
        Dim::Dyn => Type::Dim(Dim::Dyn),
    }
}

pub fn substitute_dim(dim: &Dim, substitutions: &HashMap<String, Type>) -> Type {
    match dim {
        Dim::Dyn => Type::Dim(Dim::Dyn),
        Dim::Const(expr) => match expr.substitute(substitutions) {
            Some(result) if result.poly == expr.poly => Type::Dim(dim.clone()),
            Some(expr) => dim_type(Dim::Const(expr)),
            None => Type::Dim(Dim::Dyn),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn param(name: &str) -> ConstExpr {
        ConstExpr::param(name)
    }

    fn lit(value: i64) -> ConstExpr {
        ConstExpr::lit(value)
    }

    fn add(a: &ConstExpr, b: &ConstExpr) -> ConstExpr {
        ConstExpr::binary(DimOp::Add, a, b)
    }

    fn mul(a: &ConstExpr, b: &ConstExpr) -> ConstExpr {
        ConstExpr::binary(DimOp::Mul, a, b)
    }

    fn div(a: &ConstExpr, b: &ConstExpr) -> ConstExpr {
        ConstExpr::binary(DimOp::Div, a, b)
    }

    #[test]
    fn commutative_reordering_is_equal() {
        assert_eq!(mul(&param("M"), &lit(2)), mul(&lit(2), &param("M")));
    }

    #[test]
    fn repeated_addition_equals_multiplication() {
        let b = param("B");
        assert_eq!(add(&b, &b), mul(&lit(2), &b));
        assert_eq!(verdict(&add(&b, &b), &mul(&lit(2), &b)), Verdict::Equal);
    }

    #[test]
    fn distributivity_is_applied() {
        let n = param("N");
        assert_eq!(mul(&add(&n, &lit(1)), &lit(2)), add(&mul(&lit(2), &n), &lit(2)));
    }

    #[test]
    fn literal_folding() {
        assert_eq!(mul(&lit(28), &lit(28)), lit(784));
        assert_eq!(add(&lit(1), &lit(1)).as_lit(), Some(2));
        assert_eq!(ConstExpr::binary(DimOp::Sub, &lit(10), &lit(4)).as_lit(), Some(6));
        assert_eq!(div(&lit(10), &lit(4)).as_lit(), Some(2));
        assert_eq!(ConstExpr::binary(DimOp::Rem, &lit(10), &lit(4)).as_lit(), Some(2));
    }

    #[test]
    fn folding_happens_across_reordered_terms() {
        let m = param("M");
        assert_eq!(add(&add(&lit(1), &m), &lit(1)), add(&m, &lit(2)));
    }

    #[test]
    fn division_is_opaque() {
        let n = param("N");
        let halved = div(&mul(&lit(2), &n), &lit(2));
        assert_ne!(halved, n);
        assert_eq!(verdict(&halved, &n), Verdict::CannotProve);
        assert_eq!(halved, div(&mul(&n, &lit(2)), &lit(2)));
        assert_eq!(halved.normal_form(), "(2 * N) / 2");
    }

    #[test]
    fn remainder_is_opaque() {
        let n = param("N");
        let rem = ConstExpr::binary(DimOp::Rem, &n, &lit(4));
        assert_eq!(rem.as_lit(), None);
        assert_eq!(verdict(&rem, &lit(0)), Verdict::CannotProve);
    }

    #[test]
    fn division_by_zero_is_left_unfolded() {
        assert!(div(&lit(1), &lit(0)).as_lit().is_none());
    }

    #[test]
    fn coefficient_overflow_is_opaque() {
        let n = param("N");
        let big = mul(&mul(&n, &lit(i64::MAX)), &lit(2));
        assert_eq!(big.as_lit(), None);
        assert_eq!(verdict(&big, &n), Verdict::CannotProve);
        assert_eq!(mul(&lit(i64::MAX), &lit(2)).as_lit(), None);
    }

    #[test]
    fn verdicts_distinguish_different_from_unproved() {
        let n = param("N");
        let two_n = mul(&lit(2), &n);
        assert_eq!(verdict(&two_n, &add(&two_n, &lit(1))), Verdict::Different);
        assert_eq!(verdict(&lit(3), &lit(4)), Verdict::Different);
        assert_eq!(verdict(&two_n, &n), Verdict::CannotProve);
        assert_eq!(verdict(&two_n, &add(&n, &n)), Verdict::Equal);
    }

    #[test]
    fn substitution_folds_to_a_literal() {
        let expr = mul(&param("M"), &lit(2));
        let subs = HashMap::from([("M".to_string(), Type::Dim(Dim::Const(lit(384))))]);
        assert_eq!(expr.substitute(&subs), Some(lit(768)));
    }

    #[test]
    fn substituting_dyn_makes_the_expression_dyn() {
        let expr = mul(&param("M"), &lit(2));
        let subs = HashMap::from([("M".to_string(), Type::Dim(Dim::Dyn))]);
        assert_eq!(expr.substitute(&subs), None);
    }

    #[test]
    fn substitution_reaches_inside_opaque_atoms() {
        let expr = div(&param("M"), &lit(2));
        let subs = HashMap::from([("M".to_string(), Type::Dim(Dim::Const(lit(8))))]);
        assert_eq!(expr.substitute(&subs), Some(lit(4)));
    }

    #[test]
    fn an_offset_parameter_is_inferred() {
        let n = param("N");
        let mut subs = HashMap::from([("N".to_string(), Type::Generic("N".to_string()))]);
        assert!(unify(&Dim::Const(add(&n, &lit(1))), &Dim::Const(lit(8)), &mut subs));
        assert_eq!(subs["N"], Type::Dim(Dim::Const(lit(7))));
    }

    #[test]
    fn a_scaled_parameter_is_not_inferred() {
        let n = param("N");
        let two_n = mul(&lit(2), &n);
        let mut subs = HashMap::from([("N".to_string(), Type::Generic("N".to_string()))]);
        assert!(!unify(&Dim::Const(two_n.clone()), &Dim::Const(lit(8)), &mut subs));
        assert_eq!(uninferred(&two_n, &lit(8), &subs), Some(("N", Some(4))));
        assert_eq!(uninferred(&two_n, &lit(7), &subs), Some(("N", None)));
    }

    #[test]
    fn written_form_is_kept_for_display() {
        let m = param("M");
        let written = add(&m, &m);
        assert_eq!(written.to_string(), "M + M");
        assert_eq!(written.normal_form(), "2 * M");
        let nested = mul(&add(&param("N"), &lit(1)), &lit(3));
        assert_eq!(nested.to_string(), "(N + 1) * 3");
        assert_eq!(nested.normal_form(), "3 * N + 3");
        assert_eq!(ConstExpr::binary(DimOp::Sub, &lit(1), &param("N")).normal_form(), "-N + 1");
    }
}
