package lexer

// lexNumber emits the next number by accepting numbers like -14.50 or +192
func lexNumber(lexer *Lexer) {
	isSymbol := lexer.accept("+-")
	digits := "0123456789"
	width := lexer.acceptRun(digits)

	// If its only a symbol do not register the item as a number
	if isSymbol && width == 0 {
		lexer.back()
		return
	}

	if lexer.accept(".") {
		lexer.acceptRun(digits)
	}

	lexer.emit(ItemNumber)
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

	lexer.emit(ItemString)
}

// lexIdentifier scans an alphanumeric or field
func lexIdentifier(lexer *Lexer) {
Loop:
	for {
		switch rune := lexer.next(); {
		case IsAlphaNumeric(rune):
			// let the characters be included
		default:
			lexer.back()
			word := lexer.Input[lexer.Start:lexer.Position]

			switch {
			case keywords[word] > ItemKeyword:
				lexer.emit(keywords[word])
			case word[0] == '|':
				lexer.emit(ItemField)
			case word == "true" || word == "false":
				lexer.emit(ItemBoolean)
			default:
				lexer.emit(ItemIdentifier)
			}

			break Loop
		}
	}
}

// ignoreComments ignore the rest of the line where a comment has been started
func ignoreComments(lexer *Lexer) {
	var rune rune
	for rune != '\n' {
		rune = lexer.next()
	}
	lexer.ignore()
}
