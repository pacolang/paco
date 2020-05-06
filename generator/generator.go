package generator

import (
	"github.com/hugolgst/paco/parser"
)

// The Generator will take the parsers's nodes to generate the C code with it
type Generator struct {
	nodesChannel chan parser.Node
	imports      []string
	mainCalls    []string
	functions    []string
}

// Generate takes the code and generates the matching C code
func Generate(input string) {
	generator := &Generator{
		nodesChannel: parser.Parse(input).NodesChannel,
	}

	generator.run()
}

// run waits for the parser's nodes and translate them to C
func (generator *Generator) run() {
	for {
		node := <-generator.nodesChannel

		// Breaks the infinite loop if it is the last node
		if node.Type == parser.EOF {
			break
		}

		switch node.Type {
		case parser.CallExpression:

		}
	}
}
