package generator

import (
	"fmt"
	"github.com/hugolgst/paco/parser"
)

// generateAssignment translates a variable assignment to C
func generateAssignment(node parser.Node) string {
	cType := cTypes[node.Params[0].Type]

	return fmt.Sprintf(cAssignment, cType, node.Value, node.Params[0].Value)
}

// generateEmptyAssignment translates an empty variable assignment to C
func generateEmptyAssignment(node parser.Node) string {
	cType := cTypes[node.ReturnType]

	return fmt.Sprintf(cEmptyAssignment, cType, node.Value)
}