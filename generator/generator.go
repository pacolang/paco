package generator

import (
	"fmt"
	"github.com/hugolgst/paco/parser"
	"io/ioutil"
	"os/exec"
	"strings"
)

var (
	imports   []string
	mainCalls []string
	functions []string
	types = map[parser.NodeType]string{
		parser.StringLiteral: "char*",
		parser.NumberLiteral: "int",
	}
)

func Generate(input string) {
	nodes := parser.Parse(input).Nodes

	for _, node := range nodes {
		switch node.Type {
		case parser.Function:
			functions = append(functions, generateFunction(node))
			break
		case parser.CallExpression:
			mainCalls = append(mainCalls, generateCall(node))
			break
		}
	}

	code := fmt.Sprintf(
		"%s\n%s\nint main(){%sreturn 0;}",
		strings.Join(imports, "\n"),
		strings.Join(functions, "\n"),
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

func generateNode(node parser.Node) string {
	switch node.Type {
	case parser.CallExpression:
		return generateCall(node)
	}

	return ""
}

func generateParam(node parser.Node) string {
	return fmt.Sprintf("%s %s", types[node.ReturnType], node.Value)
}

func generateFunction(node parser.Node) string {
	returnType := types[node.ReturnType]
	if returnType == "" {
		returnType = "void"
	}

	var params []string
	for _, param := range node.Params {
		params = append(params, generateParam(param))
	}

	joinedParams := ""
	if len(params) > 0 {
		joinedParams = strings.Join(params, ",")
	}

	var body []string
	for i := 0; i < len(node.Body)-1; i++ {
		body = append(body, generateNode(node.Body[i]))
	}

	return fmt.Sprintf(
		"%s %s(%s){%sreturn %s;}",
		returnType,
		node.Value,
		joinedParams,
		strings.Join(body, ";"),
		node.Body[len(node.Body)-1].Value,
	)
}

func generateCall(node parser.Node) string {
	var identifier string
	if strings.Contains(node.Value, "|") {
		split := strings.Split(node.Value, "|")

		imports = append(imports, fmt.Sprintf(`#include "%s.h"`, split[0]))

		identifier = split[1]
	} else {
		identifier = node.Value
	}

	var params []string
	for _, param := range node.Params {
		params = append(params, param.Value)
	}

	call := fmt.Sprintf(
		"%s(%s);",
		identifier,
		strings.Join(params, ","),
	)

	return call
}
