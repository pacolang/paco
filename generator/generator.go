package generator

import (
	"../parser"
	"fmt"
)

func Generate(input string) {
	nodes := parser.Parse(input).Nodes

	for _, node := range nodes {
		fmt.Println(node)
	}
}