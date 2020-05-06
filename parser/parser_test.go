package parser

import (
	"fmt"
	"testing"
)

func TestParse(t *testing.T) {
	code := `fn hello(val int) string
console|println("hey")
end`

	parser := Parse(code)
	for {
		node := <-parser.NodesChannel

		if node.Type == EOF {
			break
		}

		fmt.Println(node)
	}
}
