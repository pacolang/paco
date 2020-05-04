package parser

type nodeType int

// A Node is used to make the AST tree
type Node struct {
	Type   nodeType
	Value  string
	Params []Node
}

const (
	Program nodeType = iota
)
