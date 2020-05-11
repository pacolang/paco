package runtime

import (
	"github.com/hugolgst/paco/generator"
	"io"
	"log"
	"os"
	"os/exec"
)

// Run creates an executable from the given code and the given executable name
func Run(input, executableName string) {
	code := generator.Generate(input)
	writeCode(code)

	// Generate the executable using gcc
	cmd := exec.Command("gcc", "main.c", "-L.", "-lpaco", "-o", executableName)
	cmd.Dir = "core"
	if err := cmd.Run(); err != nil {
		log.Fatalf("Run() failed with %s\n", err)
	}

	// Moves the executable and deletes the source file
	movesExecutable(executableName)
	deleteFile("main.c")
}

// writeCode writes the code into a C file
func writeCode(code string) {
	file, err := os.Create("./core/main.c")
	if err != nil {
		panic(err)
	}
	defer file.Close()

	_, err = io.WriteString(file, code)
	if err != nil {
		panic(err)
	}
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

// deleteFile deletes the given file
func deleteFile(file string) {
	cmd := exec.Command(
		"rm",
		getPacoPath() + "/core/" + file,
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
