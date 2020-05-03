package lexer

import "testing"

func TestLex(t *testing.T) {
	Lex("console\n  |println(hello)\n  |printf('hello %s!', 'hugo')")
}
