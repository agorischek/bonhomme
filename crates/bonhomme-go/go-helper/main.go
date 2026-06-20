package main

import (
	"fmt"
	"os"
)

func main() {
	if len(os.Args) != 2 {
		fatalf("usage: go-helper parse|format")
	}

	switch os.Args[1] {
	case "parse":
		parse()
	case "format":
		formatSource()
	default:
		fatalf("unknown command %q", os.Args[1])
	}
}

func fatalf(format string, args ...any) {
	fmt.Fprintf(os.Stderr, format+"\n", args...)
	os.Exit(1)
}
