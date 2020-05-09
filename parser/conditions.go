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
			parseBoolean(parser),
		},
	}

	item := parser.next()
	for item.Type == lexer.ItemOr || item.Type == lexer.ItemAnd {
		operator := "||"
		if item.Type == lexer.ItemAnd {
			operator = "&&"
		}

		// Append the next boolean and the operator
		node.Params = append(
			node.Params,
			Node{
				Type: ConditionOperator,
				Value: operator,
			},
			parseBoolean(parser),
		)
		item = parser.next()
	}

	// Add body items
	for item.Type != lexer.ItemEnd {
		node.Body = append(node.Body, parseItem(parser, item))
		item = parser.next()

		if item.Type == lexer.ItemEOF {
			log.Errorf("end was not found")
		}
	}

	return
}

func parseBoolean(parser *Parser) (node Node) {
	node = Node{
		Type: Boolean,
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

	return
}
