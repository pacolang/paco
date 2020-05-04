package lexer

import (
	"fmt"
	"testing"
)

func TestLex(t *testing.T) {
	_, channel := Lex(`hello|print = "jgk\"eaz"`)

	for {
		item := <-channel

		fmt.Println(item)

		if item.Type == itemEOF {
			break
		}
	}
}
