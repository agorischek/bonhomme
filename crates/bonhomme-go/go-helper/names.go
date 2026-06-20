package main

import (
	"go/ast"
	"go/types"
)

func receiverTypeName(expr ast.Expr) string {
	switch expr := expr.(type) {
	case *ast.Ident:
		return expr.Name
	case *ast.StarExpr:
		return receiverTypeName(expr.X)
	case *ast.IndexExpr:
		return receiverTypeName(expr.X)
	case *ast.IndexListExpr:
		return receiverTypeName(expr.X)
	case *ast.SelectorExpr:
		return expr.Sel.Name
	default:
		return ""
	}
}

func namedTypeName(typ types.Type) string {
	switch typ := typ.(type) {
	case *types.Named:
		return typ.Obj().Name()
	case *types.Pointer:
		return namedTypeName(typ.Elem())
	case *types.Alias:
		return typ.Obj().Name()
	default:
		return ""
	}
}
