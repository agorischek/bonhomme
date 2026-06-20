import { Code2, Network } from 'lucide-react'
import { CodeBlock } from './CodeBlock'
import { SectionHeader } from './SectionHeader'
import type { RenderedFile, SemanticGraph, SymbolNode } from '../types'

function shortId(id: string) {
  return id ? id.slice(0, 8) : 'unknown'
}

function GraphNode({
  symbol,
  graph,
  depth,
}: {
  symbol: SymbolNode
  graph?: SemanticGraph
  depth: number
}) {
  const children = Object.values(graph?.symbols ?? {})
    .filter((child) => child.parentId === symbol.id)
    .sort((a, b) => a.ordinal - b.ordinal || a.name.localeCompare(b.name))

  return (
    <div>
      <div className="graph-node" style={{ paddingLeft: `${depth * 18}px` }}>
        <span>{symbol.kind}</span>
        <strong>{symbol.name}</strong>
        <code>{shortId(symbol.id)}</code>
      </div>
      {children.map((child) => (
        <GraphNode key={child.id} symbol={child} graph={graph} depth={depth + 1} />
      ))}
    </div>
  )
}

export function Workbench({
  graph,
  graphRoots,
  selectedFile,
  revisionLabel,
}: {
  graph?: SemanticGraph
  graphRoots: SymbolNode[]
  selectedFile?: RenderedFile
  revisionLabel: string
}) {
  return (
    <section className="workbench">
      <div className="section-block graph-block">
        <SectionHeader icon={<Network />} title="Semantic Graph" detail="Materialized from replay" />
        <div className="graph-tree">
          {graphRoots.map((symbol) => (
            <GraphNode key={symbol.id} symbol={symbol} graph={graph} depth={0} />
          ))}
        </div>
      </div>

      <div className="section-block code-block">
        <SectionHeader
          icon={<Code2 />}
          title={selectedFile?.path ?? 'Rendered TypeScript'}
          detail={revisionLabel}
        />
        <CodeBlock
          className="code-pane"
          code={selectedFile?.content ?? 'Waiting for bonhomme API...'}
          path={selectedFile?.path}
        />
      </div>
    </section>
  )
}
