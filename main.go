package main

import (
	"./generator"
	"./log"
	"io/ioutil"
	"os"
)

func main() {
	// Check if the file is specified
	if len(os.Args) < 2 {
		log.Errorf("You need to give the file to execute.")
	}

	// Read the given file
	bytes, err := ioutil.ReadFile(os.Args[1])
	if err != nil {
		log.Errorf(err)
	}

	generator.Generate(string(bytes))
}
