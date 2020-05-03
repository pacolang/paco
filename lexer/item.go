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
	itemNumber
)

// String methods is the one used by Printf
func (item Item) String() string {
	if item.Type == itemError {
		return item.Value
	}

	return fmt.Sprintf("%q", item.Value)
}
