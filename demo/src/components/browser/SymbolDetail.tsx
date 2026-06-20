import { ChevronRightIcon } from '@primer/octicons-react'
import { Label } from '@primer/react'
import { Fragment, useState } from 'react'
import type { DemoState, SymbolNode } from '../../types'
import { fileForSymbol, metaBool, metaString, pathOf, relatedSymbols } from './graph'
import { KindIcon } from './KindIcon'

type DetailMode = 'code' | 'semantic'

function SemanticRow({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="bh-sem-row">
      <span className="bh-sem-label">{label}</span>
      <span className={mono ? 'bh-sem-value bh-mono' : 'bh-sem-value'}>{value}</span>
    </div>
  )
}

function SemanticView({ state, symbol }: { state: DemoState; symbol: SymbolNode }) {
  const symbols = state.mainGraph.symbols
  const children = Object.values(symbols)
    .filter((candidate) => candidate.parentId === symbol.id)
    .sort((a, b) => a.ordinal - b.ordinal)
  const related = relatedSymbols(state.mainGraph, symbol.id)
  const signature = metaString(symbol, 'signature') ?? metaString(symbol, 'declaration')

  return (
    <div className="bh-semantic">
      <SemanticRow label="kind" value={symbol.kind} />
      <SemanticRow label="name" value={symbol.name} />
      {signature && <SemanticRow label="signature" value={signature} mono />}
      <SemanticRow label="exported" value={metaBool(symbol, 'exported') ? 'yes' : 'no'} />
      {symbol.body != null && (
        <SemanticRow label="body" value={`${symbol.body.split('\n').length} line(s)`} />
      )}
      <SemanticRow
        label="children"
        value={
          children.length
            ? children.map((child) => `${child.name} (${child.kind})`).join(', ')
            : 'none'
        }
      />
      <SemanticRow
        label="references"
        value={`${related.callers.length} callers · ${related.callees.length} callees · ${related.dependencies.length} deps`}
      />
      <SemanticRow label="symbol id" value={symbol.id} mono />
    </div>
  )
}

export function SymbolDetail({ state, symbol }: { state: DemoState; symbol: SymbolNode | null }) {
  const [mode, setMode] = useState<DetailMode>('code')

  if (!symbol) {
    return (
      <div className="bh-empty">
        <KindIcon kind="file" size={28} />
        <p>Select a symbol to inspect its code, references, and provenance.</p>
      </div>
    )
  }

  const symbols = state.mainGraph.symbols
  const breadcrumb = pathOf(symbols, symbol.id)
  const file = fileForSymbol(symbols, symbol.id)
  const filePath = file ? (metaString(file, 'path') ?? file.name) : undefined
  const rendered = filePath
    ? state.renderedFiles.find((entry) => entry.path === filePath)
    : undefined
  const signature = metaString(symbol, 'signature') ?? metaString(symbol, 'declaration')

  return (
    <div className="bh-detail">
      <div className="bh-detail-head">
        <div className="bh-breadcrumb">
          {breadcrumb.map((node, index) => (
            <Fragment key={node.id}>
              {index > 0 && <ChevronRightIcon size={12} />}
              <span className={node.id === symbol.id ? 'bh-breadcrumb-current' : undefined}>
                {node.name}
              </span>
            </Fragment>
          ))}
        </div>
        <div className="bh-toggle">
          {(['code', 'semantic'] as const).map((option) => (
            <button
              key={option}
              type="button"
              className={mode === option ? 'bh-toggle-on' : undefined}
              aria-pressed={mode === option}
              onClick={() => setMode(option)}
            >
              {option === 'code' ? 'Code' : 'Semantic'}
            </button>
          ))}
        </div>
      </div>

      <div className="bh-meta">
        <Label variant="secondary">
          <KindIcon kind={symbol.kind} size={12} /> {symbol.kind}
        </Label>
        {metaBool(symbol, 'exported') && <Label variant="accent">exported</Label>}
        {signature && <span className="bh-meta-sig">{signature}</span>}
        <span className="bh-meta-id" title={symbol.id}>
          {symbol.id.slice(0, 8)}…
        </span>
      </div>

      {mode === 'semantic' ? (
        <SemanticView state={state} symbol={symbol} />
      ) : rendered ? (
        <>
          <div className="bh-code-path">{rendered.path}</div>
          <pre className="bh-code">{rendered.content}</pre>
        </>
      ) : symbol.body ? (
        <pre className="bh-code">{symbol.body}</pre>
      ) : (
        <div className="bh-muted">No rendered projection for this symbol.</div>
      )}
    </div>
  )
}
