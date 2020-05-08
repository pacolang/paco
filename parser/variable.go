package parser

import "github.com/hugolgst/paco/lexer"

// parseAssignment parses a variable assignment with the given identifier
func parseAssignment(parser *Parser, identifier string) Node {
	return Node{
		Type: Assignment,
		Value: identifier,
		Params: []Node{
			parseItem(parser, parser.next()),
		},
	}
}

// parseEmptyAssignment parses a empty variable initialisation with the given identifier and
// the given item type
func parseEmptyAssignment(identifier string, itemType lexer.ItemType) Node {
	return Node{
		Type: EmptyAssignment,
		Value: identifier,
		ReturnType: types[itemType],
	}
}