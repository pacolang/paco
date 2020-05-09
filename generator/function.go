package generator

import (
	"fmt"
	"github.com/hugolgst/paco/parser"
	"strings"
)

// generateFunction returns the translation of the given function node in C
func generateFunction(generator *Generator, node parser.Node) string {
	params := generateFunctionParams(node.Params)
	body := generateFunctionBody(generator, node)
	functionType := generateFunctionType(node.ReturnType)

	// Register function type
	functions[node.Value] = functionType

	return fmt.Sprintf(
		cFunction,
		functionType,
		node.Value,
		strings.Join(params, ","),
		body,
	)
}

// generateFunctionType returns the C type of the given node type
func generateFunctionType(functionType parser.NodeType) string {
	cType := cTypes[functionType]
	// If the type is empty, then it is a void function
	if cType == "" {
		cType = "void"
	}

	return cType
}

// generateFunctionBody returns the C instructions for the body of the given node
func generateFunctionBody(generator *Generator, node parser.Node) (bodyInstructions string) {
	for i, argument := range node.Body {
		// Generate the instruction and makes it a return instruction if it is the last element and that
		// the return type isn't void
		instruction := generateInstruction(generator, argument)
		if i == len(node.Body)-1 && cTypes[node.ReturnType] != "" {
			instruction = fmt.Sprintf(cReturn, instruction)
		}

		bodyInstructions += instruction + ";"
	}

	return
}

// generateFunctionParams returns the translated params in C from the given array of node params
func generateFunctionParams(params []parser.Node) (cParams []string) {
	// Iterate through the given parameters to translate them to C
	for _, param := range params {
		// Retrieve the C type of the current parameter
		paramType := cTypes[param.ReturnType]

		// Append the translated parameter to the return array
		cParams = append(
			cParams,
			fmt.Sprintf(cParam, paramType, param.Value),
		)
	}

	return
}
