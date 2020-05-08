package parser

// parseCondition
func parseCondition(parser *Parser) Node {
	node := Node{
		Type: Condition,
	}

	item := parser.next()
	node.Params = append(node.Params, parseItem(parser, item))

	node.Value = parser.next().Value

	item = parser.next()
	node.Params = append(node.Params, parseItem(parser, item))

	return node
}
