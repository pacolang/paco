package generator

import (
	"fmt"

	"github.com/pacolang/paco/log"
	"github.com/pacolang/paco/parser"
)

// generateAssignment translates a variable assignment to C
func generateAssignment(generator *Generator, node parser.Node) string {
	cType := cTypes[node.Params[0].ReturnType]

	// The used function does not exists
	if cType == "" {
		log.Errorf("%s function call does not exists or returns nothing.", node.Value)
	}

	return fmt.Sprintf(cAssignment, cType, node.Value, generateInstruction(generator, node.Params[0]))
}

// generateEmptyAssignment translates an empty variable assignment to C
func generateEmptyAssignment(node parser.Node) string {
	cType := cTypes[node.ReturnType]

	return fmt.Sprintf(cEmptyAssignment, cType, node.Value)
}
