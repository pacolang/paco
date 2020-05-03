package lexer

import "regexp"

// IsLetter checks if the given character is a letter and returns the condition
func IsLetter(character string) bool {
	letterRegex := regexp.MustCompile(`[a-zA-Z]`)

	return letterRegex.Match([]byte(character))
}
