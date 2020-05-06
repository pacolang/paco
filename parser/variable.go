package parser

// parseAssignment parses a variable assignment
func parseAssignment(parser *Parser, identifier string) Node {
	return Node{
		Type: Assignment,
		Value: identifier,
		Params: []Node{
			parser.parseItem(parser.next()),
		},
	}
}
