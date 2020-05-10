package lexer

import (
	"fmt"
	"testing"
)

func TestLex(t *testing.T) {
	_, channel := Lex(`hey=true
	console|println(*hey)`)

	for {
		item := <-channel

		fmt.Println(item)

		if item.Type == ItemEOF {
			break
		}
	}
}
