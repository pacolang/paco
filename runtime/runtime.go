package runtime

import (
	"fmt"
	"github.com/hugolgst/paco/generator"
	"gopkg.in/pipe.v2"
	"log"
	"os"
	"os/exec"
)

// Run
func Run(input, executableName string) {
	code := generator.Generate(input)

	// Generate the executable using gcc
	p := pipe.Line(
		pipe.ChDir(getPacoPath() + "/core"),
		pipe.Println(code),
		pipe.Exec("gcc", "-L.", "-lpaco", "-x", "c", "-O", "-Wall", "-o", executableName, "-"),
	)

	output, err := pipe.CombinedOutput(p)
	if err != nil {
		fmt.Printf("%v\n", err)
	}
	fmt.Printf("%s", output)

	// Moves the executable
	movesExecutable(executableName)
}

// movesExecutable moves the generated executable to a ne
func movesExecutable(executableName string) {
	cmd := exec.Command(
		"mv",
		getPacoPath() + "/core/" + executableName,
		getCurrentPath() + "/" + executableName,
	)

	err := cmd.Run()
	if err != nil {
		log.Fatalf("mv failed with %s\n", err)
	}
}

// getPacoPath returns the specified path for paco
func getPacoPath() string {
	path := os.Getenv("PACOPATH")
	if path == "" {
		path = "."
	}

	return path
}

// getCurrentPath returns the current path
func getCurrentPath() string {
	path, err := os.Getwd()
	if err != nil {
		log.Println(err)
	}

	return path
}
