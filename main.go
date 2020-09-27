package main

import (
	"flag"
	"github.com/pacolang/paco/log"
	"github.com/pacolang/paco/runtime"
	"io/ioutil"
	"os"
)

func main() {
	// Build command with its flags
	buildCommand := flag.NewFlagSet("build", flag.ExitOnError)
	buildFile := buildCommand.String("f", "", "Specifies the file to execute")
	buildName := buildCommand.String("o", "main", "Specifies the name of the executable")

	// Gets the executable name
	flag.Parse()

	if len(os.Args) < 2 {
		log.Errorf("You need to specify a command.")
	}

	// Commands
	switch os.Args[1] {
	case "build":
		err := buildCommand.Parse(os.Args[2:])
		if err != nil {
			panic(err)
		}

		build(*buildFile, *buildName)
		break
	}
}

// build builds a specified paco file
func build(file, name string) {
	// Check if the file is specified
	if file == "" {
		log.Errorf("You need to specify a file to build.")
	}

	// Read the given file
	bytes, err := ioutil.ReadFile(file)
	if err != nil {
		log.Errorf(err)
	}

	runtime.Run(string(bytes), name)
}
