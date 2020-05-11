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
	Generic
	Variable

	Condition
	ConditionIf
	ConditionElse
	Boolean
	ConditionOperator

	Assignment
	EmptyAssignment
	Function
	Parameter
)

var types = map[lexer.ItemType]NodeType{
	lexer.ItemStringType:  StringLiteral,
	lexer.ItemIntType:     NumberLiteral,
	lexer.ItemBoolType:    Boolean,
	lexer.ItemGenericType: Generic,
}
