package generator

import "testing"

func TestGenerate(t *testing.T) {
	Generate(`fn hello()
console|println("hello world")
end

hello()`)
}
