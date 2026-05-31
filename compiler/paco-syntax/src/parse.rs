//! Recursive-descent parser for the initial executable Paco subset.

use paco_diag::{Diagnostic, Reporter};
use paco_span::Span;

use crate::{
    ast::{
        BinaryOp, Block, EnumDecl, EnumVariant, Expr, FieldDecl, FnDecl, Item, LetStmt, Literal,
        MatchArm, MethodsBlock, Module, Param, Pat, Stmt, StructDecl, Ty, UnaryOp, UseDecl,
        VariantFields,
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
        allow_struct_literals: true,
    }
    .parse_module()
}

struct Parser<'a, 'reporter> {
    tokens: &'a [Token],
    reporter: &'reporter mut Reporter,
    current: usize,
    allow_struct_literals: bool,
}

impl Parser<'_, '_> {
    fn parse_module(&mut self) -> ParseResult<Module> {
        let start = self.peek().span.start();
        let mut items = Vec::new();

        while !self.check(TokenKind::Eof) {
            if self.matches(TokenKind::Fn) {
                items.push(Item::Fn(self.function_decl()?));
            } else if self.matches(TokenKind::Struct) {
                items.push(Item::Struct(self.struct_decl()?));
            } else if self.matches(TokenKind::Enum) {
                items.push(Item::Enum(self.enum_decl()?));
            } else if self.matches(TokenKind::Methods) {
                items.push(Item::Methods(self.methods_block()?));
            } else if self.matches(TokenKind::Use) {
                items.push(Item::Use(self.use_decl()?));
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
        let generics = self.generic_params()?;
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
            generics,
            params,
            return_ty,
            body,
            span,
        })
    }

    fn struct_decl(&mut self) -> ParseResult<StructDecl> {
        let start = self.previous().span.start();
        let name = self.consume_identifier("expected struct name")?;
        let generics = self.generic_params()?;
        self.consume(TokenKind::LeftBrace, "expected `{` before struct body")?;
        let mut fields = Vec::new();
        let mut methods = Vec::new();

        while !self.check(TokenKind::RightBrace) && !self.check(TokenKind::Eof) {
            if self.matches(TokenKind::Fn) {
                methods.push(self.function_decl()?);
            } else {
                fields.push(self.field_decl()?);
            }
            self.matches(TokenKind::Comma);
        }

        let right = self.consume(TokenKind::RightBrace, "expected `}` after struct body")?;
        Ok(StructDecl {
            name,
            generics,
            fields,
            methods,
            span: Span::new(right.span.file_id(), start, right.span.end()),
        })
    }

    fn enum_decl(&mut self) -> ParseResult<EnumDecl> {
        let start = self.previous().span.start();
        let name = self.consume_identifier("expected enum name")?;
        let generics = self.generic_params()?;
        self.consume(TokenKind::LeftBrace, "expected `{` before enum body")?;
        let mut variants = Vec::new();
        let mut methods = Vec::new();

        while !self.check(TokenKind::RightBrace) && !self.check(TokenKind::Eof) {
            if self.matches(TokenKind::Fn) {
                methods.push(self.function_decl()?);
            } else {
                variants.push(self.enum_variant()?);
            }
            self.matches(TokenKind::Comma);
        }

        let right = self.consume(TokenKind::RightBrace, "expected `}` after enum body")?;
        Ok(EnumDecl {
            name,
            generics,
            variants,
            methods,
            span: Span::new(right.span.file_id(), start, right.span.end()),
        })
    }

    fn methods_block(&mut self) -> ParseResult<MethodsBlock> {
        let start = self.previous().span.start();
        let generics = self.generic_params()?;
        let target = self.ty()?;
        self.consume(TokenKind::LeftBrace, "expected `{` before methods body")?;
        let mut methods = Vec::new();

        while !self.check(TokenKind::RightBrace) && !self.check(TokenKind::Eof) {
            self.consume(TokenKind::Fn, "expected method declaration")?;
            methods.push(self.function_decl()?);
        }

        let right = self.consume(TokenKind::RightBrace, "expected `}` after methods body")?;
        Ok(MethodsBlock {
            generics,
            target,
            methods,
            span: Span::new(right.span.file_id(), start, right.span.end()),
        })
    }

