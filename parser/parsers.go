package parser

import (
	"github.com/pacolang/paco/log"
	"strings"

	"github.com/pacolang/paco/lexer"
)

// parseItem returns the parsed node from the given Item
func parseItem(parser *Parser, item lexer.Item) Node {
	switch {
	case item.Type == lexer.ItemNumber:
		return Node{
			Type:  NumberLiteral,
			Value: item.Value,
			ReturnType: NumberLiteral,
		}

	case item.Type == lexer.ItemBoolean:
		// Convert the booleans expressions to integers
		value := "0"
		if item.Value == "true" {
			value = "1"
		}

		return Node{
			Type:  Boolean,
			Value: value,
			ReturnType: Boolean,
		}

	case item.Type == lexer.ItemString:
		return Node{
			Type:  StringLiteral,
			Value: item.Value,
			ReturnType: StringLiteral,
		}

	case item.Type == lexer.ItemVariableValue:
		name := item.Value[1:]
		variableType, ok := variables[name]
		if !ok {
			log.Errorf("Unable to find the %s variable")
		}

		return Node{
			Type:  Variable,
			Value: name,
			ReturnType: variableType,
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
	case lexer.ItemIf:
		return parseCondition(parser)
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

	// If the next item is a type then it is an empty variable assignment
	case item.Type > lexer.ItemTypes:
		return parseEmptyAssignment(identifier, item.Type)

	case item.Type == lexer.ItemIdentifier && strings.HasPrefix(item.Value, "|"):
		parser.next()
		return parseCall(parser, identifier+item.Value)
	}

	return Node{}
}
