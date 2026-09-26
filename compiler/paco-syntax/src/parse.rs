//! Recursive-descent parser for the initial executable Paco subset.

use paco_diag::{Diagnostic, Reporter, Suggestion};
use paco_span::Span;

use crate::{
    ast::{
        AssocTypeDecl, Attribute, AttributeArg, BinaryOp, Block, ClosureParam, ConstDecl, EnumDecl, EnumVariant,
        Expr, ExternBlock, FieldDecl, FnDecl, FnSignature, GenericParam, GenericParamKind, Item, LetStmt, Literal, MatchArm,
        MethodsBlock, Module, Param, Pat, QuoteBody, SelectArm, Stmt, StructDecl, TraitDecl, Ty,
        UnaryOp, UseDecl, UsePathKind, VariantFields,
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
        allow_existentials: false,
    }
    .parse_module()
}

struct Parser<'a, 'reporter> {
    tokens: &'a [Token],
    reporter: &'reporter mut Reporter,
    current: usize,
    allow_struct_literals: bool,
    /// Inside a return or field type, where `?b` may name a dimension.
    allow_existentials: bool,
}

impl Parser<'_, '_> {
    fn parse_module(&mut self) -> ParseResult<Module> {
        let start = self.peek().span.start();
        let name = self.module_decl()?;
        let mut items = Vec::new();

        while !self.check(TokenKind::Eof) {
            if self.check(TokenKind::Module) {
                self.error_here(
                    "PACO-E0110",
                    "`module` declaration must be the first thing in the file",
                );
                self.advance();
                continue;
            }
            let attrs = self.parse_outer_attributes()?;
            let is_pub = self.matches(TokenKind::Pub);
            if self.check(TokenKind::Comptime)
                || self.check(TokenKind::Unsafe)
                || self.check(TokenKind::Iter)
                || self.check(TokenKind::Extern)
            {
                let (is_comptime, is_unsafe, is_iter, extern_abi) = self.function_modifiers()?;
                if self.matches(TokenKind::Fn) {
                    let mut decl = self.function_decl(is_pub, is_unsafe, is_iter, extern_abi)?;
                    decl.attrs = attrs;
                    decl.is_comptime = is_comptime;
                    items.push(Item::Fn(decl));
                } else if let Some(abi) = extern_abi {
                    items.push(Item::Extern(self.extern_block(abi)?));
                } else {
                    self.error_here("PACO-E0110", "expected `fn` after `unsafe`");
                    self.synchronize_item();
                }
            } else if self.matches(TokenKind::Fn) {
                let mut decl = self.function_decl(is_pub, false, false, None)?;
                decl.attrs = attrs;
                items.push(Item::Fn(decl));
            } else if self.matches(TokenKind::Struct) {
                let mut decl = self.struct_decl(is_pub)?;
                decl.attrs = attrs;
                items.push(Item::Struct(decl));
            } else if self.matches(TokenKind::Enum) {
                let mut decl = self.enum_decl(is_pub)?;
                decl.attrs = attrs;
                items.push(Item::Enum(decl));
            } else if self.matches(TokenKind::Methods) {
                items.push(Item::Methods(self.methods_block()?));
            } else if self.matches(TokenKind::Use) {
                items.push(Item::Use(self.use_decl()?));
            } else if self.matches(TokenKind::Const) {
                items.push(Item::Const(self.const_decl(is_pub)?));
            } else if self.matches(TokenKind::Trait) {
                let mut decl = self.trait_decl(is_pub)?;
                decl.attrs = attrs;
                items.push(Item::Trait(decl));
            } else {
                self.error_here("PACO-E0110", "expected item declaration");
                self.synchronize_item();
            }
        }

        let end = self.peek().span.end();
        Ok(Module {
            name,
            items,
            span: Span::new(self.peek().span.file_id(), start, end),
        })
    }

    fn module_decl(&mut self) -> ParseResult<Option<crate::ast::ModuleDecl>> {
        if !self.matches(TokenKind::Module) {
            return Ok(None);
        }
        let start = self.previous().span.start();
        let name = self.consume_identifier("expected module name")?;
        self.expect_semicolon();
        Ok(Some(crate::ast::ModuleDecl {
            name,
            span: Span::new(
                self.previous().span.file_id(),
                start,
                self.previous().span.end(),
            ),
        }))
    }

