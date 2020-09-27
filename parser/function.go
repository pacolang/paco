package parser

import (
	"github.com/pacolang/paco/lexer"
	"github.com/pacolang/paco/log"
	"strings"
)

// parseCall parses a function call and returns its node
func parseCall(parser *Parser, identifier string) Node {
	var params []Node

	// Checks if the given function exists
	var function FunctionRecorder
	for _, fn := range functions {
		if !strings.HasSuffix(fn.Name, identifier) {
			continue
		}

		function = fn
	}

	if function.Name == "" {
		log.Errorf("%s function does not exists", identifier)
	}

	// While the item is a right parentheses parses the params
	var i int
	for item := parser.next(); item.Type != lexer.ItemRightParentheses; {
		if i >= len(function.Params) {
			log.Errorf("too much parameters for %s", identifier)
		}

		node := parseItem(parser, item)
		if node.ReturnType != function.Params[i] && function.Params[i] != Generic {
			log.Errorf("type mismatch in %s call", identifier)
		}

		params = append(params, node)

		item = parser.next()
		i++
	}

	return Node{
		Type:   CallExpression,
		Value:  identifier,
		Params: params,
		ReturnType: function.ReturnType,
	}
}

// parseFunction parses a function definition and returns its node
func parseFunction(parser *Parser) Node {
	// Gets the name of the function
	identifier := parser.next()
	if identifier.Type != lexer.ItemIdentifier {
		log.Errorf("name of the function should be an identifier")
	}

	node := Node{
		Type:  Function,
		Value: identifier.Value,
	}

	item := parser.next()
	if item.Type != lexer.ItemLeftParentheses {
		log.Errorf("the left parentheses is missing")
	}

	item = parser.next()

	// Parse each parameter of the function definition
	for item.Type != lexer.ItemRightParentheses {
		node.Params = append(node.Params, parseParam(parser, item))
		item = parser.next()

		if item.Type == lexer.ItemEOF {
			log.Errorf("the right parentheses is missing")
		}
	}

	// Get the type if there is one
	item = parser.next()
	if item.Type > lexer.ItemTypes {
		node.ReturnType = types[item.Type]
		item = parser.next()
	}

	if item.Type == lexer.ItemEOF {
		log.Errorf("empty function declaration")
	}

	// Add body nodes
	for item.Type != lexer.ItemEnd {
		node.Body = append(node.Body, parseItem(parser, item))
		item = parser.next()

		if item.Type == lexer.ItemEOF {
			log.Errorf("end was not found")
		}
	}

	functions = append(functions, FunctionRecorder{
		Name: identifier.Value,
		ReturnType: node.ReturnType,
	})

	return node
}

// parseParam parses a function parameter and returns its node
func parseParam(parser *Parser, item lexer.Item) Node {
	if item.Type != lexer.ItemIdentifier {
		log.Errorf("name of the param should be an identifier")
	}

	// Get param type
	typ := parser.next()
	if typ.Type < lexer.ItemTypes {
		log.Errorf("param type isn't valid")
	}

	return Node{
		Type:       Parameter,
		Value:      item.Value,
		ReturnType: types[typ.Type],
	}
}
