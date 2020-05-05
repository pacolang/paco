package generator

import (
	"../parser"
	"fmt"
	"io/ioutil"
	"os/exec"
	"strings"
)

var (
	imports   []string
	mainCalls []string
)

func Generate(input string) {
	nodes := parser.Parse(input).Nodes

	for _, node := range nodes {
		switch node.Type {
		case parser.CallExpression:
			mainCalls = append(mainCalls, generateCall(node))
			break
		}
	}

	code := fmt.Sprintf(
		"%s\nint main(){%sreturn 0;}",
		strings.Join(imports, "\n"),
		strings.Join(mainCalls, ""),
	)

	err := ioutil.WriteFile("./core/main.c", []byte(code), 0644)
	if err != nil {
		panic(err)
	}

	cmd := exec.Command("gcc", "./core/main.c", "./core/console.o", "-o", "test")

	err = cmd.Run()
	if err != nil {
		panic(err)
	}
}

func generateCall(node parser.Node) string {
	split := strings.Split(node.Value, "|")

	imports = append(imports, fmt.Sprintf(`#include "%s.h"`, split[0]))

	var params []string
	for _, param := range node.Params {
		params = append(params, param.Value)
	}

	call := fmt.Sprintf(
		"%s(%s);",
		split[1],
		strings.Join(params, ","),
	)

	return call
}