    fn function_decl(
        &mut self,
        is_pub: bool,
        is_unsafe: bool,
        is_iter: bool,
        extern_abi: Option<String>,
    ) -> ParseResult<FnDecl> {
        let start = self.previous().span.start();
        let (name, name_splice) = if self.matches(TokenKind::Hash) {
            self.consume(TokenKind::LeftParen, "expected `(` after `#`")?;
            let inner = self.expr()?;
            self.consume(TokenKind::RightParen, "expected `)` after splice expression")?;
            (String::new(), Some(Box::new(inner)))
        } else {
            (self.consume_identifier("expected function name")?, None)
        };
        let generics = self.generic_params()?;
        self.consume(TokenKind::LeftParen, "expected `(` after function name")?;
        let params = self.parameter_list()?;
        self.consume(TokenKind::RightParen, "expected `)` after parameters")?;
        let return_ty = if self.matches(TokenKind::Arrow) {
            Some(self.existential_ty()?)
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
            name_splice,
            generics,
            params,
            return_ty,
            body,
            is_pub,
            is_unsafe,
            is_iter,
            is_comptime: false,
            extern_abi,
            attrs: Vec::new(),
            span,
        })
    }

    /// `[unsafe] [iter] [extern StringLiteral]`, valid immediately before
    /// `fn` wherever a `FunctionDecl` can appear as a struct/enum/methods-block
    /// member. Module-level `extern "ABI"` has an extra ambiguity (a
    /// bodiless `ExternBlock` also starts this way) handled separately in
    /// `parse_module`.
    /// `{ OuterAttribute }` — zero or more `#[...]` attributes immediately
    /// before an item/member. Parses eagerly: as long as the next token is
    /// `#`, one more attribute is expected (a malformed one still consumes
    /// through its own error rather than being silently skipped).
    fn parse_outer_attributes(&mut self) -> ParseResult<Vec<Attribute>> {
        let mut attrs = Vec::new();
        while self.check(TokenKind::Hash) {
            attrs.push(self.parse_one_attribute()?);
        }
        Ok(attrs)
    }

    /// `OuterAttribute = "#" "[" Identifier [ "(" [ AttributeArgList ] ")" ] "]"`.
    fn parse_one_attribute(&mut self) -> ParseResult<Attribute> {
        let start = self.consume(TokenKind::Hash, "expected `#`")?.span.start();
        self.consume(TokenKind::LeftBracket, "expected `[` after `#`")?;
        let name = self.consume_identifier("expected attribute name")?;
        let mut args = Vec::new();
        if self.matches(TokenKind::LeftParen) {
            if !self.check(TokenKind::RightParen) {
                loop {
                    args.push(self.parse_attribute_arg()?);
                    if !self.matches(TokenKind::Comma) {
                        break;
                    }
                    if self.check(TokenKind::RightParen) {
                        break;
                    }
                }
            }
            self.consume(TokenKind::RightParen, "expected `)` after attribute arguments")?;
        }
        let right = self.consume(TokenKind::RightBracket, "expected `]` after attribute")?;
        Ok(Attribute {
            name,
            args,
            span: Span::new(right.span.file_id(), start, right.span.end()),
        })
    }

    /// `AttributeArg = Literal | Path | Identifier | OuterAttribute`. A
    /// bare identifier and a single-segment path are the same production
    /// here (`AttributeArg::Path` with one segment) — nothing downstream
    /// needs to distinguish them.
    fn parse_attribute_arg(&mut self) -> ParseResult<AttributeArg> {
        if self.check(TokenKind::Hash) {
            return Ok(AttributeArg::Nested(self.parse_one_attribute()?));
        }
        if self.check(TokenKind::Identifier) {
            let start = self.peek().span.start();
            let mut path = vec![self.consume_identifier("expected identifier")?];
            if self.matches(TokenKind::Equal) {
                let key = path.remove(0);
                path.push(self.consume_identifier("expected a path after `=`")?);
                while self.matches(TokenKind::ColonColon) {
                    path.push(self.consume_identifier("expected identifier after `::`")?);
                }
                let span = Span::new(self.previous().span.file_id(), start, self.previous().span.end());
                return Ok(AttributeArg::Assign(key, path, span));
            }
            while self.matches(TokenKind::ColonColon) {
                path.push(self.consume_identifier("expected identifier after `::`")?);
            }
            let span = Span::new(self.previous().span.file_id(), start, self.previous().span.end());
            return Ok(AttributeArg::Path(path, span));
        }
        let span = self.peek().span;
        let literal = self.literal_token()?;
        Ok(AttributeArg::Literal(literal, span))
    }

    fn literal_token(&mut self) -> ParseResult<Literal> {
        if self.matches(TokenKind::Integer) {
            let value = self.previous().lexeme.replace('_', "").parse().unwrap_or(0);
            return Ok(Literal::Int(value));
        }
        if self.matches(TokenKind::Float) {
            let value = self.previous().lexeme.replace('_', "").parse().unwrap_or(0.0);
            return Ok(Literal::Float(value));
        }
        if self.matches(TokenKind::True) {
            return Ok(Literal::Bool(true));
        }
        if self.matches(TokenKind::False) {
            return Ok(Literal::Bool(false));
        }
        if self.matches(TokenKind::String) {
            return Ok(Literal::String(decode_string(&self.previous().lexeme)));
        }
        self.error_here("PACO-E0112", "expected a literal, path, or nested attribute");
        Err(ParseError)
    }

    /// `[ "comptime" | "iter" ] [ "unsafe" ] [ "extern" StringLiteral ]`
    /// (`grammar.ebnf`'s `FunctionDecl`). `comptime`/`iter` are mutually
    /// exclusive by construction: matching `comptime` consumes the token,
    /// so `iter` is never even checked — `comptime iter fn f() {}` then
    /// fails naturally at the `consume(TokenKind::Fn, ..)` call right
    /// after this function returns, since the unconsumed `iter` token
    /// sits where `fn` is expected.
    fn function_modifiers(&mut self) -> ParseResult<(bool, bool, bool, Option<String>)> {
        let is_comptime = self.matches(TokenKind::Comptime);
        let is_iter = !is_comptime && self.matches(TokenKind::Iter);
        let is_unsafe = self.matches(TokenKind::Unsafe);
        let extern_abi = if self.matches(TokenKind::Extern) {
            self.consume(TokenKind::String, "expected an ABI string literal after `extern`")?;
            Some(decode_string(&self.previous().lexeme))
        } else {
            None
        };
        Ok((is_comptime, is_unsafe, is_iter, extern_abi))
    }

    fn extern_block(&mut self, abi: String) -> ParseResult<ExternBlock> {
        let start = self.previous().span.start();
        self.consume(TokenKind::LeftBrace, "expected `{` after extern ABI")?;
        let mut functions = Vec::new();
        while !self.check(TokenKind::RightBrace) && !self.check(TokenKind::Eof) {
            self.consume(TokenKind::Fn, "expected a function declaration in an extern block")?;
            functions.push(self.extern_fn_decl()?);
        }
        let right = self.consume(TokenKind::RightBrace, "expected `}` after extern block")?;
        Ok(ExternBlock {
            abi,
            functions,
            span: Span::new(right.span.file_id(), start, right.span.end()),
        })
    }

    fn extern_fn_decl(&mut self) -> ParseResult<FnSignature> {
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
        self.consume(
            TokenKind::Semicolon,
            "expected `;` after an extern function signature (no body allowed)",
        )?;
        Ok(FnSignature {
            name,
            generics,
            params,
            return_ty,
            body: None,
            span: Span::new(
                self.previous().span.file_id(),
                start,
                self.previous().span.end(),
            ),
        })
    }

    fn struct_decl(&mut self, is_pub: bool) -> ParseResult<StructDecl> {
        let start = self.previous().span.start();
        let name = self.consume_identifier("expected struct name")?;
        let generics = self.generic_params()?;
        self.consume(TokenKind::LeftBrace, "expected `{` before struct body")?;
        let mut fields = Vec::new();
        let mut methods = Vec::new();
        let mut consts = Vec::new();
        let mut assoc_types = Vec::new();

        while !self.check(TokenKind::RightBrace) && !self.check(TokenKind::Eof) {
            let member_attrs = self.parse_outer_attributes()?;
            let member_is_pub = self.matches(TokenKind::Pub);
            if self.matches(TokenKind::Type) {
                assoc_types.push(self.assoc_type_decl()?);
                continue;
            }
            if self.check(TokenKind::Comptime)
                || self.check(TokenKind::Unsafe)
                || self.check(TokenKind::Iter)
                || self.check(TokenKind::Extern)
            {
                let (is_comptime, is_unsafe, is_iter, extern_abi) = self.function_modifiers()?;
                self.consume(TokenKind::Fn, "expected `fn` after comptime/unsafe/iter/extern modifiers")?;
                let mut decl = self.function_decl(member_is_pub, is_unsafe, is_iter, extern_abi)?;
                decl.attrs = member_attrs;
                decl.is_comptime = is_comptime;
                methods.push(decl);
            } else if self.matches(TokenKind::Fn) {
                let mut decl = self.function_decl(member_is_pub, false, false, None)?;
                decl.attrs = member_attrs;
                methods.push(decl);
            } else if self.matches(TokenKind::Const) {
                consts.push(self.const_decl(member_is_pub)?);
            } else {
                let mut field = self.field_decl(member_is_pub)?;
                field.attrs = member_attrs;
                fields.push(field);
                if !self.matches(TokenKind::Comma) && !self.check(TokenKind::RightBrace) {
                    self.missing_separator("PACO-E0112", ",", "expected `,` after a struct field");
                }
                continue;
            }
            self.matches(TokenKind::Comma);
        }

        let right = self.consume(TokenKind::RightBrace, "expected `}` after struct body")?;
        Ok(StructDecl {
            name,
            generics,
            fields,
            methods,
            consts,
            assoc_types,
            is_pub,
            attrs: Vec::new(),
            span: Span::new(right.span.file_id(), start, right.span.end()),
        })
    }

    fn enum_decl(&mut self, is_pub: bool) -> ParseResult<EnumDecl> {
        let start = self.previous().span.start();
        let name = self.consume_identifier("expected enum name")?;
        let generics = self.generic_params()?;
        self.consume(TokenKind::LeftBrace, "expected `{` before enum body")?;
        let mut variants = Vec::new();
        let mut methods = Vec::new();
        let mut consts = Vec::new();

        while !self.check(TokenKind::RightBrace) && !self.check(TokenKind::Eof) {
            let member_attrs = self.parse_outer_attributes()?;
            let member_is_pub = self.matches(TokenKind::Pub);
            if self.check(TokenKind::Comptime)
                || self.check(TokenKind::Unsafe)
                || self.check(TokenKind::Iter)
                || self.check(TokenKind::Extern)
            {
                let (is_comptime, is_unsafe, is_iter, extern_abi) = self.function_modifiers()?;
                self.consume(TokenKind::Fn, "expected `fn` after comptime/unsafe/iter/extern modifiers")?;
                let mut decl = self.function_decl(member_is_pub, is_unsafe, is_iter, extern_abi)?;
                decl.attrs = member_attrs;
                decl.is_comptime = is_comptime;
                methods.push(decl);
            } else if self.matches(TokenKind::Fn) {
                let mut decl = self.function_decl(member_is_pub, false, false, None)?;
                decl.attrs = member_attrs;
                methods.push(decl);
            } else if self.matches(TokenKind::Const) {
                consts.push(self.const_decl(member_is_pub)?);
            } else {
                if member_is_pub {
                    self.error_here(
                        "PACO-E0110",
                        "`pub` is not allowed directly before an enum variant",
                    );
                }
                let mut variant = self.enum_variant()?;
                variant.attrs = member_attrs;
                variants.push(variant);
                if !self.matches(TokenKind::Comma) && !self.check(TokenKind::RightBrace) {
                    self.missing_separator("PACO-E0112", ",", "expected `,` after an enum variant");
                }
                continue;
            }
            self.matches(TokenKind::Comma);
        }

        let right = self.consume(TokenKind::RightBrace, "expected `}` after enum body")?;
        Ok(EnumDecl {
            name,
            generics,
            variants,
            methods,
            consts,
            is_pub,
            attrs: Vec::new(),
            span: Span::new(right.span.file_id(), start, right.span.end()),
        })
    }

    fn methods_block(&mut self) -> ParseResult<MethodsBlock> {
        let start = self.previous().span.start();
        let generics = self.generic_params()?;
        let target = self.ty()?;
        self.consume(TokenKind::LeftBrace, "expected `{` before methods body")?;
        let mut methods = Vec::new();
        let mut consts = Vec::new();

        while !self.check(TokenKind::RightBrace) && !self.check(TokenKind::Eof) {
            let member_attrs = self.parse_outer_attributes()?;
            let member_is_pub = self.matches(TokenKind::Pub);
            if self.matches(TokenKind::Const) {
                consts.push(self.const_decl(member_is_pub)?);
            } else {
                let (is_comptime, is_unsafe, is_iter, extern_abi) = self.function_modifiers()?;
                self.consume(TokenKind::Fn, "expected method declaration")?;
                let mut decl = self.function_decl(member_is_pub, is_unsafe, is_iter, extern_abi)?;
                decl.is_comptime = is_comptime;
                decl.attrs = member_attrs;
                methods.push(decl);
            }
        }

        let right = self.consume(TokenKind::RightBrace, "expected `}` after methods body")?;
        Ok(MethodsBlock {
            generics,
            target,
            methods,
            consts,
            span: Span::new(right.span.file_id(), start, right.span.end()),
        })
    }

    fn module_path(&mut self) -> ParseResult<(Vec<String>, UsePathKind)> {
        let first = self.consume_identifier("expected module path")?;
        if self.check(TokenKind::ColonColon) {
            let mut path = vec![first];
            while self.matches(TokenKind::ColonColon) {
                path.push(self.consume_identifier("expected path segment after `::`")?);
            }
            return Ok((path, UsePathKind::Plain));
        }
        if self.check(TokenKind::Dot) || self.check(TokenKind::Slash) {
            let mut segments = vec![first];
            while self.matches(TokenKind::Dot) {
                segments.push(self.consume_identifier("expected path segment after `.`")?);
            }
            self.consume(TokenKind::Slash, "expected `/` after domain segments")?;
            segments.push(self.consume_identifier("expected path segment after `/`")?);
            while self.matches(TokenKind::Slash) {
                segments.push(self.consume_identifier("expected path segment after `/`")?);
            }
            return Ok((segments, UsePathKind::Domain));
        }
        Ok((vec![first], UsePathKind::Plain))
    }

    fn use_decl(&mut self) -> ParseResult<UseDecl> {
        let start = self.previous().span.start();
        let (path, kind) = self.module_path()?;
        let alias = if self.matches(TokenKind::As) {
            Some(self.consume_identifier("expected alias name after `as`")?)
        } else {
            None
        };
        let end = self.previous().span.end();
        self.expect_semicolon();
        Ok(UseDecl {
            path,
            alias,
            kind,
            span: Span::new(self.previous().span.file_id(), start, end),
        })
    }

    /// `const NAME: Type = Expr [;]`. Modeled on `let_stmt`, but the type
    /// annotation is mandatory (ADR 0016) and a value is always required.
    fn const_decl(&mut self, is_pub: bool) -> ParseResult<ConstDecl> {
        let start = self.previous().span.start();
        let name = self.consume_identifier("expected constant name")?;
        self.consume(TokenKind::Colon, "expected `:` after constant name (const requires a type annotation)")?;
        let ty = self.ty()?;
        self.consume(TokenKind::Equal, "expected `=` after constant type")?;
        let value = self.expr()?;
        self.expect_semicolon();
        Ok(ConstDecl {
            name,
            ty,
            value,
            is_pub,
            span: Span::new(
                self.previous().span.file_id(),
                start,
                self.previous().span.end(),
            ),
        })
    }

    fn trait_decl(&mut self, is_pub: bool) -> ParseResult<TraitDecl> {
        let start = self.previous().span.start();
        let name = self.consume_identifier("expected trait name")?;
        let generics = self.generic_params()?;
        self.consume(TokenKind::LeftBrace, "expected `{` before trait body")?;
        let mut methods = Vec::new();
        let mut consts = Vec::new();
        let mut assoc_types = Vec::new();

        while !self.check(TokenKind::RightBrace) && !self.check(TokenKind::Eof) {
            if self.matches(TokenKind::Fn) {
                methods.push(self.trait_fn_decl()?);
            } else if self.matches(TokenKind::Type) {
                assoc_types.push(self.assoc_type_decl()?);
            } else if self.matches(TokenKind::Const) {
                consts.push(self.const_decl(false)?);
            } else {
                self.error_here("PACO-E0110", "expected item declaration");
                self.synchronize_trait_member();
            }
        }

        let right = self.consume(TokenKind::RightBrace, "expected `}` after trait body")?;
        Ok(TraitDecl {
            name,
            generics,
            methods,
            consts,
            assoc_types,
            is_pub,
            attrs: Vec::new(),
            span: Span::new(right.span.file_id(), start, right.span.end()),
        })
    }

    fn trait_fn_decl(&mut self) -> ParseResult<FnSignature> {
        let start = self.previous().span.start();
        let name = self.consume_identifier("expected function name")?;
        let generics = self.generic_params()?;
        self.consume(TokenKind::LeftParen, "expected `(` after function name")?;
        let params = self.parameter_list()?;
        self.consume(TokenKind::RightParen, "expected `)` after parameters")?;
        let return_ty = if self.matches(TokenKind::Arrow) {
            Some(self.existential_ty()?)
        } else {
            None
        };
        let body = if self.matches(TokenKind::Semicolon) {
            None
        } else {
            Some(self.block()?)
        };
        Ok(FnSignature {
            name,
            generics,
            params,
            return_ty,
            body,
            span: Span::new(
                self.previous().span.file_id(),
                start,
                self.previous().span.end(),
            ),
        })
    }

    fn assoc_type_decl(&mut self) -> ParseResult<AssocTypeDecl> {
        let start = self.previous().span.start();
        let name = self.consume_identifier("expected associated type name")?;
        let default = if self.matches(TokenKind::Equal) {
            Some(self.ty()?)
        } else {
            None
        };
        self.expect_semicolon();
        Ok(AssocTypeDecl {
            name,
            default,
            span: Span::new(
                self.previous().span.file_id(),
                start,
                self.previous().span.end(),
            ),
        })
    }

    fn existential_ty(&mut self) -> ParseResult<Ty> {
        let outer = std::mem::replace(&mut self.allow_existentials, true);
        let ty = self.ty();
        self.allow_existentials = outer;
        ty
    }

    fn field_decl(&mut self, is_pub: bool) -> ParseResult<FieldDecl> {
        let start = self.peek().span.start();
        let name = self.consume_identifier("expected field name")?;
        self.consume(TokenKind::Colon, "expected `:` after field name")?;
        let ty = self.existential_ty()?;
        Ok(FieldDecl {
            name,
            ty,
            is_pub,
            attrs: Vec::new(),
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
                    fields.push(self.field_decl(false)?);
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
            attrs: Vec::new(),
            span: Span::new(
                self.previous().span.file_id(),
                start,
                self.previous().span.end(),
            ),
        })
    }

    fn generic_params(&mut self) -> ParseResult<Vec<GenericParam>> {
        let mut params: Vec<GenericParam> = Vec::new();
        if !self.matches(TokenKind::Less) {
            return Ok(params);
        }
        loop {
            if self.check(TokenKind::Greater) {
                break;
            }
            let param = self.consume_generic_parameter()?;
            if let Some(pack) = params.iter().find(|existing| existing.is_pack()) {
                let message = if param.is_pack() {
                    "only one const parameter pack is allowed per generic parameter list"
                } else {
                    "a const parameter pack must be the last generic parameter"
                };
                self.reporter.push(Diagnostic::error("PACO-E0114", pack.span, message));
            }
            params.push(param);
            if !self.matches(TokenKind::Comma) {
                break;
            }
        }
        self.consume(TokenKind::Greater, "expected `>` after generic parameters")?;
        Ok(params)
    }

    /// One generic argument: a type, the `Dyn` dimension marker, or a const
    /// expression (`768`, `M * 2`, `(N + 1) * 2`).
    fn generic_arg(&mut self) -> ParseResult<Ty> {
        let token = self.peek().clone();
        if token.kind == TokenKind::Identifier
            && token.lexeme == "Dyn"
            && !self.peek_next().is_some_and(|next| matches!(next.kind, TokenKind::ColonColon | TokenKind::Less))
        {
            self.advance();
            return Ok(Ty::DynDim(token.span));
        }
        if token.kind == TokenKind::Question && self.peek_next().is_some_and(|next| next.kind == TokenKind::Identifier) {
            if !self.allow_existentials {
                self.error_here(
                    "PACO-E0112",
                    "`?name` names a dimension the producer chooses; it is allowed only in a return type or a struct field type",
                );
            }
            self.advance();
            let name = self.advance().clone();
            return Ok(Ty::Existential(name.lexeme, Span::new(token.span.file_id(), token.span.start(), name.span.end())));
        }
        if token.kind == TokenKind::Identifier
            && self.peek_next().is_some_and(|next| next.kind == TokenKind::DotDotDot)
        {
            self.advance();
            let end = self.advance().span.end();
            return Ok(Ty::Expand(token.lexeme, Span::new(token.span.file_id(), token.span.start(), end)));
        }
        if self.starts_const_arg() {
            let expr = self.additive()?;
            let span = expr_span(&expr);
            return Ok(Ty::Const(Box::new(expr), span));
        }
        self.ty()
    }

    fn starts_const_arg(&self) -> bool {
        let is_operator = |kind: Option<TokenKind>| {
            matches!(
                kind,
                Some(TokenKind::Plus | TokenKind::Minus | TokenKind::Star | TokenKind::Slash | TokenKind::Percent)
            )
        };
        let next = self.peek_next().map(|token| token.kind);
        match self.peek().kind {
            TokenKind::Integer => true,
            TokenKind::Identifier => is_operator(next),
            TokenKind::LeftParen => {
                let after = self.tokens.get(self.current + 2).map(|token| token.kind);
                next == Some(TokenKind::Integer)
                    || next == Some(TokenKind::LeftParen)
                    || (next == Some(TokenKind::Identifier) && is_operator(after))
            }
            _ => false,
        }
    }

    fn parameter_list(&mut self) -> ParseResult<Vec<Param>> {
        let mut params = Vec::new();
        if self.check(TokenKind::RightParen) {
            return Ok(params);
        }

        loop {
            let start = self.peek().span.start();

            // A leading `&` can only begin a receiver here: an ordinary
            // parameter is always `name: Type`.
            let (name, ty) = if self.matches(TokenKind::Ampersand) {
                let mutable = self.matches(TokenKind::Mut);
                let name = self.consume_identifier("expected `self` after `&` in receiver")?;
                if name != "self" {
                    self.error_here(
                        "PACO-E0112",
                        "only `self` may be borrowed in a parameter list; \
                         write `name: &Type` for an ordinary borrowed parameter",
                    );
                }
                let span = Span::new(
                    self.previous().span.file_id(),
                    start,
                    self.previous().span.end(),
                );
                (
                    name,
                    Ty::Borrow {
                        mutable,
                        lifetime: None,
                        ty: Box::new(Ty::Path(vec!["Self".to_string()], self.previous().span)),
                        span,
                    },
                )
            } else {
                let name = self.consume_identifier("expected parameter name")?;
                if name == "self" {
                    let ty = Ty::Path(vec!["Self".to_string()], self.previous().span);
                    (name, ty)
                } else {
                    self.consume(TokenKind::Colon, "expected `:` after parameter name")?;
                    let ty = self.ty()?;
                    (name, ty)
                }
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
            if !self.matches(TokenKind::Comma) || self.check(TokenKind::RightParen) {
                break;
            }
        }
        Ok(params)
    }

    fn ty(&mut self) -> ParseResult<Ty> {
        let start = self.peek().span.start();
        // `type` (the comptime-only pseudo-type, `phase-9-comptime`
        // Decision 5) lexes as the `TokenKind::Type` keyword — the same
        // token `type Alias = ..` declarations start with — so it needs
        // its own case here rather than falling through to `path()`,
        // which only accepts `TokenKind::Identifier`.
        if self.matches(TokenKind::Type) {
            let span = self.previous().span;
            return Ok(Ty::Path(vec!["type".to_string()], span));
        }
        // `#(expr)` in a type position — only meaningful inside a `quote {
        // .. }` template (`phase-9-comptime` Decision 7).
        if self.matches(TokenKind::Hash) {
            let start = self.previous().span;
            self.consume(TokenKind::LeftParen, "expected `(` after `#`")?;
            let inner = self.expr()?;
            let right = self.consume(TokenKind::RightParen, "expected `)` after splice expression")?;
            let span = Span::new(start.file_id(), start.start(), right.span.end());
            return Ok(Ty::Splice(Box::new(inner), span));
        }
        if self.matches(TokenKind::LeftParen) {
            let left_paren = self.previous().span;
            let mut items = Vec::new();
            if !self.check(TokenKind::RightParen) {
                loop {
                    items.push(self.ty()?);
                    if !self.matches(TokenKind::Comma) {
                        break;
                    }
                }
            }
            let right = self.consume(TokenKind::RightParen, "expected `)` after tuple type")?;
            let span = Span::new(left_paren.file_id(), start, right.span.end());
            // A single type with no trailing comma is a parenthesized type, not a
            // 1-tuple — there is no `(T,)` 1-tuple syntax to disambiguate it from,
            // since no `Idx`/`Type::Tuple` use case in this compiler needs one.
            return Ok(if items.len() == 1 {
                items.into_iter().next().expect("checked len == 1")
            } else {
                Ty::Tuple(items, span)
            });
        }
        if self.matches(TokenKind::Fn) {
            let fn_span = self.previous().span;
            self.consume(TokenKind::LeftParen, "expected `(` after `fn` in a function type")?;
            let mut params = Vec::new();
            while !self.check(TokenKind::RightParen) {
                params.push(self.ty()?);
                if !self.matches(TokenKind::Comma) {
                    break;
                }
            }
            let mut end = self.consume(TokenKind::RightParen, "expected `)` after function type parameters")?.span.end();
            let return_ty = if self.matches(TokenKind::Arrow) {
                let ty = self.ty()?;
                end = ty_span(&ty).end();
                Some(Box::new(ty))
            } else {
                None
            };
            return Ok(Ty::Fn {
                params,
                return_ty,
                span: Span::new(fn_span.file_id(), start, end),
            });
        }
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
        if self.matches(TokenKind::LeftBracket) {
            let left_bracket = self.previous().span;
            self.consume(TokenKind::RightBracket, "expected `]` after `[` in a slice type")?;
            let ty = self.ty()?;
            let span = Span::new(left_bracket.file_id(), start, ty_span(&ty).end());
            return Ok(Ty::Slice(Box::new(ty), span));
        }
        if self.matches(TokenKind::Star) {
            let star = self.previous().span;
            let mutable = if self.matches(TokenKind::Mut) {
                true
            } else {
                self.consume(TokenKind::Const, "expected `const` or `mut` after `*` in a type")?;
                false
            };
            let ty = self.ty()?;
            let span = Span::new(star.file_id(), start, ty_span(&ty).end());
            return Ok(Ty::RawPointer {
                mutable,
                ty: Box::new(ty),
                span,
            });
        }
        let path = self.path()?;
        if self.matches(TokenKind::Less) {
            let mut args = Vec::new();
            if !self.check(TokenKind::Greater) {
                loop {
                    args.push(self.generic_arg()?);
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
            if self.matches(TokenKind::Semicolon) {
                continue;
            }
            if self.matches(TokenKind::Let) {
                stmts.push(Stmt::Let(self.let_stmt()?));
                continue;
            }

            let expr = if self.starts_block_like() {
                let expr = self.primary()?;
                if !self.check(TokenKind::Dot) && !self.check(TokenKind::Question) {
                    if self.matches(TokenKind::Semicolon) || !self.check(TokenKind::RightBrace) {
                        stmts.push(Stmt::Expr(expr));
                    } else {
                        tail = Some(Box::new(expr));
                    }
                    continue;
                }
                self.postfix(expr)?
            } else {
                self.expr()?
            };
            if self.matches(TokenKind::Semicolon) {
                stmts.push(Stmt::Expr(expr));
            } else if self.check(TokenKind::RightBrace) {
                tail = Some(Box::new(expr));
            } else {
                self.expect_semicolon();
                stmts.push(Stmt::Expr(expr));
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
        let pattern = self.pattern()?;
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
        self.expect_semicolon();
        let span = Span::new(
            self.previous().span.file_id(),
            start,
            self.previous().span.end(),
        );
        Ok(LetStmt {
            mutable,
            pattern,
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
        let mut expr = self.bit_or()?;
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
                let right = self.bit_or()?;
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

    fn bit_or(&mut self) -> ParseResult<Expr> {
        self.left_associative(Self::bit_xor, &[(TokenKind::Pipe, BinaryOp::BitOr)])
    }

    fn bit_xor(&mut self) -> ParseResult<Expr> {
        self.left_associative(Self::bit_and, &[(TokenKind::Caret, BinaryOp::BitXor)])
    }

    fn bit_and(&mut self) -> ParseResult<Expr> {
        self.left_associative(Self::shift, &[(TokenKind::Ampersand, BinaryOp::BitAnd)])
    }

    /// `<<` and `>>` are two adjacent `<`/`>` tokens, so that nested generic
    /// arguments such as `Vec<Vec<i64>>` keep closing with single tokens.
    fn shift(&mut self) -> ParseResult<Expr> {
        let mut expr = self.additive()?;
        loop {
            let adjacent = self.peek_next().is_some_and(|next| next.kind == self.peek().kind && next.span.start() == self.peek().span.end());
            let op = match self.peek().kind {
                TokenKind::Less if adjacent => BinaryOp::Shl,
                TokenKind::Greater if adjacent => BinaryOp::Shr,
                _ => return Ok(expr),
            };
            self.advance();
            self.advance();
            let right = self.additive()?;
            let span = join_expr_span(&expr, &right, self.previous().span);
            expr = Expr::Binary { op, left: Box::new(expr), right: Box::new(right), span };
        }
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
            Self::cast,
            &[
                (TokenKind::Star, BinaryOp::Mul),
                (TokenKind::Slash, BinaryOp::Div),
                (TokenKind::Percent, BinaryOp::Rem),
            ],
        )
    }

    /// `CastExpr = UnaryExpr { "as" Type } ;` — left-associative, binds
    /// tighter than arithmetic (`a * b as i64` is `a * (b as i64)`) but
    /// looser than unary prefix ops (`-x as i64` is `(-x) as i64`).
    fn cast(&mut self) -> ParseResult<Expr> {
        let mut expr = self.unary()?;
        while self.matches(TokenKind::As) {
            let ty = self.ty()?;
            let span = Span::new(
                expr_span(&expr).file_id(),
                expr_span(&expr).start(),
                ty_span(&ty).end(),
            );
            expr = Expr::Cast {
                expr: Box::new(expr),
                ty,
                span,
            };
        }
        Ok(expr)
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
        if self.matches(TokenKind::Tilde) {
            let operator = self.previous().span;
            let expr = self.unary()?;
            let span = Span::new(operator.file_id(), operator.start(), expr_span(&expr).end());
            return Ok(Expr::Unary { op: UnaryOp::BitNot, expr: Box::new(expr), span });
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
        if self.matches(TokenKind::Star) {
            let operator = self.previous().span;
            let expr = self.unary()?;
            let span = Span::new(operator.file_id(), operator.start(), expr_span(&expr).end());
            return Ok(Expr::Unary {
                op: UnaryOp::Deref,
                expr: Box::new(expr),
                span,
            });
        }
        if self.matches(TokenKind::Spawn) {
            let start = self.previous().span;
            let expr = if self.check(TokenKind::LeftBrace) {
                Expr::Block(Box::new(self.block()?))
            } else {
                self.unary()?
            };
            let span = Span::new(start.file_id(), start.start(), expr_span(&expr).end());
            return Ok(Expr::Spawn {
                expr: Box::new(expr),
                span,
            });
        }
        self.call()
    }

    fn call(&mut self) -> ParseResult<Expr> {
        let expr = self.primary()?;
        self.postfix(expr)
    }

    fn postfix(&mut self, mut expr: Expr) -> ParseResult<Expr> {
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
                    type_args: Vec::new(),
                    args,
                    span,
                };
            } else if self.check(TokenKind::Dot) && self.peek_next().is_some_and(|t| t.kind == TokenKind::Hash) {
                // `base.#(name_expr)` — an identifier-position splice
                // (`phase-9-comptime` Decision 7), only meaningful inside
                // a `quote { .. }` template. Desugars at parse time to a
                // call of the `splice_field` builtin, rather than adding
                // a dynamic-name variant to `Expr::Field` itself (which
                // every other consumer of that variant would then need to
                // handle) — `quote { .. }`'s own evaluation (task 5.3)
                // recognizes this exact call shape and substitutes it
                // like any other splice. `check`+`peek_next` (not
                // `matches`), so a plain `.identifier` immediately after
                // falls through to the ordinary field/method-call branch
                // below unconsumed, rather than losing the `.` to a failed
                // lookahead the way `matches(..) && check(..)` would.
                self.advance();
                self.advance();
                self.consume(TokenKind::LeftParen, "expected `(` after `#`")?;
                let name_expr = self.expr()?;
                let right = self.consume(TokenKind::RightParen, "expected `)` after splice expression")?;
                let span = Span::new(
                    expr_span(&expr).file_id(),
                    expr_span(&expr).start(),
                    right.span.end(),
                );
                expr = Expr::Call {
                    callee: Box::new(Expr::Ident("splice_field".to_string(), span)),
                    type_args: Vec::new(),
                    args: vec![expr, name_expr],
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
            } else if self.matches(TokenKind::Question) {
                let span = Span::new(
                    expr_span(&expr).file_id(),
                    expr_span(&expr).start(),
                    self.previous().span.end(),
                );
                expr = Expr::Try {
                    expr: Box::new(expr),
                    span,
                };
            } else if self.matches(TokenKind::LeftBracket) {
                let mut index = vec![self.expr()?];
                while self.matches(TokenKind::Comma) {
                    index.push(self.expr()?);
                }
                let right = self.consume(TokenKind::RightBracket, "expected `]` after index expression")?;
                let span = Span::new(
                    expr_span(&expr).file_id(),
                    expr_span(&expr).start(),
                    right.span.end(),
                );
                expr = Expr::Index {
                    base: Box::new(expr),
                    index,
                    span,
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
            if self.check(TokenKind::Identifier)
                && self.peek_next().is_some_and(|t| t.kind == TokenKind::Colon)
            {
                self.advance();
                self.advance();
            }
            args.push(self.expr()?);
            if !self.matches(TokenKind::Comma) {
                break;
            }
        }
        Ok(args)
    }

    fn primary(&mut self) -> ParseResult<Expr> {
        if self.check(TokenKind::Pipe) || self.check(TokenKind::OrOr) {
            return self.closure();
        }
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
        if self.matches(TokenKind::Char) {
            let token = self.previous();
            return Ok(Expr::Literal(
                Literal::Char(decode_char(&token.lexeme)),
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
        if self.matches(TokenKind::Select) {
            return self.select_expr();
        }
        if self.matches(TokenKind::Yield) {
            let start = self.previous().span;
            let value = self.expr()?;
            let span = Span::new(start.file_id(), start.start(), expr_span(&value).end());
            return Ok(Expr::Yield(Box::new(value), span));
        }
        if self.matches(TokenKind::Identifier) {
            let token = self.previous().clone();
            let ty = self.expr_type_path(&token)?;
            let ty = if let Ty::Path(mut path, mut span) = ty {
                let mut generic = None;
                while self.check(TokenKind::ColonColon)
                    && self.tokens.get(self.current + 1).is_some_and(|t| t.kind == TokenKind::Identifier)
                {
                    let after = self.tokens.get(self.current + 2).map(|t| t.kind);
                    if after != Some(TokenKind::ColonColon) && after != Some(TokenKind::Less) {
                        break;
                    }
                    let saved = self.current;
                    self.advance();
                    let segment = self.consume_identifier("expected path segment")?;
                    if after == Some(TokenKind::Less) {
                        if !self.starts_generic_type_application() {
                            self.current = saved;
                            break;
                        }
                        path.push(segment);
                        let (args, end) = self.generic_type_args()?;
                        generic = Some(Ty::Generic {
                            path: std::mem::take(&mut path),
                            args,
                            span: Span::new(span.file_id(), span.start(), end),
                        });
                        break;
                    }
                    span = Span::new(span.file_id(), span.start(), self.previous().span.end());
                    path.push(segment);
                }
                generic.unwrap_or(Ty::Path(path, span))
            } else {
                ty
            };
            if self.allow_struct_literals && self.matches(TokenKind::LeftBrace) {
                return self.struct_literal(ty);
            }
            if self.matches(TokenKind::ColonColon) {
                let function = self.consume_identifier("expected associated item name")?;
                if self.allow_struct_literals && self.check(TokenKind::LeftBrace) {
                    let Ty::Path(mut path, path_span) = ty else {
                        self.error_here("PACO-E0111", "expected a plain path before `{` in a qualified struct literal");
                        return Err(ParseError);
                    };
                    path.push(function);
                    self.matches(TokenKind::LeftBrace);
                    let qualified_span = Span::new(path_span.file_id(), path_span.start(), self.previous().span.end());
                    return self.struct_literal(Ty::Path(path, qualified_span));
                }
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
            if let Ty::Generic { path, args: type_args, span: generic_span } = ty {
                if path.len() == 1 && self.matches(TokenKind::LeftParen) {
                    let args = self.argument_list()?;
                    let right = self.consume(TokenKind::RightParen, "expected `)` after arguments")?;
                    return Ok(Expr::Call {
                        callee: Box::new(Expr::Ident(path[0].clone(), token.span)),
                        type_args,
                        args,
                        span: Span::new(token.span.file_id(), generic_span.start(), right.span.end()),
                    });
                }
                self.error_here("PACO-E0111", "expected expression");
                return Err(ParseError);
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
        if self.matches(TokenKind::Unsafe) {
            let start = self.previous().span;
            let body = self.block()?;
            let span = Span::new(start.file_id(), start.start(), body.span.end());
            return Ok(Expr::Unsafe(Box::new(body), span));
        }
        if self.matches(TokenKind::Comptime) {
            let start = self.previous().span;
            let body = self.block()?;
            let span = Span::new(start.file_id(), start.start(), body.span.end());
            return Ok(Expr::Comptime {
                expr: Box::new(Expr::Block(Box::new(body))),
                span,
            });
        }
        // `quote { .. }` (`phase-9-comptime` Decision 7): the template's
        // body is an item when it starts with an item keyword, an
        // expression otherwise. Scoped to `methods { .. }` blocks for
        // now — the only item shape `#[derive]` expansion (this phase's
        // own deliverable) produces; extending this to other item kinds
        // is a small, mechanical follow-up once something needs one.
        if self.matches(TokenKind::Quote) {
            let start = self.previous().span;
            self.consume(TokenKind::LeftBrace, "expected `{` after `quote`")?;
            let body = if self.matches(TokenKind::Methods) {
                QuoteBody::Item(Item::Methods(self.methods_block()?))
            } else {
                QuoteBody::Expr(self.expr()?)
            };
            let right = self.consume(TokenKind::RightBrace, "expected `}` after quote template")?;
            let span = Span::new(start.file_id(), start.start(), right.span.end());
            return Ok(Expr::Quote(Box::new(body), span));
        }
        // `#(expr)` in expression position — only meaningful inside a
        // `quote { .. }` template (unchecked here; a `paco-types` check
        // rejects a `Splice` reached outside one, mirroring `comptime
        // fn`'s own `requires_comptime` restriction).
        if self.matches(TokenKind::Hash) {
            let start = self.previous().span;
            self.consume(TokenKind::LeftParen, "expected `(` after `#`")?;
            let inner = self.expr()?;
            let right = self.consume(TokenKind::RightParen, "expected `)` after splice expression")?;
            let span = Span::new(start.file_id(), start.start(), right.span.end());
            return Ok(Expr::Splice(Box::new(inner), span));
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
            let left = self.previous().span;
            let mut items = Vec::new();
            while !self.check(TokenKind::RightParen) {
                items.push(self.expr()?);
                if !self.matches(TokenKind::Comma) {
                    break;
                }
            }
            let end = self.consume(TokenKind::RightParen, "expected `)` after expression")?.span.end();
            if items.len() == 1 {
                return Ok(items.pop().expect("one element"));
            }
            return Ok(Expr::Tuple(items, Span::new(left.file_id(), left.start(), end)));
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
            let arm = self.match_arm()?;
            if !self.matches(TokenKind::Comma) && !self.check(TokenKind::RightBrace) && !is_block_like(&arm.body) {
                self.missing_separator("PACO-E0112", ",", "expected `,` after a match arm");
            }
            arms.push(arm);
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

    fn select_expr(&mut self) -> ParseResult<Expr> {
        let start = self.previous().span;
        self.consume(TokenKind::LeftBrace, "expected `{` after `select`")?;
        let mut arms = Vec::new();
        let mut default = None;
        while !self.check(TokenKind::RightBrace) && !self.check(TokenKind::Eof) {
            if self.check(TokenKind::Identifier)
                && self.peek().lexeme == "default"
                && self.peek_next().is_some_and(|t| t.kind == TokenKind::FatArrow)
            {
                self.advance();
                self.advance();
                let body_expr = self.expr()?;
                let block_like = is_block_like(&body_expr);
                default = Some(self.expr_to_block(body_expr));
                if !self.matches(TokenKind::Comma) && !self.check(TokenKind::RightBrace) && !block_like {
                    self.missing_separator("PACO-E0112", ",", "expected `,` after a select arm");
                }
                continue;
            }
            let arm = self.select_arm()?;
            let block_like = arm.body.tail.as_deref().is_some_and(is_block_like);
            if !self.matches(TokenKind::Comma) && !self.check(TokenKind::RightBrace) && !block_like {
                self.missing_separator("PACO-E0112", ",", "expected `,` after a select arm");
            }
            arms.push(arm);
        }
        let right = self.consume(TokenKind::RightBrace, "expected `}` after select arms")?;
        Ok(Expr::Select {
            arms,
            default,
            span: Span::new(start.file_id(), start.start(), right.span.end()),
        })
    }

    fn select_arm(&mut self) -> ParseResult<SelectArm> {
        let start = self.peek().span.start();
        let operation = if self.check(TokenKind::Identifier)
            && self.peek_next().is_some_and(|t| t.kind == TokenKind::Equal)
        {
            let name = self.advance().clone();
            self.advance();
            let channel_expr = self.expr_without_struct_literals()?;
            let span = Span::new(
                name.span.file_id(),
                name.span.start(),
                expr_span(&channel_expr).end(),
            );
            Expr::Assign {
                target: Box::new(Expr::Ident(name.lexeme.clone(), name.span)),
                value: Box::new(channel_expr),
                span,
            }
        } else {
            self.expr_without_struct_literals()?
        };
        self.consume(TokenKind::FatArrow, "expected `=>` in select arm")?;
        let body_expr = self.expr()?;
        let end = expr_span(&body_expr).end();
        let body = self.expr_to_block(body_expr);
        Ok(SelectArm {
            operation,
            body,
            span: Span::new(self.previous().span.file_id(), start, end),
        })
    }

    fn expr_to_block(&self, expr: Expr) -> Block {
        let span = expr_span(&expr);
        Block {
            stmts: Vec::new(),
            tail: Some(Box::new(expr)),
            span,
        }
    }

    fn closure(&mut self) -> ParseResult<Expr> {
        let start = self.peek().span;
        let mut params = Vec::new();
        if !self.matches(TokenKind::OrOr) {
            self.consume(TokenKind::Pipe, "expected `|` to open closure parameters")?;
            while !self.check(TokenKind::Pipe) {
                let pattern = self.pattern_atom()?;
                let ty = if self.matches(TokenKind::Colon) { Some(self.ty()?) } else { None };
                let span = Span::new(start.file_id(), pat_span(&pattern).start(), self.previous().span.end());
                params.push(ClosureParam { pattern, ty, span });
                if !self.matches(TokenKind::Comma) {
                    break;
                }
            }
            self.consume(TokenKind::Pipe, "expected `|` after closure parameters")?;
        }
        let body = if self.check(TokenKind::LeftBrace) {
            Expr::Block(Box::new(self.block()?))
        } else {
            self.expr()?
        };
        let span = Span::new(start.file_id(), start.start(), expr_span(&body).end());
        Ok(Expr::Closure {
            params,
            body: Box::new(body),
            span,
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
        if self.matches(TokenKind::Char) {
            let token = self.previous();
            return Ok(Pat::Literal(Literal::Char(decode_char(&token.lexeme)), token.span));
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
            if self.matches(TokenKind::LeftBrace) {
                return self.struct_pattern(path, start);
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

    fn struct_pattern(&mut self, path: Vec<String>, start: usize) -> ParseResult<Pat> {
        let mut fields = Vec::new();
        let mut rest = false;
        while !self.check(TokenKind::RightBrace) {
            if self.matches(TokenKind::DotDot) {
                rest = true;
                break;
            }
            let name = self.consume(TokenKind::Identifier, "expected field name in struct pattern")?.clone();
            let pattern = if self.matches(TokenKind::Colon) {
                self.pattern()?
            } else {
                Pat::Ident(name.lexeme.clone(), name.span)
            };
            fields.push((name.lexeme, pattern));
            if !self.matches(TokenKind::Comma) {
                break;
            }
        }
        let right = self.consume(TokenKind::RightBrace, "expected `}` after struct pattern fields")?;
        Ok(Pat::Struct {
            path,
            fields,
            rest,
            span: Span::new(right.span.file_id(), start, right.span.end()),
        })
    }

    fn expr_type_path(&mut self, token: &Token) -> ParseResult<Ty> {
        let path = vec![token.lexeme.clone()];
        if self.starts_generic_type_application() {
            let (args, end) = self.generic_type_args()?;
            Ok(Ty::Generic {
                path,
                args,
                span: Span::new(token.span.file_id(), token.span.start(), end),
            })
        } else {
            Ok(Ty::Path(path, token.span))
        }
    }

    fn generic_type_args(&mut self) -> ParseResult<(Vec<Ty>, usize)> {
        self.consume(TokenKind::Less, "expected `<` before generic type arguments")?;
        let mut args = Vec::new();
        if !self.check(TokenKind::Greater) {
            loop {
                args.push(self.generic_arg()?);
                if !self.matches(TokenKind::Comma) {
                    break;
                }
            }
        }
        let end = self.consume(TokenKind::Greater, "expected `>` after generic type arguments")?.span.end();
        Ok((args, end))
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
                        return matches!(
                            next.kind,
                            TokenKind::LeftBrace | TokenKind::ColonColon | TokenKind::LeftParen
                        );
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
            let body = self.block()?;
            return Ok(for_iter_expr(start, name, range_start, body));
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

    fn consume_generic_parameter(&mut self) -> ParseResult<GenericParam> {
        let start = self.peek().span;
        if self.matches(TokenKind::Const) {
            let name = self.consume_identifier("expected const parameter name after `const`")?;
            self.consume(TokenKind::Colon, "expected `:` and a type after a const parameter name")?;
            let ty = self.ty()?;
            let pack = self.matches(TokenKind::DotDotDot);
            let span = Span::new(start.file_id(), start.start(), self.previous().span.end());
            let kind = if pack { GenericParamKind::ConstPack(ty) } else { GenericParamKind::Const(ty) };
            return Ok(GenericParam { name, kind, bounds: Vec::new(), span });
        }
        if self.peek().kind == TokenKind::Identifier
            && self.peek().lexeme == "dim"
            && self.peek_next().is_some_and(|next| next.kind == TokenKind::Identifier)
        {
            self.advance();
            let name = self.consume_identifier("expected dimension parameter name after `dim`")?;
            let span = Span::new(start.file_id(), start.start(), self.previous().span.end());
            return Ok(GenericParam { name, kind: GenericParamKind::Dim, bounds: Vec::new(), span });
        }
        if self.matches(TokenKind::Lifetime) {
            let name = self.previous().lexeme.trim_start_matches('\'').to_string();
            return Ok(GenericParam { name, kind: GenericParamKind::Lifetime, bounds: Vec::new(), span: self.previous().span });
        }
        if self.matches(TokenKind::Identifier) {
            let name = self.previous().lexeme.clone();
            let mut bounds = Vec::new();
            if self.matches(TokenKind::Colon) {
                loop {
                    bounds.push(self.ty()?);
                    if !self.matches(TokenKind::Plus) {
                        break;
                    }
                }
            }
            let span = Span::new(start.file_id(), start.start(), self.previous().span.end());
            return Ok(GenericParam { name, kind: GenericParamKind::Type, bounds, span });
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

    fn peek_next(&self) -> Option<&Token> {
        self.tokens.get(self.current + 1)
    }

    fn previous(&self) -> &Token {
        &self.tokens[self.current.saturating_sub(1)]
    }

    fn starts_block_like(&self) -> bool {
        matches!(
            self.peek().kind,
            TokenKind::If
                | TokenKind::Match
                | TokenKind::While
                | TokenKind::For
                | TokenKind::Loop
                | TokenKind::Unsafe
                | TokenKind::Comptime
                | TokenKind::Select
                | TokenKind::LeftBrace
        )
    }

    fn expect_semicolon(&mut self) {
        if !self.matches(TokenKind::Semicolon) {
            self.missing_separator("PACO-E0115", ";", "expected `;` after this statement");
        }
    }

    fn missing_separator(&mut self, code: &str, separator: &str, message: &str) {
        let end = self.previous().span;
        let at = Span::new(end.file_id(), end.end(), end.end());
        self.reporter.push(
            Diagnostic::error(code, at, message)
                .with_secondary(
                    self.peek().span,
                    if separator == ";" { "the next statement starts here" } else { "the next member starts here" },
                )
                .with_suggestion(Suggestion::new(at, separator, format!("insert `{separator}` here"))),
        );
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

    fn synchronize_trait_member(&mut self) {
        while !self.check(TokenKind::Eof)
            && !self.check(TokenKind::RightBrace)
            && !matches!(
                self.peek().kind,
                TokenKind::Fn | TokenKind::Type | TokenKind::Const
            )
        {
            self.advance();
        }
    }
}

pub fn is_block_like(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Block(_)
            | Expr::Unsafe(..)
            | Expr::If { .. }
            | Expr::Loop { .. }
            | Expr::While { .. }
            | Expr::Match { .. }
            | Expr::Select { .. }
            | Expr::Comptime { .. }
    )
}

pub fn expr_span(expr: &Expr) -> Span {
    match expr {
        Expr::Literal(_, span)
        | Expr::Ident(_, span)
        | Expr::Return(_, span)
        | Expr::Break(_, span)
        | Expr::Continue(span)
        | Expr::Unsafe(_, span)
        | Expr::Quote(_, span)
        | Expr::Splice(_, span)
        | Expr::Yield(_, span)
        | Expr::Tuple(_, span) => *span,
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
        | Expr::Closure { span, .. }
        | Expr::Select { span, .. }
        | Expr::Comptime { span, .. }
        | Expr::StructLiteral { span, .. }
        | Expr::Borrow { span, .. }
        | Expr::Try { span, .. }
        | Expr::Cast { span, .. } => *span,
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

fn for_iter_expr(start: Span, name: (String, Span), iterable: Expr, body: Block) -> Expr {
    let (name, name_span) = name;
    let cursor_name = format!("$paco_for_iter_{}", start.start());
    let end = body.span.end();
    let full_span = Span::new(start.file_id(), start.start(), end);

    let next_call = Expr::MethodCall {
        receiver: Box::new(Expr::Ident(cursor_name.clone(), name_span)),
        method: "next".to_string(),
        args: Vec::new(),
        span: full_span,
    };
    let some_arm = MatchArm {
        pattern: Pat::Enum {
            path: vec!["Option".to_string(), "Some".to_string()],
            fields: vec![Pat::Ident(name, name_span)],
            span: name_span,
        },
        guard: None,
        body: Expr::Block(Box::new(body)),
        span: full_span,
    };
    let none_arm = MatchArm {
        pattern: Pat::Enum {
            path: vec!["Option".to_string(), "None".to_string()],
            fields: Vec::new(),
            span: full_span,
        },
        guard: None,
        body: Expr::Break(None, full_span),
        span: full_span,
    };
    let match_expr = Expr::Match {
        scrutinee: Box::new(next_call),
        arms: vec![some_arm, none_arm],
        span: full_span,
    };
    let loop_body = Block {
        stmts: vec![Stmt::Expr(match_expr)],
        tail: None,
        span: full_span,
    };
    let loop_expr = Expr::Loop { body: loop_body, span: full_span };

    Expr::Block(Box::new(Block {
        stmts: vec![
            Stmt::Let(LetStmt {
                mutable: true,
                pattern: Pat::Ident(cursor_name, name_span),
                ty: None,
                value: Some(iterable),
                span: full_span,
            }),
            Stmt::Expr(loop_expr),
        ],
        tail: None,
        span: full_span,
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
        Expr::Unsafe(block, _) => rewrite_continue_in_block(block, increment),
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
        | Expr::AssociatedCall { args, .. }
        | Expr::Tuple(args, _) => {
            for arg in args {
                rewrite_continue_in_expr(arg, increment);
            }
        }
        Expr::StructLiteral { fields, .. } => {
            for (_, value) in fields {
                rewrite_continue_in_expr(value, increment);
            }
        }
        Expr::Binary { left, right, .. } => {
            rewrite_continue_in_expr(left, increment);
            rewrite_continue_in_expr(right, increment);
        }
        Expr::Index { base, index, .. } => {
            rewrite_continue_in_expr(base, increment);
            for index_expr in index {
                rewrite_continue_in_expr(index_expr, increment);
            }
        }
        Expr::Unary { expr, .. }
        | Expr::Field { base: expr, .. }
        | Expr::Try { expr, .. }
        | Expr::Cast { expr, .. } => {
            rewrite_continue_in_expr(expr, increment);
        }
        Expr::Loop { .. }
        | Expr::While { .. }
        | Expr::Literal(_, _)
        | Expr::Ident(_, _)
        | Expr::Return(None, _)
        | Expr::Break(None, _)
        | Expr::Spawn { .. }
        | Expr::Closure { .. }
        | Expr::Select { .. }
        | Expr::Comptime { .. }
        | Expr::Quote(..)
        | Expr::Splice(..) => {}
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
        | Ty::Const(_, span)
        | Ty::DynDim(span)
        | Ty::Expand(_, span)
        | Ty::Existential(_, span)
        | Ty::Borrow { span, .. }
        | Ty::RawPointer { span, .. }
        | Ty::Splice(_, span) => *span,
    }
}

fn decode_char(source: &str) -> char {
    let inner = source
        .strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''))
        .unwrap_or(source);
    let mut chars = inner.chars();
    match chars.next() {
        Some('\\') => match chars.next() {
            Some('n') => '\n',
            Some('t') => '\t',
            Some('r') => '\r',
            Some('\'') => '\'',
            Some('\\') => '\\',
            Some(other) => other,
            None => '\\',
        },
        Some(ch) => ch,
        None => '\0',
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
