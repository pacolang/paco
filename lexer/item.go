package lexer

import "fmt"

// ItemType is the type of Item, types are initialized below in const
type ItemType int

// An item is used for the lexer
type Item struct {
	Type  ItemType
	Value string
}

const (
	itemError ItemType = iota
	itemEOF
	itemNumber
	itemString
	itemPipe
	itemEquals
	itemPlus
	itemMinus
	itemDivide
	itemTimes
	itemField
	itemBoolean
	itemIdentifier
	itemLeftParentheses
	itemRightParentheses
	// Delimit the keywords
	itemKeyword
	itemFunction
	itemIncludes
	itemStringType
	itemIntType
)

var keywords = map[string]ItemType{
	"fn":       itemFunction,
	"includes": itemIncludes,
	"string":   itemStringType,
	"int":      itemIntType,
}

var symbols = map[rune]ItemType{
	'|': itemPipe,
	'(': itemLeftParentheses,
	')': itemRightParentheses,
	'=': itemEquals,
	'+': itemPlus,
	'-': itemMinus,
	'*': itemTimes,
	'/': itemDivide,
}

// String methods is the one used by Printf
func (item Item) String() string {
	if item.Type == itemError {
		return item.Value
	}

	return fmt.Sprintf("%q", item.Value)
}
