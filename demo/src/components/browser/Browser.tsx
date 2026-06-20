import '@primer/primitives/dist/css/primitives.css'
import '@primer/primitives/dist/css/functional/themes/light.css'
import './browser.css'

import { DatabaseIcon, GitBranchIcon, SyncIcon } from '@primer/octicons-react'
import { BaseStyles, Spinner, ThemeProvider } from '@primer/react'
import { useEffect, useMemo, useState } from 'react'
import { api } from '../../api'
import type { DemoState } from '../../types'
import { branchColor, buildTree } from './graph'
import { Inspector } from './Inspector'
import { SymbolDetail } from './SymbolDetail'
import { SymbolTree } from './SymbolTree'

function Metric({ label, value }: { label: string; value: number }) {
  return (
    <span className="bh-metric">
      <strong>{value}</strong> {label}
    </span>
  )
}

function Timeline({ state }: { state: DemoState }) {
  const ops = state.operations
  return (
    <div className="bh-timeline">
      <div className="bh-pane-title">Operation log — {ops.length} operations</div>
      <div className="bh-ticks">
        {ops.map((op) => (
          <span
            key={op.id}
            className="bh-tick"
            title={`#${op.position} ${op.opType} · ${op.branchName}`}
            style={{ background: branchColor(op.branchName) }}
          />
        ))}
      </div>
    </div>
  )
}

function BrowserBody() {
  const [state, setState] = useState<DemoState | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [selectedId, setSelectedId] = useState<string | null>(null)

  const load = () => {
    setLoading(true)
    api<DemoState>('/api/demo/state')
      .then((next) => {
        setState(next)
        setError(null)
      })
      .catch((err: Error) => setError(err.message))
      .finally(() => setLoading(false))
  }

  useEffect(load, [])

  const roots = useMemo(() => (state ? buildTree(state.mainGraph.symbols) : []), [state])
  const selected = selectedId && state ? (state.mainGraph.symbols[selectedId] ?? null) : null
  const revision = state ? state.mainGraph.appliedOperations.length : 0

  if (loading && !state) {
    return (
      <div className="bh-loading">
        <Spinner /> <span>Loading repository…</span>
      </div>
    )
  }
  if (error) {
    return (
      <div className="bh-error" role="alert">
        Could not reach the bonhomme API: {error}
      </div>
    )
  }
  if (!state) return null

  return (
    <div className="bh-browser">
      <div className="bh-topbar">
        <span className="bh-brand">
          <DatabaseIcon size={16} /> bonhomme
        </span>
        <span className="bh-chip">
          <DatabaseIcon size={14} /> {state.repository.name}
        </span>
        <span className="bh-chip">
          <GitBranchIcon size={14} /> {state.mainBranch.name}
        </span>
        <span className="bh-asof">as of op {revision}</span>
        <div className="bh-spacer" />
        <Metric label="symbols" value={state.metrics.symbolCount} />
        <Metric label="refs" value={state.metrics.referenceCount} />
        <Metric label="branches" value={state.metrics.branchCount} />
        <button className="bh-refresh" onClick={load} title="Refresh">
          <SyncIcon size={15} />
        </button>
      </div>

      <div className="bh-main">
        <div className="bh-pane bh-tree-pane">
          <div className="bh-pane-title">Symbol tree</div>
          <SymbolTree roots={roots} selectedId={selectedId} onSelect={setSelectedId} />
        </div>
        <div className="bh-pane">
          <SymbolDetail state={state} symbol={selected} />
        </div>
        <div className="bh-pane">
          {selected ? (
            <Inspector state={state} symbol={selected} onSelect={setSelectedId} />
          ) : (
            <div className="bh-muted">References, provenance, and history appear here.</div>
          )}
        </div>
      </div>

      <div className="bh-pane">
        <Timeline state={state} />
      </div>
    </div>
  )
}

export function Browser() {
  return (
    <ThemeProvider colorMode="day">
      <BaseStyles>
        <BrowserBody />
      </BaseStyles>
    </ThemeProvider>
  )
}
