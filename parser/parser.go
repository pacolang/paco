package parser

import (
	"../lexer"
	"../log"
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

// add appends the given node to the array of nodes of the parser
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
		// Gets the next item
		item := parser.next()

		// Break the loop if EOF occurs
		if item.Type == lexer.ItemEOF {
			break
		}

		// Adds the parsed item
		parser.add(parser.parseItem(item))
		parser.position++
	}
}

// parseItem returns the parsed node from the given item
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

// parseIdentifier identifies whether the identifier is a function call or an assignment
func parseIdentifier(parser *Parser, identifier string) Node {
	item := parser.next()
	switch item.Type {
	case lexer.ItemLeftParentheses:
		return parseCall(parser, identifier)
	case lexer.ItemEquals:
		return parseAssignment(parser, identifier)
	}

	return Node{}
}

func parseKeyword(parser *Parser, keyword lexer.ItemType) Node {
	switch keyword {
	case lexer.ItemFunction:
		return parseFunction(parser)
	}

	return Node{}
}

func parseFunction(parser *Parser) Node {
	identifier := parser.next()
	if identifier.Type != lexer.ItemIdentifier {
		log.Errorf("name of the function should be an identifier")
	}

	node := Node{
		Type: Function,
		Value: identifier.Value,
	}

	item := parser.next()
	if item.Type != lexer.ItemLeftParentheses {
		log.Errorf("the left parentheses is missing")
	}

	for item.Type != lexer.ItemRightParentheses {
		node.Params = append(node.Params, parseParam(parser))
		item = parser.next()

		if item.Type == lexer.ItemEOF {
			log.Errorf("the right parentheses is missing")
		}
	}

	item = parser.next()
	if item.Type > lexer.ItemTypes {
		node.ReturnType = types[item.Type]
	}

	if item.Type == lexer.ItemEOF {
		log.Errorf("empty function declaration")
	}

	for item.Type != lexer.ItemEnd {
		node.Body = append(node.Body, parser.parseItem(item))
		item = parser.next()

		if item.Type == lexer.ItemEOF {
			log.Errorf("end was not found")
		}
	}

	return node
}

func parseParam(parser *Parser) Node {
	identifier := parser.next()
	if identifier.Type != lexer.ItemIdentifier {
		log.Errorf("name of the param should be an identifier")
	}

	typ := parser.next()
	if typ.Type < lexer.ItemTypes {
		log.Errorf("param type isn't valid")
	}

	return Node{
		Type: Parameter,
		Value: identifier.Value,
		ReturnType: types[typ.Type],
	}
}

// parseAssignment parses a variable assignment
func parseAssignment(parser *Parser, identifier string) Node {
	item := parser.next()

	node := Node{
		Type: Assignment,
		Value: identifier,
		Params: []Node{
			parser.parseItem(item),
		},
	}

	return node
}

// parseCall parses a function call and returns its node
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
