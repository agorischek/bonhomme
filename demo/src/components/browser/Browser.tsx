import '@primer/primitives/dist/css/primitives.css'
import '@primer/primitives/dist/css/functional/themes/light.css'
import './browser.css'

import { DatabaseIcon, GitBranchIcon, SyncIcon } from '@primer/octicons-react'
import { BaseStyles, Spinner, ThemeProvider } from '@primer/react'
import { useEffect, useMemo, useState } from 'react'
import { api } from '../../api'
import type { DemoState } from '../../types'
import { BranchDashboard } from './BranchDashboard'
import { ChangesetReview } from './ChangesetReview'
import { branchColor, buildTree, filterGraphAsOf } from './graph'
import { Inspector } from './Inspector'
import { ReferenceGraph } from './ReferenceGraph'
import { SymbolDetail } from './SymbolDetail'
import { SymbolTree } from './SymbolTree'

type Mode = 'explorer' | 'branches' | 'changesets' | 'graph'

const MODE_LABEL: Record<Mode, string> = {
  explorer: 'Explorer',
  branches: 'Branches',
  changesets: 'Changesets',
  graph: 'Graph',
}

function Metric({ label, value }: { label: string; value: number }) {
  return (
    <span className="bh-metric">
      <strong>{value}</strong> {label}
    </span>
  )
}

function Scrubber({
  state,
  asOf,
  max,
  onChange,
}: {
  state: DemoState
  asOf: number
  max: number
  onChange: (value: number) => void
}) {
  return (
    <div className="bh-timeline">
      <div className="bh-pane-title">
        Operation log — {state.operations.length} operations · viewing op {asOf} / {max}
      </div>
      <input
        type="range"
        className="bh-scrubber"
        min={1}
        max={max}
        value={asOf}
        aria-label="Time travel through the operation log"
        onChange={(event) => onChange(Number(event.target.value))}
      />
      <div className="bh-ticks">
        {state.operations.map((op) => (
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
  const [mode, setMode] = useState<Mode>('explorer')
  const [asOf, setAsOf] = useState<number | null>(null)

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

  useEffect(() => {
    let cancelled = false
    api<DemoState>('/api/demo/state')
      .then((next) => {
        if (cancelled) return
        setState(next)
        setError(null)
      })
      .catch((err: Error) => {
        if (!cancelled) setError(err.message)
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [])

  const maxOrdinal = state ? state.mainGraph.appliedOperations.length : 0
  const effectiveAsOf = asOf ?? maxOrdinal

  // Explorer / Graph view the graph as of the scrubber position; Branches / Changesets are
  // meta views over the full repository, so they read the unfiltered state.
  const viewState = useMemo(() => {
    if (!state) return null
    return effectiveAsOf < maxOrdinal ? filterGraphAsOf(state, effectiveAsOf) : state
  }, [state, effectiveAsOf, maxOrdinal])

  const roots = useMemo(
    () => (viewState ? buildTree(viewState.mainGraph.symbols) : []),
    [viewState],
  )
  const selected =
    selectedId && viewState ? (viewState.mainGraph.symbols[selectedId] ?? null) : null

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
  if (!state || !viewState) return null

  return (
    <div className="bh-browser">
      <div className="bh-topbar">
        <span className="bh-brand">
          <DatabaseIcon size={16} /> bonhomme
        </span>
        <div className="bh-toggle bh-view-toggle">
          {(Object.keys(MODE_LABEL) as Mode[]).map((option) => (
            <button
              key={option}
              type="button"
              className={mode === option ? 'bh-toggle-on' : undefined}
              aria-pressed={mode === option}
              onClick={() => setMode(option)}
            >
              {MODE_LABEL[option]}
            </button>
          ))}
        </div>
        <span className="bh-chip">
          <DatabaseIcon size={14} /> {state.repository.name}
        </span>
        <span className="bh-chip">
          <GitBranchIcon size={14} /> {state.mainBranch.name}
        </span>
        <span className="bh-asof">as of op {effectiveAsOf}</span>
        <div className="bh-spacer" />
        <Metric label="symbols" value={Object.keys(viewState.mainGraph.symbols).length} />
        <Metric label="refs" value={Object.keys(viewState.mainGraph.references).length} />
        <Metric label="branches" value={state.metrics.branchCount} />
        <button className="bh-refresh" onClick={load} title="Refresh">
          <SyncIcon size={15} />
        </button>
      </div>

      {mode === 'explorer' && (
        <div className="bh-main">
          <div className="bh-pane bh-tree-pane">
            <div className="bh-pane-title">Symbol tree</div>
            <SymbolTree roots={roots} selectedId={selectedId} onSelect={setSelectedId} />
          </div>
          <div className="bh-pane">
            <SymbolDetail state={viewState} symbol={selected} />
          </div>
          <div className="bh-pane">
            {selected ? (
              <Inspector state={viewState} symbol={selected} onSelect={setSelectedId} />
            ) : (
              <div className="bh-muted">References, provenance, and history appear here.</div>
            )}
          </div>
        </div>
      )}

      {mode === 'branches' && (
        <div className="bh-pane bh-branches-pane">
          <div className="bh-pane-title">Branches · {state.branches.length}</div>
          <BranchDashboard
            state={state}
            onSelectSymbol={(id) => {
              setSelectedId(id)
              setMode('explorer')
            }}
          />
        </div>
      )}

      {mode === 'changesets' && (
        <div className="bh-pane bh-branches-pane">
          <div className="bh-pane-title">Changesets · {state.changesets.length}</div>
          <ChangesetReview
            state={state}
            onSelectSymbol={(id) => {
              setSelectedId(id)
              setMode('explorer')
            }}
          />
        </div>
      )}

      {mode === 'graph' && (
        <div className="bh-main bh-graph-layout">
          <div className="bh-pane bh-tree-pane">
            <div className="bh-pane-title">Symbol tree</div>
            <SymbolTree roots={roots} selectedId={selectedId} onSelect={setSelectedId} />
          </div>
          <div className="bh-pane">
            <ReferenceGraph state={viewState} symbol={selected} onSelect={setSelectedId} />
          </div>
        </div>
      )}

      <div className="bh-pane">
        <Scrubber
          state={state}
          asOf={effectiveAsOf}
          max={Math.max(maxOrdinal, 1)}
          onChange={(value) => setAsOf(value >= maxOrdinal ? null : value)}
        />
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
