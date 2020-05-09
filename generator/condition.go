package generator

import (
	"fmt"
	"github.com/hugolgst/paco/parser"
)

// generateCondition generates the differents booleans and put them together with the body
func generateCondition(generator *Generator, node parser.Node) (statement string) {
	boolean := generateBoolean(generator, node.Params[0])

	// If the condition contains and/or operators
	if len(node.Params) > 1 {
		for i := 1; i < len(node.Params); i += 2 {
			boolean += node.Params[i].Value + generateBoolean(generator, node.Params[i+1])
		}
	}

	statement = fmt.Sprintf(
		cCondition,
		boolean,
		generateConditionBody(generator, node.Body[0]),
	)
	// Returns if there isn't an else statement
	if len(node.Body) == 1 {
		return
	}

	statement += fmt.Sprintf(
		cElseCondition,
		generateConditionBody(generator, node.Body[1]),
	)

	return
}

// generateBoolean generates the C code for a boolean
func generateBoolean(generator *Generator, node parser.Node) string {
	firstValue := generateInstruction(generator, node.Params[0])
	secondValue := generateInstruction(generator, node.Params[1])

	condition := firstValue+node.Value+secondValue
	// Use strcmp for string comparison
	if node.Params[0].Type == parser.StringLiteral ||
		node.Params[1].Type == parser.StringLiteral {
		generator.addImport("string")
		condition = fmt.Sprintf("strcmp(%s,%s)==0", firstValue, secondValue)
	}

	return condition
}

// generateConditionBody generates the instructions contained inside a condition
func generateConditionBody(generator *Generator, node parser.Node) (instructions string) {
	for _, argument := range node.Body {
		instructions += generateInstruction(generator, argument) + ";"
	}

	return instructions
}