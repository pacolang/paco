package parser

import (
	"github.com/hugolgst/paco/lexer"
	"github.com/hugolgst/paco/log"
)

// parseCondition parses a condition and its body and returns the node
func parseCondition(parser *Parser) (node Node) {
	node = Node{
		Type: Condition,
		Params: []Node{
			parseItem(parser, parser.next()),
		},
		Value: parser.next().Value,
	}

	// Append the last element on the params
	node.Params = append(
		node.Params,
		parseItem(parser, parser.next()),
	)

	// Add body nodes
	item := parser.next()
	for item.Type != lexer.ItemEnd {
		node.Body = append(node.Body, parseItem(parser, item))
		item = parser.next()

		if item.Type == lexer.ItemEOF {
			log.Errorf("end was not found")
		}
	}

	return
}
