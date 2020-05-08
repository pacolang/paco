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
	ItemError ItemType = iota
	ItemEOF
	ItemNumber
	ItemString
	ItemPipe
	ItemEquals
	ItemPlus
	ItemMinus
	ItemDivide
	ItemTimes
	ItemBoolean
	ItemIdentifier
	ItemComma
	ItemLeftParentheses
	ItemRightParentheses
	ItemVariableValue
	// Delimit the equality operators
	ItemEquality
	ItemEqualityCheck
	ItemNotEqualityCheck
	// Delimit the keywords
	ItemKeyword
	ItemFunction
	ItemIncludes
	ItemEnd
	ItemIf
	// Delimit the types
	ItemTypes
	ItemStringType
	ItemIntType
)

var keywords = map[string]ItemType{
	"fn":       ItemFunction,
	"includes": ItemIncludes,
	"string":   ItemStringType,
	"int":      ItemIntType,
	"end":      ItemEnd,
	"if":       ItemIf,
}

var symbols = map[rune]ItemType{
	'|': ItemPipe,
	',': ItemComma,
	'(': ItemLeftParentheses,
	')': ItemRightParentheses,
	'+': ItemPlus,
	'-': ItemMinus,
	'*': ItemTimes,
	'/': ItemDivide,
}

// String methods is the one used by Printf
func (item Item) String() string {
	if item.Type == ItemError {
		return item.Value
	}

	return fmt.Sprintf("typ: %d | val: %q", item.Type, item.Value)
}