    fn use_decl(&mut self) -> ParseResult<UseDecl> {
        let start = self.previous().span.start();
        let path = self.path()?;
        let end = self.previous().span.end();
        self.matches(TokenKind::Semicolon);
        Ok(UseDecl {
            path,
            span: Span::new(self.previous().span.file_id(), start, end),
        })
    }

    fn field_decl(&mut self) -> ParseResult<FieldDecl> {
        let start = self.peek().span.start();
        let name = self.consume_identifier("expected field name")?;
        self.consume(TokenKind::Colon, "expected `:` after field name")?;
        let ty = self.ty()?;
        Ok(FieldDecl {
            name,
            ty,
            span: Span::new(
                self.previous().span.file_id(),
                start,
                self.previous().span.end(),
            ),
        })
    }

    fn enum_variant(&mut self) -> ParseResult<EnumVariant> {
        let start = self.peek().span.start();
        let name = self.consume_identifier("expected enum variant name")?;
        let fields = if self.matches(TokenKind::LeftParen) {
            let mut tys = Vec::new();
            if !self.check(TokenKind::RightParen) {
                loop {
                    tys.push(self.ty()?);
                    if !self.matches(TokenKind::Comma) {
                        break;
                    }
                }
            }
            self.consume(
                TokenKind::RightParen,
                "expected `)` after enum variant fields",
            )?;
            VariantFields::Tuple(tys)
        } else if self.matches(TokenKind::LeftBrace) {
            let mut fields = Vec::new();
            if !self.check(TokenKind::RightBrace) {
                loop {
                    fields.push(self.field_decl()?);
                    if !self.matches(TokenKind::Comma) {
                        break;
                    }
                }
            }
            self.consume(
                TokenKind::RightBrace,
                "expected `}` after enum variant fields",
            )?;
            VariantFields::Struct(fields)
        } else {
            VariantFields::Unit
        };
        Ok(EnumVariant {
            name,
            fields,
            span: Span::new(
                self.previous().span.file_id(),
                start,
                self.previous().span.end(),
            ),
        })
    }

    fn generic_params(&mut self) -> ParseResult<Vec<String>> {
        let mut params = Vec::new();
        if !self.matches(TokenKind::Less) {
            return Ok(params);
        }
        loop {
            params.push(self.consume_generic_parameter()?);
            if !self.matches(TokenKind::Comma) {
                break;
            }
        }
        self.consume(TokenKind::Greater, "expected `>` after generic parameters")?;
        Ok(params)
    }

