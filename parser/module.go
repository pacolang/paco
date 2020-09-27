package parser

import (
	"github.com/pacolang/paco/lexer"
	"github.com/pacolang/paco/log"
	"strings"
)

var (
	item lexer.Item
	functions []FunctionRecorder
)

// A FunctionRecorder gets a function record without its definition
type FunctionRecorder struct {
	Name       string
	Params     []NodeType
	ReturnType NodeType
}

func ParseModules(input string) []FunctionRecorder {
	_, channel := lexer.Lex(input)

	parser := Parser{
		ItemsChannel: channel,
	}

	parser.runModulesParser()

	return functions
}

// runModulesParser parses a module file with its function records
func (parser *Parser) runModulesParser() {
	item = parser.next()
	if item.Type != lexer.ItemMod {
		log.Errorf("Module should begin with mod")
	}

	// Gets the module name
	moduleName := strings.Replace(parser.next().Value, "\"", "", -1)

	// Parses the function records
	item = parser.next()
	for item.Type != lexer.ItemEOF {
		parseFunctionRecord(parser, moduleName)
	}
}

// parseFunctionRecord append the parsed function record to the functions slice
func parseFunctionRecord(parser *Parser, moduleName string) {
	// Check if the first item is the function keyword
	if item.Type != lexer.ItemFunction {
		log.Errorf("a module file must contain only function recorder")
	}

	// Get the function name
	name := parser.next()
	if name.Type != lexer.ItemIdentifier {
		log.Errorf("name of the function should be an identifier")
	}

	// Check if the next item in an opening parentheses
	parentheses := parser.next()
	if parentheses.Type != lexer.ItemLeftParentheses {
		log.Errorf("the left parentheses is missing")
	}

	var params []NodeType
	item = parser.next()

	// Parses the types parameters
	for item.Type != lexer.ItemRightParentheses {
		// log the error if the parameter isn't a type
		if item.Type < lexer.ItemTypes {
			log.Errorf("function recorder params should be types")
		}

		// Add the type to the parameters slice
		params = append(params, types[item.Type])
		item = parser.next()
	}

	// Create the function recorder
	recorder := FunctionRecorder{
		Name: moduleName + "|" + name.Value,
		Params: params,
	}

	// Add the type to the recorder if there is one
	item = parser.next()
	if item.Type > lexer.ItemTypes {
		recorder.ReturnType = types[item.Type]
		item = parser.next()
	}

	// Add the record to the slice
	functions = append(functions, recorder)
}
