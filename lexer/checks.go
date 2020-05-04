package lexer

import "unicode"

// IsAlphaNumeric reports whether r is an alphabetic, digit, or underscore.
func IsAlphaNumeric(rune rune) bool {
	return rune == '_' || unicode.IsLetter(rune) || unicode.IsDigit(rune)
}

// IsSpace reports whether rune is a space, line break
func IsSpace(rune rune) bool {
	return rune == ' ' || rune == '\t' || rune == '\n' || rune == '\r'
}
