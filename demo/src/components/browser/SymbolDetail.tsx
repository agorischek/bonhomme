import { ChevronRightIcon } from '@primer/octicons-react'
import { Label } from '@primer/react'
import { Fragment } from 'react'
import type { DemoState, SymbolNode } from '../../types'
import { fileForSymbol, metaBool, metaString, pathOf } from './graph'
import { KindIcon } from './KindIcon'

export function SymbolDetail({
  state,
  symbol,
}: {
  state: DemoState
  symbol: SymbolNode | null
}) {
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
  const exported = metaBool(symbol, 'exported')

  return (
    <div className="bh-detail">
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

      <div className="bh-meta">
        <Label variant="secondary">
          <KindIcon kind={symbol.kind} size={12} /> {symbol.kind}
        </Label>
        {exported && <Label variant="accent">exported</Label>}
        {signature && <span className="bh-meta-sig">{signature}</span>}
        <span className="bh-meta-id" title={symbol.id}>
          {symbol.id.slice(0, 8)}…
        </span>
      </div>

      {rendered ? (
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
