package generator

import (
	"fmt"
	"github.com/hugolgst/paco/parser"
)

func generateCondition(generator *Generator, node parser.Node) string {
	firstValue := generateInstruction(generator, node.Params[0])
	secondValue := generateInstruction(generator, node.Params[1])

	condition := firstValue+node.Value+secondValue
	if node.Params[0].Type == parser.StringLiteral ||
		node.Params[1].Type == parser.StringLiteral {
		generator.addImport("string")
		condition = fmt.Sprintf("strcmp(%s,%s)==0", firstValue, secondValue)
	}

	return fmt.Sprintf(cCondition, condition, generateFunctionBody(generator, node))
}
