package generator

import (
	"fmt"
	"github.com/hugolgst/paco/parser"
)

func generateAssignment(node parser.Node) string {
	cType := cTypes[node.Params[0].Type]

	return fmt.Sprintf(cAssignment, cType, node.Value, node.Params[0].Value)
}
