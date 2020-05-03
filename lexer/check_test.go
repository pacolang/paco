package lexer

import "testing"

func TestIsLetter(t *testing.T) {
	sentence := "heyGuysWelcomeToPaco"

	for _, letter := range sentence {
		if !IsLetter(string(letter)) {
			t.Errorf("IsLetter() failed, excepted %s to be a letter.", string(letter))
		}
	}
}
