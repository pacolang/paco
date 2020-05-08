package parser

import (
	"fmt"
	"testing"
)

func TestParse(t *testing.T) {
	code := `fn hello()
  hey = "yo"
  console|println(*hey)
end

hello()`

	parser := Parse(code)
	for {
		node := <-parser.NodesChannel

		if node.Type == EOF {
			break
		}

		fmt.Println(node)
	}
}
