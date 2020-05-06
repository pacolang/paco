package parser

import "github.com/hugolgst/paco/lexer"

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
		break

	case item.Type > lexer.ItemKeyword:
		return parseKeyword(parser, item.Type)
		break
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
	switch item := parser.next(); item.Type {
	// If the next item is a parentheses, then it is a function
	case lexer.ItemLeftParentheses:
		return parseCall(parser, identifier)

	// If the next item is an equal symbol, then it is an assignment
	case lexer.ItemEquals:
		return parseAssignment(parser, identifier)
	}

	return Node{}
}
