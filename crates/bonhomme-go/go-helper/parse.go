package main

import (
	"encoding/json"
	"fmt"
	"go/ast"
	"go/importer"
	"go/parser"
	"go/token"
	"go/types"
	"os"
	"strings"
)

func parse() {
	var request parseRequest
	if err := json.NewDecoder(os.Stdin).Decode(&request); err != nil {
		fatalf("decode parse request: %v", err)
	}

	fset := token.NewFileSet()
	inputs := make([]parsedInput, 0, len(request.Files))
	astFiles := make([]*ast.File, 0, len(request.Files))
	for _, file := range request.Files {
		parsed, err := parser.ParseFile(fset, file.Path, file.Content, parser.ParseComments)
		if err != nil {
			fatalf("parse %s: %v", file.Path, err)
		}
		tokenFile := fset.File(parsed.FileStart)
		inputs = append(inputs, parsedInput{file: file, ast: parsed, tokens: tokenFile})
		astFiles = append(astFiles, parsed)
	}

	info := &types.Info{
		Uses:       map[*ast.Ident]types.Object{},
		Selections: map[*ast.SelectorExpr]*types.Selection{},
	}
	conf := types.Config{
		Importer: importer.Default(),
		Error:    func(error) {},
	}
	pkgName := "bonhomme"
	if len(astFiles) > 0 {
		pkgName = astFiles[0].Name.Name
	}
	pkg, _ := conf.Check(pkgName, fset, astFiles, info)

	response := parseResponse{Files: make([]parsedFile, 0, len(inputs))}
	for _, input := range inputs {
		response.Files = append(response.Files, parseOneFile(input, fset, info, pkg))
	}

	encoder := json.NewEncoder(os.Stdout)
	encoder.SetIndent("", "  ")
	if err := encoder.Encode(response); err != nil {
		fatalf("encode parse response: %v", err)
	}
}

func parseOneFile(
	input parsedInput,
	fset *token.FileSet,
	info *types.Info,
	pkg *types.Package,
) parsedFile {
	file := parsedFile{
		Path:        input.file.Path,
		PackageName: input.ast.Name.Name,
		Imports:     importsText(input, fset),
	}

	for _, decl := range input.ast.Decls {
		switch decl := decl.(type) {
		case *ast.FuncDecl:
			if parsed := parseFuncDecl(input, decl, info, pkg); parsed != nil {
				file.Declarations = append(file.Declarations, *parsed)
			}
		case *ast.GenDecl:
			file.Declarations = append(file.Declarations, parseGenDecl(input, decl)...)
		}
	}

	return file
}

func importsText(input parsedInput, fset *token.FileSet) string {
	var blocks []string
	for _, decl := range input.ast.Decls {
		gen, ok := decl.(*ast.GenDecl)
		if !ok || gen.Tok != token.IMPORT {
			continue
		}
		blocks = append(blocks, strings.TrimSpace(span(input, gen.Pos(), gen.End())))
	}
	return strings.Join(blocks, "\n")
}

func parseGenDecl(input parsedInput, decl *ast.GenDecl) []declaration {
	if decl.Tok == token.IMPORT {
		return nil
	}

	var declarations []declaration
	for _, spec := range decl.Specs {
		switch spec := spec.(type) {
		case *ast.TypeSpec:
			declarations = append(declarations, parseTypeSpec(input, spec))
		case *ast.ValueSpec:
			declarations = append(declarations, parseValueSpec(input, decl, spec)...)
		}
	}
	return declarations
}

