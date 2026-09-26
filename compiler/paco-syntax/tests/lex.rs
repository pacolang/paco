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
fn lexer_tokenizes_brackets() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "[]T");
    let mut reporter = Reporter::new();

    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let kinds: Vec<_> = tokens.iter().map(|token| token.kind).collect();

    assert_eq!(
        kinds,
        vec![
            TokenKind::LeftBracket,
            TokenKind::RightBracket,
            TokenKind::Identifier,
            TokenKind::Eof,
        ]
    );
    assert!(!reporter.has_errors());
}

#[test]
fn lexer_tokenizes_const_as_a_keyword_not_an_identifier() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "const TILE: i64 = 64;");
    let mut reporter = Reporter::new();

    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let kinds: Vec<_> = tokens.iter().map(|token| token.kind).collect();

    assert_eq!(
        kinds,
        vec![
            TokenKind::Const,
            TokenKind::Identifier,
            TokenKind::Colon,
            TokenKind::Identifier,
            TokenKind::Equal,
            TokenKind::Integer,
            TokenKind::Semicolon,
            TokenKind::Eof,
        ]
    );
    assert!(!reporter.has_errors());
}

#[test]
fn lexer_tokenizes_type_as_a_keyword_not_an_identifier() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "type Output");
    let mut reporter = Reporter::new();

    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let kinds: Vec<_> = tokens.iter().map(|token| token.kind).collect();

    assert_eq!(kinds, vec![TokenKind::Type, TokenKind::Identifier, TokenKind::Eof]);
    assert!(!reporter.has_errors());
}

#[test]
fn lexer_tokenizes_module_pub_and_as_as_keywords() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "module nn pub as");
    let mut reporter = Reporter::new();

    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let kinds: Vec<_> = tokens.iter().map(|token| token.kind).collect();

    assert_eq!(
        kinds,
        vec![
            TokenKind::Module,
            TokenKind::Identifier,
            TokenKind::Pub,
            TokenKind::As,
            TokenKind::Eof,
        ]
    );
    assert!(!reporter.has_errors());
}

#[test]
fn lexer_tokenizes_question_mark_as_its_own_token() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "x?");
    let mut reporter = Reporter::new();

    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let kinds: Vec<_> = tokens.iter().map(|token| token.kind).collect();

    assert_eq!(
        kinds,
        vec![TokenKind::Identifier, TokenKind::Question, TokenKind::Eof]
    );
    assert!(!reporter.has_errors());
}

#[test]
fn lexer_tokenizes_extern_and_unsafe_as_keywords_not_identifiers() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "extern unsafe const externally unsafely constant");
    let mut reporter = Reporter::new();

    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let kinds: Vec<_> = tokens.iter().map(|token| token.kind).collect();

    assert_eq!(
        kinds,
        vec![
            TokenKind::Extern,
            TokenKind::Unsafe,
            TokenKind::Const,
            TokenKind::Identifier,
            TokenKind::Identifier,
            TokenKind::Identifier,
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

#[test]
fn lexer_distinguishes_char_literals_from_lifetimes() {
    let mut sources = SourceMap::new();
    let file = sources.add_file(
        "main.paco",
        "fn f(x: &'a i64) { let c = 'a'; let n = '\\n'; let q = '\\''; }",
    );
    let mut reporter = Reporter::new();

    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);

    assert!(!reporter.has_errors(), "{reporter:?}");
    let lifetime = tokens
        .iter()
        .find(|token| token.kind == TokenKind::Lifetime)
        .expect("expected a lifetime token for `&'a i64`");
    assert_eq!(lifetime.lexeme, "'a");
    let chars: Vec<_> = tokens
        .iter()
        .filter(|token| token.kind == TokenKind::Char)
        .map(|token| token.lexeme.as_str())
        .collect();
    assert_eq!(chars, vec!["'a'", "'\\n'", "'\\''"]);
}

#[test]
fn lexer_tokenizes_a_derive_attribute() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "#[derive(Display)]");
    let mut reporter = Reporter::new();

    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let kinds: Vec<_> = tokens.iter().map(|token| token.kind).collect();

    assert!(!reporter.has_errors(), "{reporter:?}");
    assert_eq!(
        kinds,
        vec![
            TokenKind::Hash,
            TokenKind::LeftBracket,
            TokenKind::Identifier,
            TokenKind::LeftParen,
            TokenKind::Identifier,
            TokenKind::RightParen,
            TokenKind::RightBracket,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn lexer_tokenizes_caret_and_tilde() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "a ^ ~b");
    let mut reporter = Reporter::new();

    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let kinds: Vec<_> = tokens.iter().map(|token| token.kind).collect();

    assert_eq!(
        kinds,
        vec![TokenKind::Identifier, TokenKind::Caret, TokenKind::Tilde, TokenKind::Identifier, TokenKind::Eof]
    );
    assert!(!reporter.has_errors());
}
