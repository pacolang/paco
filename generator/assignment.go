package generator

import (
	"fmt"
	"github.com/hugolgst/paco/log"
	"github.com/hugolgst/paco/parser"
)

// generateAssignment translates a variable assignment to C
func generateAssignment(generator *Generator, node parser.Node) string {
	cType := cTypes[node.Params[0].Type]
	if node.Params[0].Type == parser.CallExpression {
		cType = functions[node.Params[0].Value]
	}

	if cType == "void" {
		log.Errorf("Using a function as a value but it returns nothing")
	}

	return fmt.Sprintf(cAssignment, cType, node.Value, generateInstruction(generator, node.Params[0]))
}

// generateEmptyAssignment translates an empty variable assignment to C
func generateEmptyAssignment(node parser.Node) string {
	cType := cTypes[node.ReturnType]

	return fmt.Sprintf(cEmptyAssignment, cType, node.Value)
}