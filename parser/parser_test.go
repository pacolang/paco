package parser

import (
	"fmt"
	"testing"
)

func TestParse(t *testing.T) {
	code := `fn hello(val int)`

	fmt.Println(Parse(code).nodes)
}
