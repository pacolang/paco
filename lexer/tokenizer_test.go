package lexer

import (
	"fmt"
	"testing"
)

func TestTokenize(t *testing.T) {
	code := "includes (\n  'console'\n)\n\nfn hello() string\n  'hello world'\nend\n\nhello = 1329\n\n- prints 1329 and prints my name\nconsole\n  |println(hello)\n  |printf('hello %s!', 'hugo')"

	fmt.Println(Tokenize(code))
}
