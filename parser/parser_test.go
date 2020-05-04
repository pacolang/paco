package parser

import (
	"fmt"
	"testing"
)

func TestParse(t *testing.T) {
	code := `console|println("hello")
- hey how are you
hey = 9`

	fmt.Println(Parse(code).nodes)
}
