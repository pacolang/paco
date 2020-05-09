package lexer

import (
	"fmt"
	"testing"
)

func TestLex(t *testing.T) {
	_, channel := Lex(`
if *number >= 0 and *number <= 6
    console|println("the number must me less than 0")
else
	console|print("hey")
end`)

	for {
		item := <-channel

		fmt.Println(item)

		if item.Type == ItemEOF {
			break
		}
	}
}
