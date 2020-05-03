package lexer

import (
	"fmt"
	"testing"
)

func TestTokenize(t *testing.T) {
	code := "console\n  |println(hello)\n  |printf('hello %s!', 'hugo') "

	fmt.Println(Tokenize(code))
}
