package parser

import (
	"github.com/hugolgst/paco/lexer"
	"github.com/hugolgst/paco/log"
)

// parseCall parses a function call and returns its node
func parseCall(parser *Parser, identifier string) Node {
	var params []Node

	// While the item is a right parentheses parses the params
	for item := parser.next(); item.Type != lexer.ItemRightParentheses; {
		params = append(params, parser.parseItem(item))

		item = parser.next()
	}

	return Node{
		Type:   CallExpression,
		Value:  identifier,
		Params: params,
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
		node.Body = append(node.Body, parser.parseItem(item))
		item = parser.next()

		if item.Type == lexer.ItemEOF {
			log.Errorf("end was not found")
		}
	}

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
