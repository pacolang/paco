//! Hand-written lexer for Paco compiler features.

use paco_diag::{Diagnostic, Reporter};
use paco_span::{FileId, Span};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenKind {
    Fn,
    Struct,
    Enum,
    Trait,
    Methods,
    Use,
    Let,
    Mut,
    Const,
    Type,
    Module,
    Pub,
    As,
    Extern,
    Unsafe,
    Spawn,
    Select,
    Yield,
    Iter,
    Comptime,
    Quote,
    Match,
    If,
    Else,
    While,
    For,
    In,
    Loop,
    Break,
    Continue,
    Return,
    True,
    False,
    Identifier,
    Lifetime,
    Integer,
    Float,
    String,
    Char,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Equal,
    EqualEqual,
    Bang,
    BangEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    AndAnd,
    OrOr,
    Arrow,
    Colon,
    ColonColon,
    Comma,
    Dot,
    DotDot,
    DotDotDot,
    DotDotEqual,
    Semicolon,
    FatArrow,
    At,
    Ampersand,
    Pipe,
    Underscore,
    LeftParen,
    RightParen,
    LeftBrace,
    RightBrace,
    LeftBracket,
    RightBracket,
    Question,
    Hash,
    Caret,
    Tilde,
    Error,
    Eof,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub lexeme: String,
    pub span: Span,
}

impl Token {
    fn new(kind: TokenKind, lexeme: impl Into<String>, span: Span) -> Self {
        Self {
            kind,
            lexeme: lexeme.into(),
            span,
        }
    }
}

pub fn lex(source: &str, file_id: FileId, reporter: &mut Reporter) -> Vec<Token> {
    Lexer {
        source,
        file_id,
        reporter,
        tokens: Vec::new(),
        offset: 0,
    }
    .run()
}

struct Lexer<'a, 'reporter> {
    source: &'a str,
    file_id: FileId,
    reporter: &'reporter mut Reporter,
    tokens: Vec<Token>,
    offset: usize,
}

impl Lexer<'_, '_> {
    fn run(mut self) -> Vec<Token> {
        while !self.is_at_end() {
            self.scan_token();
        }
        self.tokens.push(Token::new(
            TokenKind::Eof,
            "",
            Span::new(self.file_id, self.offset, self.offset),
        ));
        self.tokens
    }

    fn scan_token(&mut self) {
        let start = self.offset;
        let Some(ch) = self.advance() else {
            return;
        };

        match ch {
            '(' => self.push(TokenKind::LeftParen, start),
            ')' => self.push(TokenKind::RightParen, start),
            '{' => self.push(TokenKind::LeftBrace, start),
            '}' => self.push(TokenKind::RightBrace, start),
            '[' => self.push(TokenKind::LeftBracket, start),
            ']' => self.push(TokenKind::RightBracket, start),
            ',' => self.push(TokenKind::Comma, start),
            '.' => {
                if self.match_char('.') {
                    if self.match_char('=') {
                        self.push(TokenKind::DotDotEqual, start);
                    } else if self.match_char('.') {
                        self.push(TokenKind::DotDotDot, start);
                    } else {
                        self.push(TokenKind::DotDot, start);
                    }
                } else {
                    self.push(TokenKind::Dot, start);
                }
            }
            ':' if self.match_char(':') => self.push(TokenKind::ColonColon, start),
            ':' => self.push(TokenKind::Colon, start),
            ';' => self.push(TokenKind::Semicolon, start),
            '+' => self.push(TokenKind::Plus, start),
            '*' => self.push(TokenKind::Star, start),
            '%' => self.push(TokenKind::Percent, start),
            '-' if self.match_char('>') => self.push(TokenKind::Arrow, start),
            '-' => self.push(TokenKind::Minus, start),
            '!' if self.match_char('=') => self.push(TokenKind::BangEqual, start),
            '!' => self.push(TokenKind::Bang, start),
            '=' if self.match_char('=') => self.push(TokenKind::EqualEqual, start),
            '=' if self.match_char('>') => self.push(TokenKind::FatArrow, start),
            '=' => self.push(TokenKind::Equal, start),
            '<' if self.match_char('=') => self.push(TokenKind::LessEqual, start),
            '<' => self.push(TokenKind::Less, start),
            '>' if self.match_char('=') => self.push(TokenKind::GreaterEqual, start),
            '>' => self.push(TokenKind::Greater, start),
            '&' if self.match_char('&') => self.push(TokenKind::AndAnd, start),
            '&' => self.push(TokenKind::Ampersand, start),
            '|' if self.match_char('|') => self.push(TokenKind::OrOr, start),
            '|' => self.push(TokenKind::Pipe, start),
            '@' => self.push(TokenKind::At, start),
            '?' => self.push(TokenKind::Question, start),
            '#' => self.push(TokenKind::Hash, start),
            '^' => self.push(TokenKind::Caret, start),
            '~' => self.push(TokenKind::Tilde, start),
            '/' if self.match_char('/') => self.skip_line_comment(),
            '/' if self.match_char('*') => self.skip_block_comment(start),
            '/' => self.push(TokenKind::Slash, start),
            '"' => self.string(start),
            '\'' if self.looks_like_char_literal() => self.char_literal(start),
            '\'' if self.peek().is_some_and(is_identifier_start) => self.lifetime(start),
            ch if ch.is_ascii_whitespace() => {}
            ch if ch.is_ascii_digit() => self.number(start),
            ch if is_identifier_start(ch) => self.identifier(start),
            _ => {
                let span = Span::new(self.file_id, start, self.offset);
                self.tokens.push(Token::new(
                    TokenKind::Error,
                    &self.source[start..self.offset],
                    span,
                ));
                self.reporter.push(Diagnostic::error(
                    "PACO-E0100",
                    span,
                    format!("invalid character `{ch}`"),
                ));
            }
        }
    }

