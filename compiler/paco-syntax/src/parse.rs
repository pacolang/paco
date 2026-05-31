//! Recursive-descent parser for the initial executable Paco subset.

use paco_diag::{Diagnostic, Reporter};
use paco_span::Span;

use crate::{
    ast::{
        BinaryOp, Block, Expr, FnDecl, Item, LetStmt, Literal, Module, Param, Pat, Stmt, Ty,
        UnaryOp,
    },
    lex::{Token, TokenKind},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParseError;

pub type ParseResult<T> = Result<T, ParseError>;

pub fn parse_module(tokens: &[Token], reporter: &mut Reporter) -> ParseResult<Module> {
    Parser {
        tokens,
        reporter,
        current: 0,
    }
    .parse_module()
}

struct Parser<'a, 'reporter> {
    tokens: &'a [Token],
    reporter: &'reporter mut Reporter,
    current: usize,
}

impl Parser<'_, '_> {
    fn parse_module(&mut self) -> ParseResult<Module> {
        let start = self.peek().span.start();
        let mut items = Vec::new();

        while !self.check(TokenKind::Eof) {
            if self.matches(TokenKind::Fn) {
                items.push(Item::Fn(self.function_decl()?));
            } else {
                self.error_here("PACO-E0110", "expected item declaration");
                self.synchronize_item();
            }
        }

        let end = self.peek().span.end();
        Ok(Module {
            items,
            span: Span::new(self.peek().span.file_id(), start, end),
        })
    }

    fn function_decl(&mut self) -> ParseResult<FnDecl> {
        let start = self.previous().span.start();
        let name = self.consume_identifier("expected function name")?;
        self.consume(TokenKind::LeftParen, "expected `(` after function name")?;
        let params = self.parameter_list()?;
        self.consume(TokenKind::RightParen, "expected `)` after parameters")?;
        let return_ty = if self.matches(TokenKind::Arrow) {
            Some(self.ty()?)
        } else {
            None
        };
        let body = self.block()?;
        let span = Span::new(
            self.previous().span.file_id(),
            start,
            body.span.end().max(self.previous().span.end()),
        );
        Ok(FnDecl {
            name,
            params,
            return_ty,
            body,
            span,
        })
    }

    fn parameter_list(&mut self) -> ParseResult<Vec<Param>> {
        let mut params = Vec::new();
        if self.check(TokenKind::RightParen) {
            return Ok(params);
        }

        loop {
            let start = self.peek().span.start();
            let name = self.consume_identifier("expected parameter name")?;
            self.consume(TokenKind::Colon, "expected `:` after parameter name")?;
            let ty = self.ty()?;
            let span = Span::new(
                self.previous().span.file_id(),
                start,
                self.previous().span.end(),
            );
            params.push(Param {
                pattern: Pat::Ident(name, span),
                ty,
                span,
            });
            if !self.matches(TokenKind::Comma) {
                break;
            }
        }
        Ok(params)
    }

    fn ty(&mut self) -> ParseResult<Ty> {
        let token = self.consume(TokenKind::Identifier, "expected type name")?;
        Ok(Ty::Path(vec![token.lexeme.clone()], token.span))
    }

    fn block(&mut self) -> ParseResult<Block> {
        let left_span = self
            .consume(TokenKind::LeftBrace, "expected `{` before block")?
            .span;
        let mut stmts = Vec::new();
        let mut tail = None;

        while !self.check(TokenKind::RightBrace) && !self.check(TokenKind::Eof) {
            if self.matches(TokenKind::Let) {
                stmts.push(Stmt::Let(self.let_stmt()?));
                continue;
            }

            let expr = self.expr()?;
            if self.matches(TokenKind::Semicolon) || !self.check(TokenKind::RightBrace) {
                stmts.push(Stmt::Expr(expr));
            } else {
                tail = Some(Box::new(expr));
                break;
            }
        }

        let right = self.consume(TokenKind::RightBrace, "expected `}` after block")?;
        Ok(Block {
            stmts,
            tail,
            span: Span::new(left_span.file_id(), left_span.start(), right.span.end()),
        })
    }

    fn let_stmt(&mut self) -> ParseResult<LetStmt> {
        let start = self.previous().span.start();
        let mutable = self.matches(TokenKind::Mut);
        let name = self.consume_identifier("expected binding name")?;
        let ty = if self.matches(TokenKind::Colon) {
            Some(self.ty()?)
        } else {
            None
        };
        let value = if self.matches(TokenKind::Equal) {
            Some(self.expr()?)
        } else {
            None
        };
        self.matches(TokenKind::Semicolon);
        let span = Span::new(
            self.previous().span.file_id(),
            start,
            self.previous().span.end(),
        );
        Ok(LetStmt {
            mutable,
            pattern: Pat::Ident(name, span),
            ty,
            value,
            span,
        })
    }

    fn expr(&mut self) -> ParseResult<Expr> {
        self.assignment()
    }

    fn assignment(&mut self) -> ParseResult<Expr> {
        let expr = self.logical_or()?;
        if self.matches(TokenKind::Equal) {
            let operator = self.previous().span;
            let value = self.assignment()?;
            let span = join_expr_span(&expr, &value, operator);
            return Ok(Expr::Assign {
                target: Box::new(expr),
                value: Box::new(value),
                span,
            });
        }
        Ok(expr)
    }

    fn logical_or(&mut self) -> ParseResult<Expr> {
        self.left_associative(Self::logical_and, &[(TokenKind::OrOr, BinaryOp::Or)])
    }

    fn logical_and(&mut self) -> ParseResult<Expr> {
        self.left_associative(Self::comparison, &[(TokenKind::AndAnd, BinaryOp::And)])
    }

    fn comparison(&mut self) -> ParseResult<Expr> {
        let mut expr = self.additive()?;
        let operators = [
            (TokenKind::EqualEqual, BinaryOp::Eq),
            (TokenKind::BangEqual, BinaryOp::Ne),
            (TokenKind::Less, BinaryOp::Lt),
            (TokenKind::LessEqual, BinaryOp::Le),
            (TokenKind::Greater, BinaryOp::Gt),
            (TokenKind::GreaterEqual, BinaryOp::Ge),
        ];
        for (kind, op) in operators {
            if self.matches(kind) {
                let right = self.additive()?;
                let span = join_expr_span(&expr, &right, self.previous().span);
                expr = Expr::Binary {
                    op,
                    left: Box::new(expr),
                    right: Box::new(right),
                    span,
                };
                break;
            }
        }
        Ok(expr)
    }

    fn additive(&mut self) -> ParseResult<Expr> {
        self.left_associative(
            Self::multiplicative,
            &[
                (TokenKind::Plus, BinaryOp::Add),
                (TokenKind::Minus, BinaryOp::Sub),
            ],
        )
    }

    fn multiplicative(&mut self) -> ParseResult<Expr> {
        self.left_associative(
            Self::unary,
            &[
                (TokenKind::Star, BinaryOp::Mul),
                (TokenKind::Slash, BinaryOp::Div),
                (TokenKind::Percent, BinaryOp::Rem),
            ],
        )
    }

    fn left_associative(
        &mut self,
        next: fn(&mut Self) -> ParseResult<Expr>,
        operators: &[(TokenKind, BinaryOp)],
    ) -> ParseResult<Expr> {
        let mut expr = next(self)?;
        while let Some((_, op)) = operators.iter().find(|(kind, _)| self.check(*kind)) {
            let op = *op;
            self.advance();
            let right = next(self)?;
            let span = join_expr_span(&expr, &right, self.previous().span);
            expr = Expr::Binary {
                op,
                left: Box::new(expr),
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn unary(&mut self) -> ParseResult<Expr> {
        if self.matches(TokenKind::Bang) {
            let operator = self.previous().span;
            let expr = self.unary()?;
            let span = Span::new(operator.file_id(), operator.start(), expr_span(&expr).end());
            return Ok(Expr::Unary {
                op: UnaryOp::Not,
                expr: Box::new(expr),
                span,
            });
        }
        if self.matches(TokenKind::Minus) {
            let operator = self.previous().span;
            let expr = self.unary()?;
            let span = Span::new(operator.file_id(), operator.start(), expr_span(&expr).end());
            return Ok(Expr::Unary {
                op: UnaryOp::Neg,
                expr: Box::new(expr),
                span,
            });
        }
        self.call()
    }

    fn call(&mut self) -> ParseResult<Expr> {
        let mut expr = self.primary()?;
        loop {
            if !self.matches(TokenKind::LeftParen) {
                break;
            }
            let mut args = Vec::new();
            if !self.check(TokenKind::RightParen) {
                loop {
                    args.push(self.expr()?);
                    if !self.matches(TokenKind::Comma) {
                        break;
                    }
                }
            }
            let right = self.consume(TokenKind::RightParen, "expected `)` after arguments")?;
            let span = Span::new(
                expr_span(&expr).file_id(),
                expr_span(&expr).start(),
                right.span.end(),
            );
            expr = Expr::Call {
                callee: Box::new(expr),
                args,
                span,
            };
        }
        Ok(expr)
    }

    fn primary(&mut self) -> ParseResult<Expr> {
        if self.matches(TokenKind::Integer) {
            let token = self.previous();
            let value = token.lexeme.replace('_', "").parse().unwrap_or(0);
            return Ok(Expr::Literal(Literal::Int(value), token.span));
        }
        if self.matches(TokenKind::Float) {
            let token = self.previous();
            let value = token.lexeme.replace('_', "").parse().unwrap_or(0.0);
            return Ok(Expr::Literal(Literal::Float(value), token.span));
        }
        if self.matches(TokenKind::String) {
            let token = self.previous();
            return Ok(Expr::Literal(
                Literal::String(decode_string(&token.lexeme)),
                token.span,
            ));
        }
        if self.matches(TokenKind::True) {
            return Ok(Expr::Literal(Literal::Bool(true), self.previous().span));
        }
        if self.matches(TokenKind::False) {
            return Ok(Expr::Literal(Literal::Bool(false), self.previous().span));
        }
        if self.matches(TokenKind::Identifier) {
            let token = self.previous();
            return Ok(Expr::Ident(token.lexeme.clone(), token.span));
        }
        if self.matches(TokenKind::If) {
            return self.if_expr();
        }
        if self.matches(TokenKind::While) {
            return self.while_expr();
        }
        if self.matches(TokenKind::Loop) {
            let start = self.previous().span;
            let body = self.block()?;
            let span = Span::new(start.file_id(), start.start(), body.span.end());
            return Ok(Expr::Loop { body, span });
        }
        if self.matches(TokenKind::Return) {
            let start = self.previous().span;
            let value = if self.check(TokenKind::RightBrace) || self.check(TokenKind::Semicolon) {
                None
            } else {
                Some(Box::new(self.expr()?))
            };
            let end = value
                .as_ref()
                .map_or(start.end(), |expr| expr_span(expr).end());
            return Ok(Expr::Return(
                value,
                Span::new(start.file_id(), start.start(), end),
            ));
        }
        if self.matches(TokenKind::Break) {
            let token = self.previous();
            return Ok(Expr::Break(None, token.span));
        }
        if self.matches(TokenKind::Continue) {
            let token = self.previous();
            return Ok(Expr::Continue(token.span));
        }
        if self.matches(TokenKind::LeftParen) {
            let expr = self.expr()?;
            self.consume(TokenKind::RightParen, "expected `)` after expression")?;
            return Ok(expr);
        }
        if self.check(TokenKind::LeftBrace) {
            return Ok(Expr::Block(Box::new(self.block()?)));
        }

        self.error_here("PACO-E0111", "expected expression");
        Err(ParseError)
    }

    fn if_expr(&mut self) -> ParseResult<Expr> {
        let start = self.previous().span;
        let condition = self.expr()?;
        let then_branch = self.block()?;
        let else_branch = if self.matches(TokenKind::Else) {
            if self.matches(TokenKind::If) {
                Some(Box::new(self.if_expr()?))
            } else {
                Some(Box::new(Expr::Block(Box::new(self.block()?))))
            }
        } else {
            None
        };
        let end = else_branch
            .as_ref()
            .map_or(then_branch.span.end(), |expr| expr_span(expr).end());
        Ok(Expr::If {
            condition: Box::new(condition),
            then_branch,
            else_branch,
            span: Span::new(start.file_id(), start.start(), end),
        })
    }

    fn while_expr(&mut self) -> ParseResult<Expr> {
        let start = self.previous().span;
        let condition = self.expr()?;
        let body = self.block()?;
        let span = Span::new(start.file_id(), start.start(), body.span.end());
        Ok(Expr::While {
            condition: Box::new(condition),
            body,
            span,
        })
    }

    fn consume(&mut self, kind: TokenKind, message: &str) -> ParseResult<&Token> {
        if self.check(kind) {
            return Ok(self.advance());
        }
        self.error_here("PACO-E0112", message);
        Err(ParseError)
    }

    fn consume_identifier(&mut self, message: &str) -> ParseResult<String> {
        let token = self.consume(TokenKind::Identifier, message)?;
        Ok(token.lexeme.clone())
    }

    fn matches(&mut self, kind: TokenKind) -> bool {
        if !self.check(kind) {
            return false;
        }
        self.advance();
        true
    }

    fn check(&self, kind: TokenKind) -> bool {
        self.peek().kind == kind
    }

    fn advance(&mut self) -> &Token {
        if !self.check(TokenKind::Eof) {
            self.current += 1;
        }
        self.previous()
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.current]
    }

    fn previous(&self) -> &Token {
        &self.tokens[self.current.saturating_sub(1)]
    }

    fn error_here(&mut self, code: &str, message: impl Into<String>) {
        self.reporter
            .push(Diagnostic::error(code, self.peek().span, message));
    }

    fn synchronize_item(&mut self) {
        while !self.check(TokenKind::Eof) && !self.check(TokenKind::Fn) {
            self.advance();
        }
    }
}

pub fn expr_span(expr: &Expr) -> Span {
    match expr {
        Expr::Literal(_, span)
        | Expr::Ident(_, span)
        | Expr::Return(_, span)
        | Expr::Break(_, span)
        | Expr::Continue(span)
        | Expr::Yield(_, span) => *span,
        Expr::Block(block) => block.span,
        Expr::If { span, .. }
        | Expr::Loop { span, .. }
        | Expr::While { span, .. }
        | Expr::Match { span, .. }
        | Expr::Call { span, .. }
        | Expr::MethodCall { span, .. }
        | Expr::AssociatedCall { span, .. }
        | Expr::Binary { span, .. }
        | Expr::Unary { span, .. }
        | Expr::Assign { span, .. }
        | Expr::Field { span, .. }
        | Expr::Index { span, .. }
        | Expr::Spawn { span, .. }
        | Expr::Select { span, .. }
        | Expr::Comptime { span, .. }
        | Expr::StructLiteral { span, .. }
        | Expr::Borrow { span, .. } => *span,
    }
}

fn join_expr_span(left: &Expr, right: &Expr, fallback: Span) -> Span {
    let left = expr_span(left);
    let right = expr_span(right);
    Span::new(
        left.file_id(),
        left.start().min(fallback.start()),
        right.end().max(fallback.end()),
    )
}

fn decode_string(source: &str) -> String {
    let inner = source
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(source);
    let mut output = String::new();
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => output.push('\n'),
            Some('t') => output.push('\t'),
            Some('r') => output.push('\r'),
            Some('"') => output.push('"'),
            Some('\\') => output.push('\\'),
            Some(other) => output.push(other),
            None => output.push('\\'),
        }
    }
    output
}
