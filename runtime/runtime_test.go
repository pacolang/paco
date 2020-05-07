package runtime

import "testing"

func TestRun(t *testing.T) {
	Run(`fn hello()
console|println("hello world")
end

hello()`, "paco")
}
