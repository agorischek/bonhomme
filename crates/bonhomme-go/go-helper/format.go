package main

import (
	"bytes"
	"go/format"
	"io"
	"os"
)

func formatSource() {
	source, err := io.ReadAll(os.Stdin)
	if err != nil {
		fatalf("read source: %v", err)
	}
	formatted, err := format.Source(source)
	if err != nil {
		fatalf("format source: %v", err)
	}
	if _, err := io.Copy(os.Stdout, bytes.NewReader(formatted)); err != nil {
		fatalf("write formatted source: %v", err)
	}
}
