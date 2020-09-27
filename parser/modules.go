package parser

import (
	"github.com/pacolang/paco/log"
	"io/ioutil"
	"os"
	"path/filepath"
	"strings"
)

// ReadModules read the modules, parse them and returns the functions found
func ReadModules() (functions []FunctionRecorder) {
	paths := findModules()

	for _, path := range paths {
		mod, err := ioutil.ReadFile(path)
		if err != nil {
			log.Errorf("unable to read %s paco module", path)
		}

		functions = append(functions, ParseModules(string(mod))...)
	}

	return
}

// findModules returns the paths of modules files
func findModules() (paths []string) {
	filepath.Walk("./core", func(path string, f os.FileInfo, err error) error {
		if !strings.HasSuffix(path, ".pacomod") {
			return nil
		}

		paths = append(paths, path)
		return err
	})

	return
}
