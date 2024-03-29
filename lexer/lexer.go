package lexer

import (
	"unicode/utf8"
)

const (
	EOF rune = -1
)

// A Lexer is the structure to iterate through the input and emit the items
// into the channel
type Lexer struct {
	Input    string
	Start    int
	Width    int
	Position int
	Items    chan Item
}

// emit allows to add the current token to the channel
func (lexer *Lexer) emit(t ItemType) {
	lexer.Items <- Item{
		Type:  t,
		Value: lexer.Input[lexer.Start:lexer.Position],
	}

	lexer.Start = lexer.Position
}

// next moves the position to the next rune and returns it
func (lexer *Lexer) next() (rune rune) {
	// Returns EOF if the position if over the length of the input
	if lexer.Position >= len(lexer.Input) {
		return EOF
	}

	// Decodes the first rune in the given input, gets it and its width
	rune, lexer.Width = utf8.DecodeRuneInString(lexer.Input[lexer.Position:])
	lexer.Position += lexer.Width

	return rune
}

// ignore moves the current starting position to ignore a token
func (lexer *Lexer) ignore() {
	lexer.Start = lexer.Position
}

// back moves to the latest position in the input
func (lexer *Lexer) back() {
	lexer.Position -= lexer.Width
}

// run iterate through the runes of the lexer inputs and lex them
func (lexer *Lexer) run() {
	for lexer.Position < len(lexer.Input) {
		rune := lexer.next()
		value, valid := symbols[rune]

		switch {
		case IsAlphaNumeric(rune):
			lexer.back()
			lexIdentifier(lexer)
			break

		case IsSpace(rune):
			lexer.ignore()
			break

		case rune == '-':
			ignoreComments(lexer)
			break

		// Lex the string
		case rune == '"':
			lexString(lexer)
			break

		// Lex the number in case it starts with a +, - or a number
		case rune == '+' || rune == '-' || ('0' <= rune && rune <= '9'):
			lexer.back()
			lexNumber(lexer)
			break

		case valid:
			lexer.emit(value)
			break

		case rune == '<':
			nextRune := lexer.next()
			if nextRune == '=' {
				lexer.emit(ItemEqualOrLessCheck)
			} else {
				lexer.back()
				lexer.emit(ItemLessCheck)
			}

		case rune == '>':
			nextRune := lexer.next()
			if nextRune == '=' {
				lexer.emit(ItemEqualOrGreaterCheck)
			} else {
				lexer.back()
				lexer.emit(ItemGreaterCheck)
			}

		case rune == '=':
			// Check if it is a double equals or not
			nextRune := lexer.next()
			if nextRune == '=' {
				lexer.emit(ItemEqualityCheck)
			} else if nextRune == '!' {
				lexer.emit(ItemNotEqualityCheck)
			} else {
				lexer.back()
				lexer.emit(ItemEquals)
			}
		}
	}

	lexer.emit(ItemEOF)
}

// Lex creates a Lexer with the given input, runs it in a go routine and returns the lexer and
// its channel for items
func Lex(input string) (*Lexer, chan Item) {
	lexer := &Lexer{
		Input: input + " ",
		Items: make(chan Item),
	}

	go lexer.run()
	return lexer, lexer.Items
}
