package parser

import (
	"github.com/hugolgst/paco/lexer"
)

// A Parser receives the items from the lexer, parses them to get nodes and push them
// into the nodes channel
type Parser struct {
	ItemsChannel  chan lexer.Item
	PreviousItems []lexer.Item
	Item          lexer.Item
	NodesChannel  chan Node
}

// Create the parser and run it
func Parse(input string) Parser {
	_, channel := lexer.Lex(input)

	parser := Parser{
		ItemsChannel: channel,
		NodesChannel: make(chan Node),
	}

	go parser.run()

	return parser
}

// add appends the given node to the array of Nodes of the parser
func (parser *Parser) emit(node Node) {
	parser.NodesChannel <- node
}

// next moves the Position to the next Item and returns it
func (parser *Parser) next() (item lexer.Item) {
	item = <-parser.ItemsChannel
	parser.PreviousItems = append(parser.PreviousItems, item)

	return
}

// run wait for the items in the channel and parse them
func (parser *Parser) run() {
	for {
		// Gets the next Item
		item := parser.next()

		// Break the loop if EOF occurs
		if item.Type == lexer.ItemEOF {
			parser.emit(Node{
				Type: EOF,
			})
			break
		}

		// Parse the current item
		parser.emit(parseItem(parser, item))
	}
}