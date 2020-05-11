package parser

import (
	"fmt"
	"testing"
)

func TestParse(t *testing.T) {
	code := `mod "console"

fn println(string)
fn print(string)
fn getStringEntry() string
fn getIntEntry() int`

	parser := Parse(code)
	for {
		node := <-parser.NodesChannel

		if node.Type == EOF {
			break
		}

		fmt.Println(node)
	}
}
