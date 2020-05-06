package generator

import (
	"fmt"
	"github.com/hugolgst/paco/log"
	"github.com/hugolgst/paco/parser"
	"strings"
)

// generateCall returns the C code for the node call expression
func generateCall(generator *Generator, node parser.Node) string {
	var identifier string

	// Check if it is a built-in function or not
	if strings.Contains(node.Value, "|") {
		// Get the function identifier by spliting the value by the pipe
		identifier = strings.Split(node.Value, "|")[1]

		checkCall(generator, node)

		// Add import to the generator
		addCallImport(generator, node.Value)
	} else {
		identifier = node.Value
	}

	// Translate the params
	params := generateParams(node.Params)

	// Link all the translations together
	return fmt.Sprintf(
		cCall,
		identifier,
		strings.Join(params, ","),
	)
}

// generateParams returns the parameters translated in C from the given nodes parameters
func generateParams(params []parser.Node) (cParams []string) {
	// Translate each parameter
	for _, param := range params {
		// Append the translated parameter in C
		cParams = append(
			cParams,
			param.Value,
		)
	}

	return
}

// checkCall checks if a pipe call expression relies on a package def
func checkCall(generator *Generator, node parser.Node) {
	if strings.HasPrefix(node.Value, "|") {
		// Get the previous node's import
		previousNode := generator.previousNodes[len(generator.previousNodes)-1]

		if previousNode.Type != parser.CallExpression {
			log.Errorf("cannot find package")
		}
	}
}

// addCallImport adds the package of the given value
func addCallImport(generator *Generator, value string) {
	importName := strings.Split(value, "|")[0]
	if importName == "" {
		return
	}

	generator.addImport(importName)
}