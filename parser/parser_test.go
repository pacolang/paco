package parser

import (
	"fmt"
	"testing"
)

func TestParse(t *testing.T) {
	code := `if *number >= 0 and *number <= 6
    console|println("the number must me less than 0")
else
	console|print("hey")
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
