package lexer

import (
	"fmt"
	"testing"
)

func TestLex(t *testing.T) {
	_, channel := Lex(`fn hello()
  hey = "yo"
  console|println(*hey)
end

hello()`)

	for {
		item := <-channel

		fmt.Println(item)

		if item.Type == ItemEOF {
			break
		}
	}
}
