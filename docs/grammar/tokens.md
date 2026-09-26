# Paco lexical grammar (tokens)

> Status: **stable** — all design decisions settled. This file is the normative
> lexical reference. The syntactic grammar lives in `docs/grammar/grammar.ebnf`.

## Keywords

```
as        break     comptime  const     continue  default
dyn       else      enum      extern    false     fn
for       if        in        iter      let       loop
match     methods   module    mut       pub       return
select    self      spawn     struct    trait     true
type      unsafe    use       where     while     yield
```

`self` is a keyword, not an identifier: it appears only as a method receiver
(`self`, `&self`, `&mut self`) and as the receiver expression inside a method body.

## Primitive types (reserved identifiers)

```
i8   i16  i32  i64
u8   u16  u32  u64
int  uint
f16  f32  f64  bf16
f8e4m3   f8e5m2
bool  char  string  byte
```

### Floating-point types

| Type | Layout | Role |
|------|--------|------|
| `f32`, `f64` | IEEE 754 binary32 / binary64 | General computation. |
| `f16` | IEEE 754 binary16 (1-5-10) | Reduced-precision compute. |
| `bf16` | bfloat16 (1-8-7) | Reduced-precision compute; `f32` exponent range. |
| `f8e4m3`, `f8e5m2` | OCP FP8 (1-4-3 / 1-5-2) | **Storage and interchange only** — no arithmetic operators. |

`f16` and `bf16` are full arithmetic types. `f8e4m3` and `f8e5m2` carry no
arithmetic operators at all: convert them with `as` to a wider float to compute.
This keeps the cost visible — an FP8 tensor is a storage decision, not a silent
change to how arithmetic behaves.

There is **no implicit conversion between any two floating-point types**, in
either direction. Widening is as explicit as narrowing (`x as f32`). See §9 of
`docs/design/spec.md`.

## Literals

| Kind | Examples |
|------|---------|
| Integer (decimal) | `42`, `1_000`, `1_000_000` |
| Integer (hex) | `0xFF`, `0xDEAD_BEEF` |
| Integer (binary) | `0b1010`, `0b1111_0000` |
| Float | `3.14`, `1.0e9`, `2.5e-3` |
| String | `"UTF-8 text"` — escapes: `\n \t \r \" \\` |
| Char | `'a'`, `'\n'`, `'\\'` |
| Bool | `true`, `false` |

## Identifiers

Pattern: `[A-Za-z_][A-Za-z0-9_]*`

An identifier that matches a keyword is a keyword, not an identifier. Primitive
type names (`int`, `string`, etc.) are reserved and may not be used as
identifiers.

UTF-8 identifiers (non-ASCII letters): to be decided in a future revision.

## Lifetimes

A `'` followed immediately by an identifier: `'a`, `'static`.

## Operators

```
+   -   *   /   %           arithmetic
==  !=  <   <=  >   >=      comparison
&&  ||  !                   logical
&   |   ^   <<  >>  ~       bitwise
=   +=  -=  *=  /=  %=      assignment
&   &mut                    borrow (& is shared, &mut is mutable)
*const  *mut                raw pointer types (FFI only, unsafe to dereference)
?                           error / absence propagation
.   ..   ..=                field access / range
...                         const parameter pack (const D: int...)
->  =>                      return type arrow / match arm arrow
::                          module path / associated item separator
as                          type cast (also a keyword)
@                           pattern binding only (n @ 1..=9)
#   #[  ]                   attribute (#[test], #[derive(Clone)])
```

### Compound tokens

`&mut` is lexed as a single compound token distinct from `&` followed by `mut`.
The lexer produces `&mut` whenever `&` is immediately followed by `mut` with no
intervening whitespace or comment. This avoids ambiguity in borrow expressions
and receiver declarations.

## Delimiters

```
(  )    parentheses
{  }    braces
[  ]    brackets
,       comma
;       semicolon (statement terminator; newlines are whitespace)
:       colon (type annotation)
```

## Comments

```
// Line comment — extends to end of line
/* Block comment — may span multiple lines, does NOT nest */
/// Doc comment — above a declaration; content is Markdown
```

## Attributes

`#[` followed by an identifier, optionally with a parenthesised argument list, then `]`:

```
#[test]
#[bench]
#[should_panic]
#[derive(Display, Clone, Eq)]
#[allow(unhandled_result)]
#[repr(C)]
```

`@` is **not** an attribute sigil. It means pattern binding and nothing else
(`n @ 1..=9`). See RFC 0020.
