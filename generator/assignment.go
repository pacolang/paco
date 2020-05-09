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

	// If the function used returns nothing then returns the error
	if cType == "void" {
		log.Errorf("using %s as a value but it returns nothing.", node.Value)
	}

	// The used function does not exists
	if cType == "" {
		log.Errorf("%s function call does not exists.", node.Value)
	}

	return fmt.Sprintf(cAssignment, cType, node.Value, generateInstruction(generator, node.Params[0]))
}

// generateEmptyAssignment translates an empty variable assignment to C
func generateEmptyAssignment(node parser.Node) string {
	cType := cTypes[node.ReturnType]

	return fmt.Sprintf(cEmptyAssignment, cType, node.Value)
}