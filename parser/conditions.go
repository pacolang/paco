package parser

import (
	"github.com/pacolang/paco/lexer"
	"github.com/pacolang/paco/log"
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
				Type:  ConditionOperator,
				Value: operator,
			},
			parseBoolean(parser),
		)
		item = parser.next()
	}

	// Add body items
	node.Body = append(node.Body, Node{
		Type: ConditionIf,
		Body: parseBody(parser, item),
	})

	// Get the last item when parseBody stopped
	item = parser.PreviousItems[len(parser.PreviousItems)-1]
	// Return if there isn't an else
	if item.Type != lexer.ItemElse {
		return
	}

	// Parses the body of the else and returns the node
	item = parser.next()
	node.Body = append(node.Body, Node{
		Type: ConditionElse,
		Body: parseBody(parser, item),
	})

	return
}

// parseBody parses all items in the body of the condition and returns it
func parseBody(parser *Parser, item lexer.Item) (body []Node) {
	for item.Type != lexer.ItemEnd && item.Type != lexer.ItemElse {
		body = append(body, parseItem(parser, item))
		item = parser.next()

		if item.Type == lexer.ItemEOF {
			log.Errorf("end was not found")
		}
	}

	return
}

// parseBoolean parses a boolean inside the condition
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
