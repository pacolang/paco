package generator

import "github.com/hugolgst/paco/parser"

// generateInstruction returns the translated string for the given node
func generateInstruction(generator *Generator, node parser.Node) string {
	switch node.Type {
	case parser.CallExpression:
		return generateCall(generator, node)
	}

	return ""
}
