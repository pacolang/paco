package lexer

const (
	// Special characters
	LEFT_PARENTHESES  = "("
	RIGHT_PARENTHESES = ")"
	COMMENT           = "-"
	QUOTE             = "'"
	COMMA             = ","
	EQUALS            = "="

	// Keywords
	FUNCTION = "fn"
	END      = "end"
	STRING   = "string"
	INCLUDES = "includes"
)

// A Token is used to reference a part of the code with its value
type Token struct {
	Type  string
	Value string
}
