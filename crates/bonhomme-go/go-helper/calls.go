package main

import (
	"go/ast"
	"go/types"
	"sort"
)

func functionCalls(body *ast.BlockStmt, info *types.Info, pkg *types.Package) []callTarget {
	seen := map[string]callTarget{}
	ast.Inspect(body, func(node ast.Node) bool {
		call, ok := node.(*ast.CallExpr)
		if !ok {
			return true
		}
		target, ok := callTargetFor(call.Fun, info, pkg)
		if !ok {
			return true
		}
		key := target.Kind + ":" + target.Receiver + ":" + target.Name
		seen[key] = target
		return true
	})
	keys := make([]string, 0, len(seen))
	for key := range seen {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	calls := make([]callTarget, 0, len(keys))
	for _, key := range keys {
		calls = append(calls, seen[key])
	}
	return calls
}

func callTargetFor(expr ast.Expr, info *types.Info, pkg *types.Package) (callTarget, bool) {
	switch expr := expr.(type) {
	case *ast.Ident:
		return identCallTarget(expr, info, pkg)
	case *ast.SelectorExpr:
		return selectorCallTarget(expr, info, pkg)
	default:
		return callTarget{}, false
	}
}

func identCallTarget(expr *ast.Ident, info *types.Info, pkg *types.Package) (callTarget, bool) {
	fn, ok := info.Uses[expr].(*types.Func)
	if !ok || !samePackage(fn.Pkg(), pkg) {
		return callTarget{}, false
	}
	sig, _ := fn.Type().(*types.Signature)
	if sig == nil || sig.Recv() != nil {
		return callTarget{}, false
	}
	return callTarget{Kind: "function", Name: fn.Name()}, true
}

func selectorCallTarget(
	expr *ast.SelectorExpr,
	info *types.Info,
	pkg *types.Package,
) (callTarget, bool) {
	if selection := info.Selections[expr]; selection != nil {
		return methodCallTarget(selection, pkg)
	}
	fn, ok := info.Uses[expr.Sel].(*types.Func)
	if !ok || !samePackage(fn.Pkg(), pkg) {
		return callTarget{}, false
	}
	if sig, _ := fn.Type().(*types.Signature); sig != nil && sig.Recv() == nil {
		return callTarget{Kind: "function", Name: fn.Name()}, true
	}
	return callTarget{}, false
}

func methodCallTarget(selection *types.Selection, pkg *types.Package) (callTarget, bool) {
	fn, ok := selection.Obj().(*types.Func)
	if !ok || !samePackage(fn.Pkg(), pkg) {
		return callTarget{}, false
	}
	recv := ""
	if sig, _ := fn.Type().(*types.Signature); sig != nil && sig.Recv() != nil {
		recv = namedTypeName(sig.Recv().Type())
	}
	if recv == "" {
		recv = namedTypeName(selection.Recv())
	}
	if recv == "" {
		return callTarget{}, false
	}
	return callTarget{Kind: "method", Receiver: recv, Name: fn.Name()}, true
}

func samePackage(left *types.Package, right *types.Package) bool {
	if left == nil || right == nil {
		return false
	}
	return left.Path() == right.Path()
}
