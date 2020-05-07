package generator

import (
	"fmt"
	"github.com/hugolgst/paco/parser"
	"strings"
)

// The Generator will take the parsers's nodes to generate the C code with it
type Generator struct {
	nodesChannel  chan parser.Node
	previousNodes []parser.Node
	imports       []string
	mainCalls     []string
	functions     []string
}

// Generate takes the code and generates the matching C code
func Generate(input string) string {
	generator := &Generator{
		nodesChannel: parser.Parse(input).NodesChannel,
	}

	generator.run()
	return generator.assemble()
}

// next returns the next node sent by the parser
func (generator *Generator) next() parser.Node {
	node := <-generator.nodesChannel
	generator.previousNodes = append(generator.previousNodes, node)

	return node
}

// addMainCall appends the given string to the main calls array of the generator
func (generator *Generator) addMainCall(call string) {
	generator.mainCalls = append(generator.mainCalls, call)
}

// addFunction appends the given function to the function's array of the generator
func (generator *Generator) addFunction(function string) {
	generator.functions = append(generator.functions, function)
}

// addImport appends the given import to the imports array of the generator
func (generator *Generator) addImport(importName string) {
	// Return if the import already exists
	for _, imp := range generator.imports {
		if imp != importName {
			continue
		}

		return
	}

	generator.imports = append(
		generator.imports,
		fmt.Sprintf(cImports, importName),
	)
}

// run waits for the parser's nodes and translate them to C
func (generator *Generator) run() {
	for {
		node := generator.next()

		// Breaks the infinite loop if it is the last node
		if node.Type == parser.EOF {
			break
		}

		switch node.Type {
		case parser.CallExpression:
			generator.addMainCall(generateCall(generator, node))
			break
		case parser.Function:
			generator.addFunction(generateFunction(generator, node))
			break
		}
	}
}

// assemble brings all the parts of the code together
func (generator *Generator) assemble() string {
	return fmt.Sprintf(
		cCode,
		strings.Join(generator.imports, "\n"),
		strings.Join(generator.functions, "\n"),
		strings.Join(generator.mainCalls, ";"),
	)
}