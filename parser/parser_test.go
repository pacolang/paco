package parser

import (
	"fmt"
	"testing"
)

func TestParse(t *testing.T) {
	code := `console|println("hello")`

	fmt.Println(Parse(code).nodes)
}
