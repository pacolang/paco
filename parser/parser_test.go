package parser

import (
	"fmt"
	"testing"
)

func TestParse(t *testing.T) {
	code := `token = true
if *token == true
  console|println("hello")
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
