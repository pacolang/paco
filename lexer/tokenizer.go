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

		// Append the tokens for letters, symbols and numbers
		if IsLetter(character) {
			appendStringToken()
		}

		if IsSymbol(character) {
			appendSymbolToken()
		}

		if IsNumber(character) {
			appendNumberToken()
		}
	}

	return tokens
}

// appendNumberToken iterates through all the next numbers and appends the token
func appendNumberToken() {
	var value string

	// Iterate through all the next numbers
	for IsNumber(character) {
		// Append the character to the value
		value += character

		index++
		character = characters[index]
	}

	tokens = append(tokens, Token{
		Type:  Number,
		Value: value,
	})
}

// appendSymbolToken searches through the symbols to add a token
func appendSymbolToken() {
	// Iterate through the symbols to append the found one
	for _, symbol := range symbols {
		if symbol != character {
			continue
		}

		tokens = append(tokens, Token{
			Type:  Symbol,
			Value: symbol,
		})
	}
}

// appendStringToken iterates through all the next letters and appends the token
func appendStringToken() {
	var value string

	// Iterate through all the next letters
	for IsLetter(character) {
		// Append the character to the value
		value += character

		index++
		character = characters[index]
	}

	tokens = append(tokens, Token{
		Type:  getStringTokenType(value),
		Value: value,
	})
}

// getStringTokenType returns the right token type via
func getStringTokenType(value string) string {
	// Iterate through the keywords to see if the given value is a keyword
	for _, keyword := range keywords {
		if value != keyword {
			continue
		}

		return Keyword
	}

	// Returns "name" by default
	return Name
}
