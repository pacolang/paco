package parser

import (
	"github.com/pacolang/paco/lexer"
)

var variables = map[string]NodeType{}

// parseAssignment parses a variable assignment with the given identifier
func parseAssignment(parser *Parser, identifier string) Node {
	node := parseItem(parser, parser.next())
	variables[identifier] = node.ReturnType

	return Node{
		Type:  Assignment,
		Value: identifier,
		Params: []Node{
			node,
		},
		ReturnType: node.ReturnType,
	}
}

// parseEmptyAssignment parses a empty variable initialisation with the given identifier and
// the given item type
func parseEmptyAssignment(identifier string, itemType lexer.ItemType) Node {
	return Node{
		Type:       EmptyAssignment,
		Value:      identifier,
		ReturnType: types[itemType],
	}
}
