package parser

import "fmt"

// quand il y a +-/* alors je prends le symbole et la valeur après et avant
// tant que la valeur après la valeur d'après est un symbole je continue
// type de variable directement dans le parser avec le ffi
func parseOperation(parser *Parser, node Node) Node {
	item := parser.next()
	if item.


	return node
}
