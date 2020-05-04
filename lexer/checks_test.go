package lexer

import "testing"

func TestIsAlphaNumeric(t *testing.T) {
	test := "hello_Paco"

	for i := 0; i < len(test); i++ {
		rune := rune(test[i])

		if !IsAlphaNumeric(rune) {
			t.Errorf("IsAlphaNumeric() failed, excepted %v to be an alpha numeric character.", rune)
		}
	}
}

func TestIsSpace(t *testing.T) {
	if !IsSpace(' ') || !IsSpace('\n') || !IsSpace('\t') || !IsSpace('\r') {
		t.Errorf("IsSpace() failed.")
	}
}
