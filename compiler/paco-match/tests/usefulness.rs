use paco_match::{ConstructorSet, analyze_match};
use paco_span::{FileId, Span};
use paco_syntax::ast::{Expr, Literal, MatchArm, Pat};

#[test]
fn reports_unreachable_arm_after_wildcard() {
    let arms = vec![
        arm(Pat::Wildcard(span())),
        arm(Pat::Literal(Literal::Int(1), span())),
    ];

    let report = analyze_match(&arms, ConstructorSet::open("_"));

    assert_eq!(report.unreachable_arms.len(), 1);
    assert_eq!(report.unreachable_arms[0].index, 1);
    assert_eq!(report.unreachable_arms[0].witness.as_deref(), Some("_"));
    assert_eq!(report.missing_witness, None);
}

#[test]
fn reports_missing_enum_constructor_witness() {
    let arms = vec![arm(Pat::Enum {
        path: vec!["Maybe".to_string(), "Some".to_string()],
        fields: vec![Pat::Ident("value".to_string(), span())],
        span: span(),
    })];

    let report = analyze_match(
        &arms,
        ConstructorSet::closed(["Maybe::Some", "Maybe::None"]),
    );

    assert!(report.unreachable_arms.is_empty());
    assert_eq!(report.missing_witness.as_deref(), Some("Maybe::None"));
}

#[test]
fn payload_specific_enum_pattern_does_not_cover_constructor() {
    let arms = vec![arm(Pat::Enum {
        path: vec!["Maybe".to_string(), "Some".to_string()],
        fields: vec![Pat::Literal(Literal::Int(0), span())],
        span: span(),
    })];

    let report = analyze_match(
        &arms,
        ConstructorSet::closed(["Maybe::Some", "Maybe::None"]),
    );

    assert!(report.unreachable_arms.is_empty());
    assert_eq!(report.missing_witness.as_deref(), Some("Maybe::Some"));
}

#[test]
fn or_pattern_covers_each_literal_alternative() {
    let arms = vec![arm(Pat::Or(
        vec![
            Pat::Literal(Literal::Bool(true), span()),
            Pat::Literal(Literal::Bool(false), span()),
        ],
        span(),
    ))];

    let report = analyze_match(&arms, ConstructorSet::closed(["true", "false"]));

    assert!(report.unreachable_arms.is_empty());
    assert_eq!(report.missing_witness, None);
}

#[test]
fn range_pattern_makes_contained_literal_arm_unreachable() {
    let arms = vec![
        arm(Pat::Range {
            start: Box::new(Pat::Literal(Literal::Int(1), span())),
            end: Box::new(Pat::Literal(Literal::Int(9), span())),
            inclusive: true,
            span: span(),
        }),
        arm(Pat::Literal(Literal::Int(3), span())),
    ];

    let report = analyze_match(&arms, ConstructorSet::open("_"));

    assert_eq!(report.unreachable_arms.len(), 1);
    assert_eq!(report.unreachable_arms[0].index, 1);
    assert_eq!(report.unreachable_arms[0].witness.as_deref(), Some("3"));
    assert_eq!(report.missing_witness.as_deref(), Some("_"));
}

#[test]
fn closed_constructor_coverage_makes_following_wildcard_unreachable() {
    let arms = vec![
        arm(Pat::Literal(Literal::Bool(true), span())),
        arm(Pat::Literal(Literal::Bool(false), span())),
        arm(Pat::Wildcard(span())),
    ];

    let report = analyze_match(&arms, ConstructorSet::closed(["true", "false"]));

    assert_eq!(report.unreachable_arms.len(), 1);
    assert_eq!(report.unreachable_arms[0].index, 2);
    assert_eq!(
        report.unreachable_arms[0].witness.as_deref(),
        Some("all constructors")
    );
    assert_eq!(report.missing_witness, None);
}

#[test]
fn guarded_arm_does_not_prove_exhaustiveness() {
    let arms = vec![MatchArm {
        pattern: Pat::Literal(Literal::Bool(true), span()),
        guard: Some(Expr::Literal(Literal::Bool(true), span())),
        body: Expr::Literal(Literal::Int(1), span()),
        span: span(),
    }];

    let report = analyze_match(&arms, ConstructorSet::closed(["true", "false"]));

    assert!(report.unreachable_arms.is_empty());
    assert_eq!(report.missing_witness.as_deref(), Some("true"));
}

fn arm(pattern: Pat) -> MatchArm {
    MatchArm {
        pattern,
        guard: None,
        body: Expr::Literal(Literal::Int(1), span()),
        span: span(),
    }
}

fn span() -> Span {
    Span::new(FileId::new(0), 0, 0)
}
