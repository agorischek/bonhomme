import { BarChart3, Code2, Database, GitMerge, Network, type LucideIcon } from 'lucide-react'
import { useState } from 'react'
import { CodeBlock } from './CodeBlock'
import { MergeReview, OperationLog } from './Logs'
import { MetricsStrip } from './MetricsStrip'
import { SectionHeader } from './SectionHeader'
import type {
  DemoMetrics,
  MergeResult,
  OperationView,
  RenderedFile,
  SemanticGraph,
  SimulationResult,
  SymbolNode,
} from '../types'

type InspectorTab = 'code' | 'graph' | 'operations' | 'merges' | 'metrics'

const inspectorTabs: Array<{
  id: InspectorTab
  label: string
  icon: LucideIcon
}> = [
  { id: 'code', label: 'Code', icon: Code2 },
  { id: 'graph', label: 'Graph', icon: Network },
  { id: 'operations', label: 'Operations', icon: Database },
  { id: 'merges', label: 'Merges', icon: GitMerge },
  { id: 'metrics', label: 'Metrics', icon: BarChart3 },
]

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
  metrics,
  operations,
  mergeResults,
  simulationResult,
}: {
  graph?: SemanticGraph
  graphRoots: SymbolNode[]
  selectedFile?: RenderedFile
  revisionLabel: string
  metrics?: DemoMetrics
  operations: OperationView[]
  mergeResults: MergeResult[]
  simulationResult: SimulationResult | null
}) {
  const [activeTab, setActiveTab] = useState<InspectorTab>('code')

  return (
    <section className="inspector-column" aria-label="Workbench inspector">
      <div className="inspector-tabs" role="tablist" aria-label="Workbench views">
        {inspectorTabs.map((tab) => {
          const Icon = tab.icon
          return (
            <button
              key={tab.id}
              type="button"
              role="tab"
              aria-selected={activeTab === tab.id}
              onClick={() => setActiveTab(tab.id)}
            >
              <Icon aria-hidden="true" />
              {tab.label}
            </button>
          )
        })}
      </div>

      <div className="inspector-body">
        {activeTab === 'code' && (
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
        )}

        {activeTab === 'graph' && (
          <div className="section-block graph-block">
            <SectionHeader icon={<Network />} title="Semantic Graph" detail="Materialized from replay" />
            <div className="graph-tree">
              {graphRoots.map((symbol) => (
                <GraphNode key={symbol.id} symbol={symbol} graph={graph} depth={0} />
              ))}
            </div>
          </div>
        )}

        {activeTab === 'operations' && (
          <div className="section-block">
            <SectionHeader icon={<Database />} title="Operation Log" detail="Append-only" />
            <OperationLog operations={operations} />
          </div>
        )}

        {activeTab === 'merges' && (
          <div className="section-block">
            <SectionHeader icon={<GitMerge />} title="Merge Review" detail="Operations, not lines" />
            <MergeReview simulationResult={simulationResult} mergeResults={mergeResults} />
          </div>
        )}

        {activeTab === 'metrics' && (
          <div className="section-block metrics-block">
            <SectionHeader icon={<BarChart3 />} title="Repository Metrics" detail="Current state" />
            <MetricsStrip metrics={metrics} />
          </div>
        )}
      </div>
    </section>
  )
}
