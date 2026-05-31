//! Pattern usefulness and exhaustiveness analysis.

use std::collections::HashSet;

use paco_syntax::ast::{Literal, MatchArm, Pat};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConstructorSet {
    constructors: Vec<String>,
    fallback_witness: Option<String>,
}

impl ConstructorSet {
    pub fn closed(constructors: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            constructors: constructors.into_iter().map(Into::into).collect(),
            fallback_witness: None,
        }
    }

    pub fn open(witness: impl Into<String>) -> Self {
        Self {
            constructors: Vec::new(),
            fallback_witness: Some(witness.into()),
        }
    }

    fn is_exhausted_by(&self, covered: &CoveredPatterns) -> bool {
        self.fallback_witness.is_none()
            && !self.constructors.is_empty()
            && self
                .constructors
                .iter()
                .all(|constructor| covered.contains_key(constructor))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsefulnessReport {
    pub unreachable_arms: Vec<UnreachableArm>,
    pub missing_witness: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnreachableArm {
    pub index: usize,
    pub witness: Option<String>,
}

pub fn analyze_match(arms: &[MatchArm], constructors: ConstructorSet) -> UsefulnessReport {
    let mut catch_all = false;
    let mut catch_all_witness = None;
    let mut covered = CoveredPatterns::default();
    let mut unreachable_arms = Vec::new();

    for (index, arm) in arms.iter().enumerate() {
        let coverage = pattern_coverage(&arm.pattern);
        if catch_all {
            unreachable_arms.push(UnreachableArm {
                index,
                witness: catch_all_witness.clone(),
            });
            continue;
        }
        if let Some(witness) = coverage.covered_witness(&covered) {
            unreachable_arms.push(UnreachableArm {
                index,
                witness: Some(witness),
            });
            continue;
        }
        if arm.guard.is_some() {
            continue;
        }
        match coverage {
            PatternCoverage::CatchAll => {
                catch_all = true;
                catch_all_witness = Some("_".to_string());
            }
            PatternCoverage::Covered(patterns) => covered.extend(patterns),
            PatternCoverage::Opaque => {}
        }
        if constructors.is_exhausted_by(&covered) {
            catch_all = true;
            catch_all_witness = Some("all constructors".to_string());
        }
    }

    let missing_witness = if catch_all {
        None
    } else {
        constructors
            .constructors
            .iter()
            .find(|constructor| !covered.contains_key(constructor))
            .cloned()
            .or(constructors.fallback_witness)
    };

    UsefulnessReport {
        unreachable_arms,
        missing_witness,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PatternCoverage {
    CatchAll,
    Covered(CoveredPatterns),
    Opaque,
}

impl PatternCoverage {
    fn covered_witness(&self, covered: &CoveredPatterns) -> Option<String> {
        match self {
            Self::CatchAll | Self::Opaque => None,
            Self::Covered(patterns) => patterns.covered_witness(covered),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct CoveredPatterns {
    keys: HashSet<String>,
    int_ranges: Vec<IntRange>,
}

impl CoveredPatterns {
    fn key(key: String) -> Self {
        Self {
            keys: HashSet::from([key]),
            int_ranges: Vec::new(),
        }
    }

    fn int_range(range: IntRange) -> Self {
        Self {
            keys: HashSet::new(),
            int_ranges: vec![range],
        }
    }

    fn contains_key(&self, key: &str) -> bool {
        self.keys.contains(key)
            || key
                .parse::<i64>()
                .is_ok_and(|value| self.int_ranges.iter().any(|range| range.contains(value)))
    }

    fn extend(&mut self, other: Self) {
        self.keys.extend(other.keys);
        self.int_ranges.extend(other.int_ranges);
    }

    fn covered_witness(&self, covered: &Self) -> Option<String> {
        let first_uncovered_key = self.keys.iter().find(|key| !covered.contains_key(key));
        if first_uncovered_key.is_some() {
            return None;
        }
        let first_uncovered_range = self.int_ranges.iter().find(|range| {
            !covered
                .int_ranges
                .iter()
                .any(|covered_range| covered_range.contains_range(range))
        });
        if first_uncovered_range.is_some() {
            return None;
        }
        self.keys
            .iter()
            .next()
            .cloned()
            .or_else(|| self.int_ranges.first().map(IntRange::display))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct IntRange {
    start: i64,
    end: i64,
    inclusive: bool,
}

impl IntRange {
    fn contains(self, value: i64) -> bool {
        let Some(end) = self.inclusive_end() else {
            return false;
        };
        self.start <= value && value <= end
    }

    fn contains_range(self, other: &Self) -> bool {
        let Some(self_end) = self.inclusive_end() else {
            return false;
        };
        let Some(other_end) = other.inclusive_end() else {
            return true;
        };
        self.start <= other.start && other_end <= self_end
    }

    fn inclusive_end(self) -> Option<i64> {
        if self.inclusive {
            Some(self.end)
        } else {
            self.end.checked_sub(1)
        }
    }

    fn display(&self) -> String {
        if self.inclusive {
            format!("{}..={}", self.start, self.end)
        } else {
            format!("{}..{}", self.start, self.end)
        }
    }
}

fn pattern_coverage(pattern: &Pat) -> PatternCoverage {
    match pattern {
        Pat::Wildcard(_) | Pat::Ident(_, _) => PatternCoverage::CatchAll,
        Pat::Binding { pattern, .. } => pattern_coverage(pattern),
        Pat::Literal(literal, _) => {
            PatternCoverage::Covered(CoveredPatterns::key(literal_key(literal)))
        }
        Pat::Enum { path, fields, .. }
            if path.len() >= 2 && fields.iter().all(is_irrefutable_pattern) =>
        {
            PatternCoverage::Covered(CoveredPatterns::key(format!(
                "{}::{}",
                path[0],
                path.last().unwrap()
            )))
        }
        Pat::Range {
            start,
            end,
            inclusive,
            ..
        } => match int_range(start, end, *inclusive) {
            Some(range) => PatternCoverage::Covered(CoveredPatterns::int_range(range)),
            None => PatternCoverage::Opaque,
        },
        Pat::Or(patterns, _) => or_pattern_coverage(patterns),
        Pat::Tuple(_, _) | Pat::Struct { .. } | Pat::Enum { .. } => PatternCoverage::Opaque,
    }
}

fn int_range(start: &Pat, end: &Pat, inclusive: bool) -> Option<IntRange> {
    Some(IntRange {
        start: int_literal(start)?,
        end: int_literal(end)?,
        inclusive,
    })
}

fn int_literal(pattern: &Pat) -> Option<i64> {
    match pattern {
        Pat::Literal(Literal::Int(value), _) => Some(*value),
        _ => None,
    }
}

fn is_irrefutable_pattern(pattern: &Pat) -> bool {
    match pattern {
        Pat::Wildcard(_) | Pat::Ident(_, _) => true,
        Pat::Binding { pattern, .. } => is_irrefutable_pattern(pattern),
        Pat::Or(patterns, _) => patterns.iter().any(is_irrefutable_pattern),
        Pat::Literal(_, _)
        | Pat::Tuple(_, _)
        | Pat::Struct { .. }
        | Pat::Enum { .. }
        | Pat::Range { .. } => false,
    }
}

fn or_pattern_coverage(patterns: &[Pat]) -> PatternCoverage {
    let mut covered = CoveredPatterns::default();
    for pattern in patterns {
        match pattern_coverage(pattern) {
            PatternCoverage::CatchAll => return PatternCoverage::CatchAll,
            PatternCoverage::Covered(patterns) => covered.extend(patterns),
            PatternCoverage::Opaque => return PatternCoverage::Opaque,
        }
    }
    if covered.keys.is_empty() && covered.int_ranges.is_empty() {
        PatternCoverage::Opaque
    } else {
        PatternCoverage::Covered(covered)
    }
}

fn literal_key(literal: &Literal) -> String {
    match literal {
        Literal::Bool(value) => value.to_string(),
        Literal::Int(value) => value.to_string(),
        Literal::Float(value) => value.to_string(),
        Literal::String(value) => format!("{value:?}"),
        Literal::Char(value) => value.to_string(),
    }
}