    fn parameter_list(&mut self) -> ParseResult<Vec<Param>> {
        let mut params = Vec::new();
        if self.check(TokenKind::RightParen) {
            return Ok(params);
        }

        loop {
            let start = self.peek().span.start();
            let name = self.consume_identifier("expected parameter name")?;
            let ty = if name == "self" && self.matches(TokenKind::Ampersand) {
                let mutable = self.matches(TokenKind::Mut);
                Ty::Borrow {
                    mutable,
                    lifetime: None,
                    ty: Box::new(Ty::Path(vec!["Self".to_string()], self.previous().span)),
                    span: Span::new(
                        self.previous().span.file_id(),
                        start,
                        self.previous().span.end(),
                    ),
                }
            } else if name == "self" {
                Ty::Path(vec!["Self".to_string()], self.previous().span)
            } else {
                self.consume(TokenKind::Colon, "expected `:` after parameter name")?;
                self.ty()?
            };
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
        let start = self.peek().span.start();
        if self.matches(TokenKind::Ampersand) {
            let ampersand = self.previous().span;
            let lifetime = if self.matches(TokenKind::Lifetime) {
                Some(self.previous().lexeme.trim_start_matches('\'').to_string())
            } else {
                None
            };
            let mutable = self.matches(TokenKind::Mut);
            let ty = self.ty()?;
            let span = Span::new(ampersand.file_id(), start, ty_span(&ty).end());
            return Ok(Ty::Borrow {
                mutable,
                lifetime,
                ty: Box::new(ty),
                span,
            });
        }
        let path = self.path()?;
        if self.matches(TokenKind::Less) {
            let mut args = Vec::new();
            if !self.check(TokenKind::Greater) {
                loop {
                    args.push(self.ty()?);
                    if !self.matches(TokenKind::Comma) {
                        break;
                    }
                }
            }
            let end = self
                .consume(
                    TokenKind::Greater,
                    "expected `>` after generic type arguments",
                )?
                .span
                .end();
            return Ok(Ty::Generic {
                path,
                args,
                span: Span::new(self.previous().span.file_id(), start, end),
            });
        }
        Ok(Ty::Path(path, self.previous().span))
    }

    fn path(&mut self) -> ParseResult<Vec<String>> {
        let mut path = vec![self.consume_identifier("expected path segment")?];
        while self.matches(TokenKind::ColonColon) {
            path.push(self.consume_identifier("expected path segment after `::`")?);
        }
        Ok(path)
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
        if self.matches(TokenKind::Ampersand) {
            let operator = self.previous().span;
            let mutable = self.matches(TokenKind::Mut);
            let expr = self.unary()?;
            let span = Span::new(operator.file_id(), operator.start(), expr_span(&expr).end());
            return Ok(Expr::Borrow {
                mutable,
                expr: Box::new(expr),
                span,
            });
        }
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
            if self.matches(TokenKind::LeftParen) {
                let args = self.argument_list()?;
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
            } else if self.matches(TokenKind::Dot) {
                let method_or_field = self.consume_identifier("expected field or method name")?;
                if self.matches(TokenKind::LeftParen) {
                    let args = self.argument_list()?;
                    let right =
                        self.consume(TokenKind::RightParen, "expected `)` after arguments")?;
                    let span = Span::new(
                        expr_span(&expr).file_id(),
                        expr_span(&expr).start(),
                        right.span.end(),
                    );
                    expr = Expr::MethodCall {
                        receiver: Box::new(expr),
                        method: method_or_field,
                        args,
                        span,
                    };
                } else {
                    let span = Span::new(
                        expr_span(&expr).file_id(),
                        expr_span(&expr).start(),
                        self.previous().span.end(),
                    );
                    expr = Expr::Field {
                        base: Box::new(expr),
                        field: method_or_field,
                        span,
                    }
                };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn argument_list(&mut self) -> ParseResult<Vec<Expr>> {
        let mut args = Vec::new();
        if self.check(TokenKind::RightParen) {
            return Ok(args);
        }
        loop {
            args.push(self.expr()?);
            if !self.matches(TokenKind::Comma) {
                break;
            }
        }
        Ok(args)
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
        if self.matches(TokenKind::Match) {
            return self.match_expr();
        }
        if self.matches(TokenKind::Identifier) {
            let token = self.previous().clone();
            let ty = self.expr_type_path(&token)?;
            if self.allow_struct_literals && self.matches(TokenKind::LeftBrace) {
                return self.struct_literal(ty);
            }
            if self.matches(TokenKind::ColonColon) {
                let function = self.consume_identifier("expected associated item name")?;
                let mut args = Vec::new();
                let mut end = self.previous().span.end();
                if self.matches(TokenKind::LeftParen) {
                    args = self.argument_list()?;
                    end = self
                        .consume(TokenKind::RightParen, "expected `)` after arguments")?
                        .span
                        .end();
                }
                let start = ty_span(&ty).start();
                return Ok(Expr::AssociatedCall {
                    ty,
                    function,
                    args,
                    span: Span::new(token.span.file_id(), start, end),
                });
            }
            if let Ty::Path(path, span) = ty
                && path.len() == 1
            {
                return Ok(Expr::Ident(path[0].clone(), span));
            }
            self.error_here("PACO-E0111", "expected expression");
            return Err(ParseError);
        }
        if self.matches(TokenKind::If) {
            return self.if_expr();
        }
        if self.matches(TokenKind::While) {
            return self.while_expr();
        }
        if self.matches(TokenKind::For) {
            return self.for_expr();
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

    fn match_expr(&mut self) -> ParseResult<Expr> {
        let start = self.previous().span;
        let scrutinee = self.expr_without_struct_literals()?;
        self.consume(TokenKind::LeftBrace, "expected `{` before match arms")?;
        let mut arms = Vec::new();
        while !self.check(TokenKind::RightBrace) && !self.check(TokenKind::Eof) {
            arms.push(self.match_arm()?);
            self.matches(TokenKind::Comma);
        }
        let right = self.consume(TokenKind::RightBrace, "expected `}` after match arms")?;
        Ok(Expr::Match {
            scrutinee: Box::new(scrutinee),
            arms,
            span: Span::new(start.file_id(), start.start(), right.span.end()),
        })
    }

    fn match_arm(&mut self) -> ParseResult<MatchArm> {
        let start = self.peek().span.start();
        let pattern = self.pattern()?;
        let guard = if self.matches(TokenKind::If) {
            Some(self.expr_without_struct_literals()?)
        } else {
            None
        };
        self.consume(TokenKind::FatArrow, "expected `=>` after match arm pattern")?;
        let body = self.expr()?;
        let end = expr_span(&body).end();
        Ok(MatchArm {
            pattern,
            guard,
            body,
            span: Span::new(self.previous().span.file_id(), start, end),
        })
    }

    fn pattern(&mut self) -> ParseResult<Pat> {
        let mut patterns = vec![self.binding_pattern()?];
        while self.matches(TokenKind::Pipe) {
            patterns.push(self.binding_pattern()?);
        }
        if patterns.len() == 1 {
            Ok(patterns.remove(0))
        } else {
            let start = pat_span(&patterns[0]).start();
            let end = pat_span(patterns.last().unwrap()).end();
            Ok(Pat::Or(
                patterns,
                Span::new(self.previous().span.file_id(), start, end),
            ))
        }
    }

    fn binding_pattern(&mut self) -> ParseResult<Pat> {
        if self.check(TokenKind::Identifier)
            && self
                .tokens
                .get(self.current + 1)
                .is_some_and(|token| token.kind == TokenKind::At)
        {
            let token = self.advance().clone();
            self.consume(TokenKind::At, "expected `@` after binding name")?;
            let pattern = self.binding_pattern()?;
            let span = Span::new(
                token.span.file_id(),
                token.span.start(),
                pat_span(&pattern).end(),
            );
            return Ok(Pat::Binding {
                name: token.lexeme,
                pattern: Box::new(pattern),
                span,
            });
        }
        self.range_pattern()
    }

    fn range_pattern(&mut self) -> ParseResult<Pat> {
        let start = self.pattern_atom()?;
        if self.matches(TokenKind::DotDot) || self.matches(TokenKind::DotDotEqual) {
            let inclusive = self.previous().kind == TokenKind::DotDotEqual;
            let end = self.pattern_atom()?;
            let span = Span::new(
                pat_span(&start).file_id(),
                pat_span(&start).start(),
                pat_span(&end).end(),
            );
            return Ok(Pat::Range {
                start: Box::new(start),
                end: Box::new(end),
                inclusive,
                span,
            });
        }
        Ok(start)
    }

    fn pattern_atom(&mut self) -> ParseResult<Pat> {
        if self.matches(TokenKind::Underscore) {
            return Ok(Pat::Wildcard(self.previous().span));
        }
        if self.matches(TokenKind::Integer) {
            let token = self.previous();
            let value = token.lexeme.replace('_', "").parse().unwrap_or(0);
            return Ok(Pat::Literal(Literal::Int(value), token.span));
        }
        if self.matches(TokenKind::String) {
            let token = self.previous();
            return Ok(Pat::Literal(
                Literal::String(decode_string(&token.lexeme)),
                token.span,
            ));
        }
        if self.matches(TokenKind::True) {
            return Ok(Pat::Literal(Literal::Bool(true), self.previous().span));
        }
        if self.matches(TokenKind::False) {
            return Ok(Pat::Literal(Literal::Bool(false), self.previous().span));
        }
        if self.check(TokenKind::Identifier) {
            let start = self.peek().span.start();
            let path = self.path()?;
            let mut fields = Vec::new();
            if self.matches(TokenKind::LeftParen) {
                if !self.check(TokenKind::RightParen) {
                    loop {
                        fields.push(self.pattern()?);
                        if !self.matches(TokenKind::Comma) {
                            break;
                        }
                    }
                }
                let right =
                    self.consume(TokenKind::RightParen, "expected `)` after pattern fields")?;
                return Ok(Pat::Enum {
                    path,
                    fields,
                    span: Span::new(right.span.file_id(), start, right.span.end()),
                });
            }
            let span = Span::new(
                self.previous().span.file_id(),
                start,
                self.previous().span.end(),
            );
            if path.len() == 1 {
                return Ok(Pat::Ident(path[0].clone(), span));
            }
            return Ok(Pat::Enum { path, fields, span });
        }
        if self.matches(TokenKind::LeftParen) {
            let left = self.previous().span;
            let mut fields = Vec::new();
            if !self.check(TokenKind::RightParen) {
                loop {
                    fields.push(self.pattern()?);
                    if !self.matches(TokenKind::Comma) {
                        break;
                    }
                }
            }
            let right = self.consume(TokenKind::RightParen, "expected `)` after tuple pattern")?;
            return Ok(Pat::Tuple(
                fields,
                Span::new(left.file_id(), left.start(), right.span.end()),
            ));
        }
        self.error_here("PACO-E0113", "expected pattern");
        Err(ParseError)
    }

    fn expr_type_path(&mut self, token: &Token) -> ParseResult<Ty> {
        let path = vec![token.lexeme.clone()];
        if self.starts_generic_type_application() {
            self.consume(
                TokenKind::Less,
                "expected `<` before generic type arguments",
            )?;
            let mut args = Vec::new();
            if !self.check(TokenKind::Greater) {
                loop {
                    args.push(self.ty()?);
                    if !self.matches(TokenKind::Comma) {
                        break;
                    }
                }
            }
            let end = self
                .consume(
                    TokenKind::Greater,
                    "expected `>` after generic type arguments",
                )?
                .span
                .end();
            Ok(Ty::Generic {
                path,
                args,
                span: Span::new(token.span.file_id(), token.span.start(), end),
            })
        } else {
            Ok(Ty::Path(path, token.span))
        }
    }

    fn starts_generic_type_application(&self) -> bool {
        if !self.check(TokenKind::Less) {
            return false;
        }
        let mut depth = 0usize;
        for index in self.current..self.tokens.len() {
            let token = &self.tokens[index];
            match token.kind {
                TokenKind::Less => depth += 1,
                TokenKind::Greater => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        let Some(next) = self.tokens.get(index + 1) else {
                            return false;
                        };
                        return matches!(next.kind, TokenKind::LeftBrace | TokenKind::ColonColon);
                    }
                }
                TokenKind::Eof | TokenKind::LeftBrace | TokenKind::RightBrace if depth == 0 => {
                    return false;
                }
                _ => {}
            }
        }
        false
    }

    fn struct_literal(&mut self, ty: Ty) -> ParseResult<Expr> {
        let start = ty_span(&ty).start();
        let mut fields = Vec::new();
        if !self.check(TokenKind::RightBrace) {
            loop {
                let name = self.consume_identifier("expected struct field name")?;
                self.consume(TokenKind::Colon, "expected `:` after struct field name")?;
                let value = self.expr()?;
                fields.push((name, value));
                if !self.matches(TokenKind::Comma) {
                    break;
                }
            }
        }
        let right = self.consume(TokenKind::RightBrace, "expected `}` after struct literal")?;
        Ok(Expr::StructLiteral {
            ty,
            fields,
            span: Span::new(right.span.file_id(), start, right.span.end()),
        })
    }

    fn if_expr(&mut self) -> ParseResult<Expr> {
        let start = self.previous().span;
        if self.matches(TokenKind::Let) {
            return self.if_let_expr(start);
        }
        let condition = self.expr_without_struct_literals()?;
        let then_branch = self.block()?;
        let else_branch = self.else_branch()?;
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

    fn if_let_expr(&mut self, start: Span) -> ParseResult<Expr> {
        let pattern = self.pattern()?;
        self.consume(TokenKind::Equal, "expected `=` after if let pattern")?;
        let scrutinee = self.expr_without_struct_literals()?;
        let then_branch = self.block()?;
        let else_branch = self.else_branch()?;
        let then_body = Expr::Block(Box::new(then_branch));
        let else_body = else_branch
            .map(|expr| *expr)
            .unwrap_or_else(|| empty_block_expr(start));
        let pattern_span = pat_span(&pattern);
        let then_end = expr_span(&then_body).end();
        let else_span = expr_span(&else_body);
        let end = else_span.end().max(then_end);

        Ok(Expr::Match {
            scrutinee: Box::new(scrutinee),
            arms: vec![
                MatchArm {
                    pattern,
                    guard: None,
                    body: then_body,
                    span: Span::new(start.file_id(), pattern_span.start(), then_end),
                },
                MatchArm {
                    pattern: Pat::Wildcard(else_span),
                    guard: None,
                    body: else_body,
                    span: else_span,
                },
            ],
            span: Span::new(start.file_id(), start.start(), end),
        })
    }

    fn while_expr(&mut self) -> ParseResult<Expr> {
        let start = self.previous().span;
        if self.matches(TokenKind::Let) {
            return self.while_let_expr(start);
        }
        let condition = self.expr_without_struct_literals()?;
        let body = self.block()?;
        let span = Span::new(start.file_id(), start.start(), body.span.end());
        Ok(Expr::While {
            condition: Box::new(condition),
            body,
            span,
        })
    }

    fn while_let_expr(&mut self, start: Span) -> ParseResult<Expr> {
        let pattern = self.pattern()?;
        self.consume(TokenKind::Equal, "expected `=` after while let pattern")?;
        let scrutinee = self.expr_without_struct_literals()?;
        let body = self.block()?;
        let body_expr = Expr::Block(Box::new(body));
        let pattern_span = pat_span(&pattern);
        let body_end = expr_span(&body_expr).end();
        let fallback_span = Span::new(start.file_id(), body_end, body_end);
        let match_expr = Expr::Match {
            scrutinee: Box::new(scrutinee),
            arms: vec![
                MatchArm {
                    pattern,
                    guard: None,
                    body: body_expr,
                    span: Span::new(start.file_id(), pattern_span.start(), body_end),
                },
                MatchArm {
                    pattern: Pat::Wildcard(fallback_span),
                    guard: None,
                    body: Expr::Break(None, fallback_span),
                    span: fallback_span,
                },
            ],
            span: Span::new(start.file_id(), start.start(), body_end),
        };
        let loop_body = Block {
            stmts: vec![Stmt::Expr(match_expr)],
            tail: None,
            span: Span::new(start.file_id(), start.start(), body_end),
        };
        Ok(Expr::Loop {
            body: loop_body,
            span: Span::new(start.file_id(), start.start(), body_end),
        })
    }

    fn for_expr(&mut self) -> ParseResult<Expr> {
        let start = self.previous().span;
        let name = self.consume(TokenKind::Identifier, "expected loop binding name")?;
        let name = (name.lexeme.clone(), name.span);
        self.consume(TokenKind::In, "expected `in` after loop binding")?;
        let range_start = self.expr_without_struct_literals()?;
        let inclusive = if self.matches(TokenKind::DotDotEqual) {
            true
        } else if self.matches(TokenKind::DotDot) {
            false
        } else {
            self.error_here("PACO-E0112", "expected range operator after loop start");
            return Err(ParseError);
        };
        let range_end = self.expr_without_struct_literals()?;
        let body = self.block()?;
        Ok(for_range_expr(
            start,
            name,
            range_start,
            range_end,
            inclusive,
            body,
        ))
    }

    fn else_branch(&mut self) -> ParseResult<Option<Box<Expr>>> {
        if !self.matches(TokenKind::Else) {
            return Ok(None);
        }
        if self.matches(TokenKind::If) {
            Ok(Some(Box::new(self.if_expr()?)))
        } else {
            Ok(Some(Box::new(Expr::Block(Box::new(self.block()?)))))
        }
    }

    fn expr_without_struct_literals(&mut self) -> ParseResult<Expr> {
        let previous = self.allow_struct_literals;
        self.allow_struct_literals = false;
        let result = self.expr();
        self.allow_struct_literals = previous;
        result
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

    fn consume_generic_parameter(&mut self) -> ParseResult<String> {
        if self.matches(TokenKind::Identifier) || self.matches(TokenKind::Lifetime) {
            return Ok(self.previous().lexeme.trim_start_matches('\'').to_string());
        }
        self.error_here("PACO-E0112", "expected generic parameter name");
        Err(ParseError)
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
        while !self.check(TokenKind::Eof)
            && !matches!(
                self.peek().kind,
                TokenKind::Fn
                    | TokenKind::Struct
                    | TokenKind::Enum
                    | TokenKind::Methods
                    | TokenKind::Use
                    | TokenKind::Trait
            )
        {
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

fn pat_span(pattern: &Pat) -> Span {
    match pattern {
        Pat::Ident(_, span)
        | Pat::Wildcard(span)
        | Pat::Literal(_, span)
        | Pat::Tuple(_, span)
        | Pat::Struct { span, .. }
        | Pat::Enum { span, .. }
        | Pat::Range { span, .. }
        | Pat::Or(_, span)
        | Pat::Binding { span, .. } => *span,
    }
}

fn empty_block_expr(span: Span) -> Expr {
    Expr::Block(Box::new(Block {
        stmts: Vec::new(),
        tail: None,
        span,
    }))
}

fn for_range_expr(
    start: Span,
    name: (String, Span),
    range_start: Expr,
    range_end: Expr,
    inclusive: bool,
    body: Block,
) -> Expr {
    let (name, name_span) = name;
    let cursor_name = format!("$paco_for_cursor_{}", start.start());
    let end = body.span.end();
    let condition = Expr::Binary {
        op: if inclusive {
            BinaryOp::Le
        } else {
            BinaryOp::Lt
        },
        left: Box::new(Expr::Ident(cursor_name.clone(), name_span)),
        right: Box::new(range_end),
        span: Span::new(start.file_id(), name_span.start(), end),
    };
    let increment = for_increment_expr(
        &cursor_name,
        name_span,
        Span::new(start.file_id(), start.start(), end),
    );
    let mut body = body;
    rewrite_continue_in_block(&mut body, &increment);
    let body = prepend_statement(
        body,
        Stmt::Let(LetStmt {
            mutable: false,
            pattern: Pat::Ident(name, name_span),
            ty: None,
            value: Some(Expr::Ident(cursor_name.clone(), name_span)),
            span: Span::new(start.file_id(), start.start(), end),
        }),
    );
    let then_branch = append_statement(body, Stmt::Expr(increment));
    let break_span = Span::new(start.file_id(), end, end);
    let if_expr = Expr::If {
        condition: Box::new(condition),
        then_branch,
        else_branch: Some(Box::new(Expr::Break(None, break_span))),
        span: Span::new(start.file_id(), start.start(), end),
    };
    let loop_body = Block {
        stmts: vec![Stmt::Expr(if_expr)],
        tail: None,
        span: Span::new(start.file_id(), start.start(), end),
    };
    let loop_expr = Expr::Loop {
        body: loop_body,
        span: Span::new(start.file_id(), start.start(), end),
    };
    Expr::Block(Box::new(Block {
        stmts: vec![
            Stmt::Let(LetStmt {
                mutable: true,
                pattern: Pat::Ident(cursor_name, name_span),
                ty: None,
                value: Some(range_start),
                span: Span::new(start.file_id(), start.start(), end),
            }),
            Stmt::Expr(loop_expr),
        ],
        tail: None,
        span: Span::new(start.file_id(), start.start(), end),
    }))
}

fn for_increment_expr(name: &str, name_span: Span, span: Span) -> Expr {
    Expr::Assign {
        target: Box::new(Expr::Ident(name.to_string(), name_span)),
        value: Box::new(Expr::Binary {
            op: BinaryOp::Add,
            left: Box::new(Expr::Ident(name.to_string(), name_span)),
            right: Box::new(Expr::Literal(Literal::Int(1), name_span)),
            span,
        }),
        span,
    }
}

fn rewrite_continue_in_block(block: &mut Block, increment: &Expr) {
    for statement in &mut block.stmts {
        match statement {
            Stmt::Expr(expr) => rewrite_continue_in_expr(expr, increment),
            Stmt::Let(statement) => {
                if let Some(value) = &mut statement.value {
                    rewrite_continue_in_expr(value, increment);
                }
            }
            Stmt::Item(_) => {}
        }
    }
    if let Some(tail) = &mut block.tail {
        rewrite_continue_in_expr(tail, increment);
    }
}

fn rewrite_continue_in_expr(expr: &mut Expr, increment: &Expr) {
    match expr {
        Expr::Continue(span) => {
            *expr = Expr::Block(Box::new(Block {
                stmts: vec![
                    Stmt::Expr(increment.clone()),
                    Stmt::Expr(Expr::Continue(*span)),
                ],
                tail: None,
                span: *span,
            }));
        }
        Expr::Block(block) => rewrite_continue_in_block(block, increment),
        Expr::If {
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_continue_in_block(then_branch, increment);
            if let Some(else_branch) = else_branch {
                rewrite_continue_in_expr(else_branch, increment);
            }
        }
        Expr::Match { arms, .. } => {
            for arm in arms {
                rewrite_continue_in_expr(&mut arm.body, increment);
            }
        }
        Expr::Assign { value, .. }
        | Expr::Return(Some(value), _)
        | Expr::Break(Some(value), _)
        | Expr::Yield(value, _)
        | Expr::Borrow { expr: value, .. } => rewrite_continue_in_expr(value, increment),
        Expr::Call { args, .. }
        | Expr::MethodCall { args, .. }
        | Expr::AssociatedCall { args, .. } => {
            for arg in args {
                rewrite_continue_in_expr(arg, increment);
            }
        }
        Expr::StructLiteral { fields, .. } => {
            for (_, value) in fields {
                rewrite_continue_in_expr(value, increment);
            }
        }
        Expr::Binary { left, right, .. }
        | Expr::Index {
            base: left,
            index: right,
            ..
        } => {
            rewrite_continue_in_expr(left, increment);
            rewrite_continue_in_expr(right, increment);
        }
        Expr::Unary { expr, .. } | Expr::Field { base: expr, .. } => {
            rewrite_continue_in_expr(expr, increment);
        }
        Expr::Loop { .. }
        | Expr::While { .. }
        | Expr::Literal(_, _)
        | Expr::Ident(_, _)
        | Expr::Return(None, _)
        | Expr::Break(None, _)
        | Expr::Spawn { .. }
        | Expr::Select { .. }
        | Expr::Comptime { .. } => {}
    }
}

fn prepend_statement(mut block: Block, statement: Stmt) -> Block {
    block.stmts.insert(0, statement);
    block
}

fn append_statement(mut block: Block, statement: Stmt) -> Block {
    if let Some(tail) = block.tail.take() {
        block.stmts.push(Stmt::Expr(*tail));
    }
    block.stmts.push(statement);
    block
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

fn ty_span(ty: &Ty) -> Span {
    match ty {
        Ty::Path(_, span)
        | Ty::Generic { span, .. }
        | Ty::Tuple(_, span)
        | Ty::Slice(_, span)
        | Ty::Dyn { span, .. }
        | Ty::Fn { span, .. }
        | Ty::Infer(span)
        | Ty::Never(span)
        | Ty::Borrow { span, .. } => *span,
    }
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
