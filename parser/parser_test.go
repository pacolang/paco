package parser

import (
	"fmt"
	"testing"
)

func TestParse(t *testing.T) {
	code := `text = "hello"
if *text == "hello"
    console|println("bingo")
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
