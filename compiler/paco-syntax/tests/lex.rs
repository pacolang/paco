use paco_diag::Reporter;
use paco_span::SourceMap;
use paco_syntax::lex::{TokenKind, lex};

#[test]
fn lexer_tokenizes_core_program() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "fn main() { print(\"Hello, world!\") }");
    let mut reporter = Reporter::new();

    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let kinds: Vec<_> = tokens.iter().map(|token| token.kind).collect();

    assert_eq!(
        kinds,
        vec![
            TokenKind::Fn,
            TokenKind::Identifier,
            TokenKind::LeftParen,
            TokenKind::RightParen,
            TokenKind::LeftBrace,
            TokenKind::Identifier,
            TokenKind::LeftParen,
            TokenKind::String,
            TokenKind::RightParen,
            TokenKind::RightBrace,
            TokenKind::Eof,
        ]
    );
    assert!(!reporter.has_errors());
}

#[test]
fn lexer_reports_invalid_characters_without_aborting() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "fn main() { $ }");
    let mut reporter = Reporter::new();

    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);

    assert!(tokens.iter().any(|token| token.kind == TokenKind::Error));
    assert!(reporter.has_errors());
    assert!(reporter.emit_to_string(&sources).contains("PACO-E0100"));
}

#[test]
fn lexer_tokenizes_pattern_matching_tokens() {
    let mut sources = SourceMap::new();
    let file = sources.add_file(
        "main.paco",
        "match value { n @ 1..=9 => n, _ => 0 } for x in xs { x | 1..2 }",
    );
    let mut reporter = Reporter::new();

    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let kinds: Vec<_> = tokens.iter().map(|token| token.kind).collect();

    assert!(kinds.contains(&TokenKind::Match));
    assert!(kinds.contains(&TokenKind::At));
    assert!(kinds.contains(&TokenKind::DotDotEqual));
    assert!(kinds.contains(&TokenKind::FatArrow));
    assert!(kinds.contains(&TokenKind::Underscore));
    assert!(kinds.contains(&TokenKind::For));
    assert!(kinds.contains(&TokenKind::In));
    assert!(kinds.contains(&TokenKind::Pipe));
    assert!(kinds.contains(&TokenKind::DotDot));
    assert!(!reporter.has_errors());
}
