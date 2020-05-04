package parser

import (
	"../lexer"
)

type Parser struct {
	itemsChannel  chan lexer.Item
	previousItems []lexer.Item
	item          lexer.Item
	position      int
	nodes         []Node
}

// Create the parser and run it
func Parse(input string) Parser {
	_, channel := lexer.Lex(input)

	parser := Parser{
		itemsChannel: channel,
	}

	parser.run()

	return parser
}

func (parser *Parser) add(node Node) {
	parser.nodes = append(parser.nodes, node)
}

// next moves the position to the next item and returns it
func (parser *Parser) next() (item lexer.Item) {
	item = <-parser.itemsChannel
	parser.previousItems = append(parser.previousItems, item)

	return
}

// run wait for the items in the channel and parse them
func (parser *Parser) run() {
	for {
		item := parser.next()

		if item.Type == lexer.ItemEOF {
			break
		}

		parser.add(parser.parseItem(item))

		parser.position++
	}
}

func (parser *Parser) parseItem(item lexer.Item) Node {
	switch item.Type {
	case lexer.ItemNumber:
		return Node{
			Type:  NumberLiteral,
			Value: item.Value,
		}
	case lexer.ItemString:
		return Node{
			Type:  StringLiteral,
			Value: item.Value,
		}
	case lexer.ItemIdentifier:
		return parseIdentifier(parser, item.Value)
	}

	return Node{}
}

func parseIdentifier(parser *Parser, identifier string) Node {
	switch parser.next().Type {
	case lexer.ItemLeftParentheses:
		return parseCall(parser, identifier)
	}

	return Node{}
}

func parseCall(parser *Parser, identifier string) Node {
	var params []Node

	item := parser.next()
	for item.Type != lexer.ItemRightParentheses {
		params = append(params, parser.parseItem(item))
		item = parser.next()
	}

	return Node{
		Type:   CallExpression,
		Value:  identifier,
		Params: params,
	}
}
