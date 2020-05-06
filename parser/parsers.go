package parser

import (
	"github.com/hugolgst/paco/lexer"
	"strings"
)

// parseItem returns the parsed node from the given Item
func (parser *Parser) parseItem(item lexer.Item) Node {
	switch {
	case item.Type == lexer.ItemNumber:
		return Node{
			Type:  NumberLiteral,
			Value: item.Value,
		}

	case item.Type == lexer.ItemString:
		return Node{
			Type:  StringLiteral,
			Value: item.Value,
		}

	case item.Type == lexer.ItemIdentifier:
		return parseIdentifier(parser, item.Value)

	case item.Type > lexer.ItemKeyword:
		return parseKeyword(parser, item.Type)
	}

	return Node{}
}

// parseKeyword returns the node for the given keyword
func parseKeyword(parser *Parser, keyword lexer.ItemType) Node {
	switch keyword {
	case lexer.ItemFunction:
		return parseFunction(parser)
	}

	return Node{}
}

// parseIdentifier identifies whether the identifier is a function call or an assignment
func parseIdentifier(parser *Parser, identifier string) Node {
	item := parser.next()

	switch {
	// If the next item is a parentheses, then it is a function
	case item.Type == lexer.ItemLeftParentheses:
		return parseCall(parser, identifier)

	// If the next item is an equal symbol, then it is an assignment
	case item.Type == lexer.ItemEquals:
		return parseAssignment(parser, identifier)

	case item.Type == lexer.ItemIdentifier && strings.HasPrefix(item.Value, "|"):
		parser.next()
		return parseCall(parser, identifier + item.Value)
	}

	return Node{}
}