# Paco lexical grammar (tokens)

> Stable enough to implement the lexer. The syntactic grammar (EBNF) waits on the
> resolution of the open syntax frictions (see spec).

## Keywords
let, mut, fn, struct, enum, trait, methods, match, if, else, for, while, loop,
return, spawn, iter, yield, comptime, use, dyn, true, false, self

## Primitive types (reserved identifiers)
i8 i16 i32 i64  u8 u16 u32 u64  int uint  f32 f64  bool  char  string  byte

## Literals
- integer: `42`, `1_000`, `0xFF`, `0b1010`
- float: `3.14`, `1.0e9`
- string: `"UTF-8 text"`, with escapes `\n \t \r \" \\`
- char: `'a'`, `'\n'`
- bool: `true`, `false`

## Identifiers
[A-Za-z_][A-Za-z0-9_]*  (UTF-8 in identifiers: to be decided)

## Lifetimes
`'` followed by an identifier: `'a`, `'static`

## Operators
+  -  *  /  %       arithmetic
== != < <= > >=     comparison
&& || !             logical
& | ^ << >> ~       bitwise
=  += -= *= /= %=   assignment
&  &mut             borrow
?                   error/absence propagation
.  ..  ..=          access / range
->  =>              return arrow / match arm
::                  module/associated path

## Delimiters
( )  { }  [ ]  ,  ;  :

## Comments
`// line`    `/* block */`

## Attributes
`@` followed by an identifier: `@test`, `@bench`, `@derive(...)`, `@allow("...")`
