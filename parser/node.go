package parser

import "github.com/hugolgst/paco/lexer"

type NodeType int

// A Node is used to make the AST tree
type Node struct {
	Type       NodeType
	Value      string
	Params     []Node
	Body       []Node
	ReturnType NodeType
}

const (
	CallExpression NodeType = iota
	EOF
	NumberLiteral
	StringLiteral
	Assignment
	Function
	Parameter
)

var types = map[lexer.ItemType]NodeType{
	lexer.ItemStringType: StringLiteral,
	lexer.ItemIntType:    NumberLiteral,
}
