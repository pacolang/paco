package lexer

import (
	"strings"
)

// accept takes the next rune and checks if its part of the given valid value
func (lexer *Lexer) accept(valid string) bool {
	// If the next rune isn't in the given value
	if strings.IndexRune(valid, lexer.next()) >= 0 {
		return true
	}

	// Move back to the latest rune
	lexer.back()
	return false
}

// acceptRun consumes a run of runes from the valid set
func (lexer *Lexer) acceptRun(valid string) {
	for strings.IndexRune(valid, lexer.next()) >= 0 {}
	lexer.back()
}

// lexNumber emits the next number by accepting numbers like -14.50 or +192
func lexNumber(lexer *Lexer) {
	lexer.accept("+-")
	digits := "0123456789"
	lexer.acceptRun(digits)

	if lexer.accept(".") {
		lexer.acceptRun(digits)
	}

	lexer.emit(itemNumber)
}

// lexString emits the next string by accepting double quotes
func lexString(lexer *Lexer) {
	Loop:
	for {
		switch rune := lexer.next(); rune {
		case '"':
			break Loop
		// Ignore \
		case '\\':
			lexer.next()
			break
		}
	}

	lexer.emit(itemString)
}