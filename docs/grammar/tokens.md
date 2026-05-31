# Paco lexical grammar (tokens)

> Status: **stable** — all design decisions settled. This file is the normative
> lexical reference. The syntactic grammar lives in `docs/grammar/grammar.ebnf`.

## Keywords

```
as        break     comptime  continue  default   dyn
else      enum      false     fn        for       if
in        iter      let       loop      match     methods
mut       return    self      select    spawn     struct
trait     true      type      use       where     while
yield
```

## Primitive types (reserved identifiers)

```
i8   i16  i32  i64
u8   u16  u32  u64
int  uint
f32  f64
bool  char  string  byte
```

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
?                           error / absence propagation
.   ..   ..=                field access / range
->  =>                      return type arrow / match arm arrow
::                          module path / associated item separator
as                          type cast (also a keyword)
@                           attribute prefix / pattern binding (@test, n @ pat)
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
;       semicolon (optional terminator)
:       colon (type annotation)
```

## Comments

```
// Line comment — extends to end of line
/* Block comment — may span multiple lines, does NOT nest */
/// Doc comment — above a declaration; content is Markdown
```

## Attributes

`@` followed by an identifier, optionally with a parenthesised argument list:

```
@test
@bench
@should_panic
@derive(Display, Clone, Eq)
@allow("lint-code")
@repr(C)
```
