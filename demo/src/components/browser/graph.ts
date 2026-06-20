import type { DemoState, OperationView, SemanticGraph, SymbolNode } from '../../types'

export interface SymbolTreeNode extends SymbolNode {
  children: SymbolTreeNode[]
}

const compare = (a: SymbolTreeNode, b: SymbolTreeNode) =>
  a.ordinal - b.ordinal || a.kind.localeCompare(b.kind) || a.name.localeCompare(b.name)

/** Build the containment tree (roots = symbols with no parent), sorted by ordinal. */
export function buildTree(symbols: Record<string, SymbolNode>): SymbolTreeNode[] {
  const byId = new Map<string, SymbolTreeNode>()
  for (const symbol of Object.values(symbols)) byId.set(symbol.id, { ...symbol, children: [] })

  const roots: SymbolTreeNode[] = []
  for (const node of byId.values()) {
    const parent = node.parentId ? byId.get(node.parentId) : undefined
    if (parent) parent.children.push(node)
    else roots.push(node)
  }

  const sortDeep = (nodes: SymbolTreeNode[]) => {
    nodes.sort(compare)
    for (const node of nodes) sortDeep(node.children)
  }
  sortDeep(roots)
  return roots
}

/** Names from the root file down to this symbol — the breadcrumb. */
export function pathOf(symbols: Record<string, SymbolNode>, id: string): SymbolNode[] {
  const path: SymbolNode[] = []
  let current: SymbolNode | undefined = symbols[id]
  while (current) {
    path.unshift(current)
    current = current.parentId ? symbols[current.parentId] : undefined
  }
  return path
}

/** Walk up to the enclosing `file` symbol (for locating the rendered projection). */
export function fileForSymbol(
  symbols: Record<string, SymbolNode>,
  id: string | undefined,
): SymbolNode | undefined {
  let current = id ? symbols[id] : undefined
  while (current) {
    if (current.kind === 'file') return current
    current = current.parentId ? symbols[current.parentId] : undefined
  }
  return undefined
}

export function metaString(symbol: SymbolNode | undefined, key: string): string | undefined {
  const value = symbol?.metadata?.[key]
  return typeof value === 'string' ? value : undefined
}

export function metaBool(symbol: SymbolNode | undefined, key: string): boolean {
  return symbol?.metadata?.[key] === true
}

export interface RelatedSymbols {
  callers: SymbolNode[]
  callees: SymbolNode[]
  dependencies: SymbolNode[]
  dependents: SymbolNode[]
}

/** Resolve the reference edges around a symbol into navigable symbol lists. */
export function relatedSymbols(graph: SemanticGraph, id: string): RelatedSymbols {
  const refs = Object.values(graph.references)
  const lookup = (sid: string) => graph.symbols[sid]
  const dedupe = (symbols: (SymbolNode | undefined)[]): SymbolNode[] => {
    const seen = new Set<string>()
    const out: SymbolNode[] = []
    for (const symbol of symbols) {
      if (symbol && !seen.has(symbol.id)) {
        seen.add(symbol.id)
        out.push(symbol)
      }
    }
    return out.sort((a, b) => a.name.localeCompare(b.name))
  }

  return {
    callers: dedupe(
      refs.filter((r) => r.kind === 'calls' && r.toSymbolId === id).map((r) => lookup(r.fromSymbolId)),
    ),
    callees: dedupe(
      refs.filter((r) => r.kind === 'calls' && r.fromSymbolId === id).map((r) => lookup(r.toSymbolId)),
    ),
    dependencies: dedupe(
      refs.filter((r) => r.fromSymbolId === id).map((r) => lookup(r.toSymbolId)),
    ),
    dependents: dedupe(refs.filter((r) => r.toSymbolId === id).map((r) => lookup(r.fromSymbolId))),
  }
}

/** Operations that touched this symbol id, in log order — "semantic blame". */
export function historyFor(state: DemoState, id: string): OperationView[] {
  return state.operations
    .filter((operation) => operation.symbolId === id)
    .sort((a, b) => a.position - b.position)
}

/** The task / changeset / author that introduced this symbol. */
export function provenanceFor(state: DemoState, id: string) {
  const created = state.operations.find(
    (operation) => operation.symbolId === id && operation.opType === 'CreateSymbol',
  )
  const changeset = created ? state.changesets?.find((c) => c.id === created.changesetId) : undefined
  const task = changeset ? state.tasks?.find((t) => t.id === changeset.taskId) : undefined
  return { created, changeset, task }
}

/** A stable color per branch for timeline ticks / blame dots. */
export function branchColor(branchName: string): string {
  if (branchName === 'main') return '#1f883d'
  let hash = 0
  for (const char of branchName) hash = (hash * 31 + char.charCodeAt(0)) >>> 0
  const hues = [212, 280, 36, 0, 330, 190, 150]
  return `hsl(${hues[hash % hues.length]} 70% 45%)`
}
