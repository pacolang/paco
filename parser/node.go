package parser

import "../lexer"

type nodeType int

// A Node is used to make the AST tree
type Node struct {
	Type       nodeType
	Value      string
	Params     []Node
	Body       []Node
	ReturnType nodeType
}

const (
	CallExpression nodeType = iota
	NumberLiteral
	StringLiteral
	Assignment
	Function
	Parameter
)

var types = map[lexer.ItemType]nodeType{
	lexer.ItemStringType: StringLiteral,
	lexer.ItemIntType:    NumberLiteral,
}
