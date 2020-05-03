package lexer

const (
	// Special characters
	LeftParentheses  = "left parentheses"
	RightParentheses = "right parentheses"
	Quote            = "quote"
	Comma            = "comma"
	Equals           = "equals"
	Pipe             = "pipe"

	// Keywords
	Function = "fn"
	End      = "end"
	String   = "string"
	Includes = "includes"

	// Token types
	Keyword = "keyword"
	Name    = "name"
	Number  = "number"
	Symbol  = "symbol"
)

var keywords = []string{
	Function, End, String, Includes,
}

var symbols = map[string]string{
	LeftParentheses:  "(",
	RightParentheses: ")",
	Quote:            "'",
	Comma:            ",",
	Equals:           "=",
	Pipe:             "|",
}

// A Token is used to reference a part of the code with its value
type Token struct {
	Type  string
	Value string
}
