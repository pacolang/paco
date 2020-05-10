package generator

import "github.com/hugolgst/paco/parser"

var (
	cTypes = map[parser.NodeType]string{
		parser.StringLiteral: "char*",
		parser.NumberLiteral: "int",
	}
	cImports         = "#include \"%s.h\""
	cCall            = "%s(%s)"
	cParam           = "%s %s"
	cFunction        = "%s %s(%s){%s}"
	cReturn          = "return %s;"
	cCode            = "%s\n%s\ntypedef enum{false=0,true=!false}bool;\nint main(){%s;return 0;}"
	cAssignment      = "%s %s = %s;"
	cCondition       = "if(%s){%s}"
	cElseCondition   = "else{%s}"
	cEmptyAssignment = "%s %s"
)
