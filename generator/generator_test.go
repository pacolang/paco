package generator

import "testing"

func TestGenerate(t *testing.T) {
	Generate(`console|println("pd")`)
}
