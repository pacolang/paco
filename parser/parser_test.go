package parser

import (
	"fmt"
	"testing"
)

func TestParse(t *testing.T) {
	code := `fn hello(val int) string
console|println("hey")
end`

	fmt.Println(Parse(code).Nodes)
}
