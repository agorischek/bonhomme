import type { DemoState, SymbolNode } from '../../types'
import { relatedSymbols } from './graph'

const CX = 300
const CY = 230
const R = 158

export function ReferenceGraph({
  state,
  symbol,
  onSelect,
}: {
  state: DemoState
  symbol: SymbolNode | null
  onSelect: (id: string) => void
}) {
  if (!symbol) {
    return <div className="bh-muted bh-graph-empty">Select a symbol to see its reference graph.</div>
  }

  const { callers, callees } = relatedSymbols(state.mainGraph, symbol.id)
  const seen = new Set<string>([symbol.id])
  const neighbors: { node: SymbolNode; dir: 'in' | 'out' }[] = []
  for (const node of callers) {
    if (!seen.has(node.id)) {
      seen.add(node.id)
      neighbors.push({ node, dir: 'in' })
    }
  }
  for (const node of callees) {
    if (!seen.has(node.id)) {
      seen.add(node.id)
      neighbors.push({ node, dir: 'out' })
    }
  }

  const shown = neighbors.slice(0, 16)
  const placed = shown.map((entry, index) => {
    const angle = (index / shown.length) * Math.PI * 2 - Math.PI / 2
    return {
      ...entry,
      x: CX + R * Math.cos(angle),
      y: CY + R * Math.sin(angle),
      cos: Math.cos(angle),
      sin: Math.sin(angle),
    }
  })

  return (
    <div className="bh-graph">
      <div className="bh-graph-head">
        Reference graph · <strong>{symbol.name}</strong>
        {neighbors.length > shown.length && (
          <span className="bh-muted"> (+{neighbors.length - shown.length} more)</span>
        )}
      </div>
      <svg viewBox="0 0 600 460" className="bh-graph-svg" role="img">
        <defs>
          <marker id="bh-arrow" markerWidth="9" markerHeight="9" refX="8" refY="3" orient="auto">
            <path d="M0,0 L8,3 L0,6 Z" fill="#8c959f" />
          </marker>
        </defs>
        {placed.map((entry) => {
          const [x1, y1, x2, y2] =
            entry.dir === 'out' ? [CX, CY, entry.x, entry.y] : [entry.x, entry.y, CX, CY]
          return (
            <line
              key={`edge-${entry.node.id}`}
              x1={x1}
              y1={y1}
              x2={x2}
              y2={y2}
              stroke="#d0d7de"
              strokeWidth={1.5}
              markerEnd="url(#bh-arrow)"
            />
          )
        })}
        {placed.map((entry) => (
          <g
            key={entry.node.id}
            style={{ cursor: 'pointer' }}
            onClick={() => onSelect(entry.node.id)}
          >
            <circle cx={entry.x} cy={entry.y} r={6} fill={entry.dir === 'in' ? '#8250df' : '#1f883d'} />
            <text
              x={entry.x + entry.cos * 11}
              y={entry.y + entry.sin * 11 + 4}
              textAnchor={entry.cos > 0.3 ? 'start' : entry.cos < -0.3 ? 'end' : 'middle'}
              fontSize={11}
              fill="#59636e"
            >
              {entry.node.name.length > 16 ? `${entry.node.name.slice(0, 15)}…` : entry.node.name}
            </text>
          </g>
        ))}
        <circle cx={CX} cy={CY} r={9} fill="#0969da" />
        <text x={CX} y={CY - 16} textAnchor="middle" fontSize={13} fontWeight={600} fill="#1f2328">
          {symbol.name}
        </text>
        {neighbors.length === 0 && (
          <text x={CX} y={CY + 28} textAnchor="middle" fontSize={12} fill="#59636e">
            no references
          </text>
        )}
      </svg>
      <div className="bh-graph-legend">
        <span>
          <i style={{ background: '#8250df' }} /> caller
        </span>
        <span>
          <i style={{ background: '#1f883d' }} /> callee
        </span>
      </div>
    </div>
  )
}
