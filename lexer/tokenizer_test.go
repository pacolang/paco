package lexer

import (
	"testing"
)

func TestTokenize(t *testing.T) {
	code := "includes ('console') fn hello() string 'hello world' end "
	tokens := Tokenize(code)
	excepted := []string{
		Keyword, Symbol, Symbol, Name, Symbol, Symbol, Keyword, Name,
		Symbol, Symbol, Keyword, Symbol, Name, Name, Symbol, Keyword,
	}

	for i, token := range excepted {
		if tokens[i].Type != token {
			t.Errorf("Tokenize() failed, excepted %s got %s.", token, tokens[i])
		}
	}
}