    fn string(&mut self, start: usize) {
        let mut escaped = false;
        while let Some(ch) = self.peek() {
            if escaped {
                escaped = false;
                self.advance();
                continue;
            }
            match ch {
                '\\' => {
                    escaped = true;
                    self.advance();
                }
                '"' => break,
                _ => {
                    self.advance();
                }
            }
        }

        if self.is_at_end() {
            let span = Span::new(self.file_id, start, self.offset);
            self.tokens.push(Token::new(
                TokenKind::Error,
                &self.source[start..self.offset],
                span,
            ));
            self.reporter
                .push(Diagnostic::error("PACO-E0101", span, "unterminated string"));
            return;
        }

        self.advance();
        self.push(TokenKind::String, start);
    }

    fn number(&mut self, start: usize) {
        while matches!(self.peek(), Some(ch) if ch.is_ascii_digit() || ch == '_') {
            self.advance();
        }

        let mut kind = TokenKind::Integer;
        if self.peek() == Some('.') && self.peek_next().is_some_and(|ch| ch.is_ascii_digit()) {
            kind = TokenKind::Float;
            self.advance();
            while matches!(self.peek(), Some(ch) if ch.is_ascii_digit() || ch == '_') {
                self.advance();
            }
        }

        self.push(kind, start);
    }

    fn identifier(&mut self, start: usize) {
        while matches!(self.peek(), Some(ch) if is_identifier_continue(ch)) {
            self.advance();
        }

        let lexeme = &self.source[start..self.offset];
        let kind = match lexeme {
            "fn" => TokenKind::Fn,
            "struct" => TokenKind::Struct,
            "enum" => TokenKind::Enum,
            "trait" => TokenKind::Trait,
            "methods" => TokenKind::Methods,
            "use" => TokenKind::Use,
            "module" => TokenKind::Module,
            "pub" => TokenKind::Pub,
            "as" => TokenKind::As,
            "extern" => TokenKind::Extern,
            "unsafe" => TokenKind::Unsafe,
            "comptime" => TokenKind::Comptime,
            "quote" => TokenKind::Quote,
            "spawn" => TokenKind::Spawn,
            "select" => TokenKind::Select,
            "yield" => TokenKind::Yield,
            "iter" => TokenKind::Iter,
            "let" => TokenKind::Let,
            "mut" => TokenKind::Mut,
            "const" => TokenKind::Const,
            "type" => TokenKind::Type,
            "match" => TokenKind::Match,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "while" => TokenKind::While,
            "for" => TokenKind::For,
            "in" => TokenKind::In,
            "loop" => TokenKind::Loop,
            "break" => TokenKind::Break,
            "continue" => TokenKind::Continue,
            "return" => TokenKind::Return,
            "true" => TokenKind::True,
            "false" => TokenKind::False,
            "_" => TokenKind::Underscore,
            _ => TokenKind::Identifier,
        };
        self.push(kind, start);
    }

    fn lifetime(&mut self, start: usize) {
        while matches!(self.peek(), Some(ch) if is_identifier_continue(ch)) {
            self.advance();
        }
        self.push(TokenKind::Lifetime, start);
    }

    /// Disambiguates `'a'` (a char literal) from `'a` (a lifetime, e.g.
    /// `&'a string`) by lookahead: a char literal's content is either a
    /// `\`-escape followed immediately by a closing `'`, or a single
    /// character followed immediately by a closing `'`. A lifetime's
    /// identifier is not immediately followed by a closing `'`.
    fn looks_like_char_literal(&self) -> bool {
        match self.peek() {
            Some('\\') => self.peek_at(2) == Some('\''),
            Some('\'') | None => false,
            Some(_) => self.peek_next() == Some('\''),
        }
    }

    fn peek_at(&self, ahead: usize) -> Option<char> {
        self.source[self.offset..].chars().nth(ahead)
    }

    fn char_literal(&mut self, start: usize) {
        if self.peek() == Some('\\') {
            self.advance();
        }
        self.advance();
        if self.peek() == Some('\'') {
            self.advance();
        }
        self.push(TokenKind::Char, start);
    }

    fn skip_line_comment(&mut self) {
        while !self.is_at_end() && self.peek() != Some('\n') {
            self.advance();
        }
    }

    fn skip_block_comment(&mut self, start: usize) {
        while !self.is_at_end() {
            if self.peek() == Some('*') && self.peek_next() == Some('/') {
                self.advance();
                self.advance();
                return;
            }
            self.advance();
        }

        let span = Span::new(self.file_id, start, self.offset);
        self.reporter.push(Diagnostic::error(
            "PACO-E0102",
            span,
            "unterminated block comment",
        ));
    }

    fn push(&mut self, kind: TokenKind, start: usize) {
        self.tokens.push(Token::new(
            kind,
            &self.source[start..self.offset],
            Span::new(self.file_id, start, self.offset),
        ));
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.offset += ch.len_utf8();
        Some(ch)
    }

    fn match_char(&mut self, expected: char) -> bool {
        if self.peek() != Some(expected) {
            return false;
        }
        self.advance();
        true
    }

    fn peek(&self) -> Option<char> {
        self.source[self.offset..].chars().next()
    }

    fn peek_next(&self) -> Option<char> {
        let mut chars = self.source[self.offset..].chars();
        chars.next()?;
        chars.next()
    }

    fn is_at_end(&self) -> bool {
        self.offset >= self.source.len()
    }
}

fn is_identifier_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

fn is_identifier_continue(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}
