package parser

import (
	"fmt"
	"testing"
)

func TestParse(t *testing.T) {
	code := `fn getUserName() string
    name string
    stdio|scanf("%s" *name)

    *name
end

console|println("Enter your name")
name = getUserName()`

	parser := Parse(code)
	for {
		node := <-parser.NodesChannel

		if node.Type == EOF {
			break
		}

		fmt.Println(node)
	}
}
