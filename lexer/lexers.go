package lexer

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

// lexIdentifier scans an alphanumeric or field
func lexIdentifier(lexer *Lexer) {
Loop:
	for {
		switch rune := lexer.next(); {
		case IsAlphaNumeric(rune) || rune == '.' && lexer.Input[lexer.Start] == '.':
			// let the characters be included
		default:
			lexer.back()
			word := lexer.Input[lexer.Start:lexer.Position]

			switch {
			case keywords[word] > itemKeyword:
				lexer.emit(keywords[word])
			case word[0] == '|':
				lexer.emit(itemField)
			case word == "true" || word == "false":
				lexer.emit(itemBoolean)
			default:
				lexer.emit(itemIdentifier)
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
