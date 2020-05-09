package lexer

import (
	"fmt"
	"testing"
)

func TestLex(t *testing.T) {
	_, channel := Lex(`
if *text < 0
    console|println("bingo")
end`)

	for {
		item := <-channel

		fmt.Println(item)

		if item.Type == ItemEOF {
			break
		}
	}
}
