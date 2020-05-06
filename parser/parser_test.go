package parser

import (
	"fmt"
	"testing"
)

func TestParse(t *testing.T) {
	code := `console
	|println("hey")
	|println("hello")`

	parser := Parse(code)
	for {
		node := <-parser.NodesChannel

		if node.Type == EOF {
			break
		}

		fmt.Println(node)
	}
}
