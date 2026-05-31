//! Recursive-descent parser for the initial executable Paco subset.

use paco_diag::{Diagnostic, Reporter};
use paco_span::Span;

use crate::{
    ast::{
        BinaryOp, Block, EnumDecl, EnumVariant, Expr, FieldDecl, FnDecl, Item, LetStmt, Literal,
        MethodsBlock, Module, Param, Pat, Stmt, StructDecl, Ty, UnaryOp, UseDecl, VariantFields,
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
            params.push(self.consume_identifier("expected generic parameter name")?);
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
        let condition = self.expr_without_struct_literals()?;
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
        let condition = self.expr_without_struct_literals()?;
        let body = self.block()?;
        let span = Span::new(start.file_id(), start.start(), body.span.end());
        Ok(Expr::While {
            condition: Box::new(condition),
            body,
            span,
        })
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
