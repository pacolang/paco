package lexer

import "regexp"

// IsLetter checks if the given character is a letter and returns the condition
func IsLetter(character string) bool {
	letterRegex := regexp.MustCompile(`[a-zA-Z]`)

	return letterRegex.Match([]byte(character))
}

// IsSymbol checks if the given character is a symbol and returns the condition
func IsSymbol(character string) bool {
	symbolRegex := regexp.MustCompile(`[()|\-',=]`)

	return symbolRegex.Match([]byte(character))
}

// IsNumber checks if the given character is a number and returns the condition
func IsNumber(character string) bool {
	numberRegex := regexp.MustCompile(`[0-9]`)

	return numberRegex.Match([]byte(character))
}