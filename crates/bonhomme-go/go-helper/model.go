package main

import (
	"go/ast"
	"go/token"
)

type sourceFile struct {
	Path    string `json:"path"`
	Content string `json:"content"`
}

type parseRequest struct {
	Files []sourceFile `json:"files"`
}

type parseResponse struct {
	Files []parsedFile `json:"files"`
}

type parsedFile struct {
	Path         string        `json:"path"`
	PackageName  string        `json:"packageName"`
	Imports      string        `json:"imports"`
	Declarations []declaration `json:"declarations"`
}

type declaration struct {
	Kind        string       `json:"kind"`
	Name        string       `json:"name"`
	Receiver    string       `json:"receiver,omitempty"`
	Signature   string       `json:"signature,omitempty"`
	Body        string       `json:"body,omitempty"`
	Declaration string       `json:"declaration,omitempty"`
	Fields      []field      `json:"fields,omitempty"`
	Methods     []method     `json:"methods,omitempty"`
	Calls       []callTarget `json:"calls,omitempty"`
}

type field struct {
	Name        string `json:"name"`
	Declaration string `json:"declaration"`
}

type method struct {
	Name      string `json:"name"`
	Signature string `json:"signature"`
}

type callTarget struct {
	Kind     string `json:"kind"`
	Name     string `json:"name"`
	Receiver string `json:"receiver,omitempty"`
}

type parsedInput struct {
	file   sourceFile
	ast    *ast.File
	tokens *token.File
}
