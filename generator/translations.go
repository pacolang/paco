package generator

import "github.com/hugolgst/paco/parser"

var (
	types = map[parser.NodeType]string{
		parser.StringLiteral: "char*",
		parser.NumberLiteral: "int",
	}
	imports = "#include <%s.h>"
)
