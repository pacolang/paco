package parser

import "../lexer"

type nodeType int

// A Node is used to make the AST tree
type Node struct {
	Type   nodeType
	Value  string
	Params []Node
	Body   []Node
}

const (
	CallExpression nodeType = iota
	NumberLiteral
	StringLiteral
	Assignment
	Function
)

var types = map[lexer.ItemType]nodeType{
	lexer.ItemStringType: StringLiteral,
	lexer.ItemIntType:    NumberLiteral,
}