func parseTypeSpec(input parsedInput, spec *ast.TypeSpec) declaration {
	switch typed := spec.Type.(type) {
	case *ast.StructType:
		return declaration{
			Kind:        "struct",
			Name:        spec.Name.Name,
			Declaration: fmt.Sprintf("type %s struct", spec.Name.Name),
			Fields:      parseFields(input, typed.Fields),
		}
	case *ast.InterfaceType:
		return declaration{
			Kind:        "interface",
			Name:        spec.Name.Name,
			Declaration: fmt.Sprintf("type %s interface", spec.Name.Name),
			Methods:     parseInterfaceMethods(input, typed.Methods),
		}
	default:
		return declaration{
			Kind:        "type",
			Name:        spec.Name.Name,
			Declaration: strings.TrimSpace(span(input, spec.Pos(), spec.End())),
		}
	}
}

func parseValueSpec(input parsedInput, decl *ast.GenDecl, spec *ast.ValueSpec) []declaration {
	kind := strings.ToLower(decl.Tok.String())
	declarations := make([]declaration, 0, len(spec.Names))
	for _, name := range spec.Names {
		declarations = append(declarations, declaration{
			Kind:        kind,
			Name:        name.Name,
			Declaration: strings.TrimSpace(span(input, decl.Pos(), decl.End())),
		})
	}
	return declarations
}

func parseFields(input parsedInput, fields *ast.FieldList) []field {
	if fields == nil {
		return nil
	}
	var parsed []field
	for _, entry := range fields.List {
		typeText := strings.TrimSpace(span(input, entry.Type.Pos(), entry.Type.End()))
		tagText := fieldTag(input, entry)
		if len(entry.Names) == 0 {
			parsed = append(parsed, field{
				Name:        typeText,
				Declaration: strings.TrimSpace(span(input, entry.Pos(), entry.End())),
			})
			continue
		}
		for _, name := range entry.Names {
			parsed = append(parsed, field{
				Name:        name.Name,
				Declaration: fmt.Sprintf("%s %s%s", name.Name, typeText, tagText),
			})
		}
	}
	return parsed
}

func fieldTag(input parsedInput, entry *ast.Field) string {
	if entry.Tag == nil {
		return ""
	}
	return " " + strings.TrimSpace(span(input, entry.Tag.Pos(), entry.Tag.End()))
}

func parseInterfaceMethods(input parsedInput, methods *ast.FieldList) []method {
	if methods == nil {
		return nil
	}
	var parsed []method
	for _, entry := range methods.List {
		if len(entry.Names) == 0 {
			continue
		}
		fullText := strings.TrimSpace(span(input, entry.Pos(), entry.End()))
		for _, name := range entry.Names {
			parsed = append(parsed, method{
				Name:      name.Name,
				Signature: interfaceMethodSignature(name.Name, fullText, entry.Names),
			})
		}
	}
	return parsed
}

func interfaceMethodSignature(name string, fullText string, names []*ast.Ident) string {
	if len(names) == 1 {
		return fullText
	}
	return name + strings.TrimPrefix(fullText, name)
}

func parseFuncDecl(
	input parsedInput,
	decl *ast.FuncDecl,
	info *types.Info,
	pkg *types.Package,
) *declaration {
	if decl.Body == nil {
		return nil
	}
	signature := strings.TrimSpace(span(input, decl.Pos(), decl.Body.Lbrace))
	parsed := &declaration{
		Kind:      "function",
		Name:      decl.Name.Name,
		Signature: signature,
		Body:      strings.TrimSpace(span(input, decl.Body.Lbrace+1, decl.Body.Rbrace)),
		Calls:     functionCalls(decl.Body, info, pkg),
	}
	if decl.Recv != nil && len(decl.Recv.List) > 0 {
		if receiver := receiverTypeName(decl.Recv.List[0].Type); receiver != "" {
			parsed.Kind = "method"
			parsed.Receiver = receiver
		}
	}
	return parsed
}

func span(input parsedInput, start token.Pos, end token.Pos) string {
	startOffset := input.tokens.Offset(start)
	endOffset := input.tokens.Offset(end)
	if startOffset < 0 || endOffset < startOffset || endOffset > len(input.file.Content) {
		return ""
	}
	return input.file.Content[startOffset:endOffset]
}
