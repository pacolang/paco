package lexer

import (
	"fmt"
	"testing"
)

func TestLex(t *testing.T) {
	_, channel := Lex("+12.42")

	for {
		item := <-channel

		fmt.Printf("item recieved: %s\n", item.Value)
		fmt.Printf("item type: %d\n", item.Type)

		if item.Type == itemError {
			break
		}
	}
}
