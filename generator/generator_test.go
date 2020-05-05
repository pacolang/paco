package generator

import "testing"

func TestGenerate(t *testing.T) {
	Generate(`
console|println("hello brah")

fn hello(val int) string
	console|println("hey")
end`)
}
