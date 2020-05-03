package lexer

import (
	"strings"
)

var (
	characters []string
	character  string
	index      int
	tokens     []Token
)

// Tokenize returns an array of tokens for the given code
func Tokenize(code string) []Token {
	// Get all characters of the code
	characters = strings.Split(code, "")

	// Iterate through each character of the code
	for index = 0; index < len(characters); index++ {
		character = characters[index]

		if IsLetter(character) {
			AppendStringToken()
		}
	}

	return tokens
}

// AppendStringToken iterate through all the next letters and appends the token
func AppendStringToken() {
	var value string

	// Iterate through all the next letters
	for IsLetter(character) {
		// Append the character to the value
		value += character

		index++
		character = characters[index]
	}

	tokens = append(tokens, Token{
		Type:  "name",
		Value: value,
	})
}
