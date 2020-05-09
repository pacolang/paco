package generator

import "github.com/hugolgst/paco/parser"

// generateInstruction returns the translated string for the given node
func generateInstruction(generator *Generator, node parser.Node) string {
	switch node.Type {
	case parser.CallExpression:
		return generateCall(generator, node)

	case parser.StringLiteral:
		return node.Value

	case parser.NumberLiteral:
		return node.Value

	case parser.Assignment:
		return generateAssignment(generator, node)

	case parser.EmptyAssignment:
		return generateEmptyAssignment(node)

	case parser.Variable:
		return node.Value

	case parser.Condition:
		return generateCondition(generator, node)
	}

	return ""
}
