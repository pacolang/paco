package generator

import "github.com/hugolgst/paco/parser"

var (
	cTypes = map[parser.NodeType]string{
		parser.StringLiteral: "char*",
		parser.NumberLiteral: "int",
	}
	cImports = "#include <%s.h>"
	cCall    = "%s(%s);"
)
