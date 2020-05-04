package parser

import "../lexer"

type Parser struct {
	itemsChannel  chan lexer.Item
	previousItems []lexer.Item
	item          lexer.Item
	position      int
	nodes         []Node
}

func Parse(input string) *Parser {
	_, channel := lexer.Lex(input)

	return &Parser{
		itemsChannel: channel,
	}
}

func (parser *Parser) run() {
	for {
		// Wait for an element in the channel to appear and add it to the array of items
		item := <-parser.itemsChannel
		parser.previousItems = append(parser.previousItems, item)

		// When the lexer has finished its job break the for loop
		if item.Type == lexer.ItemEOF {
			break
		}
	}
}

func parseCall() {

}
