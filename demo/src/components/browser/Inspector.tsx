import { ArrowLeftIcon, ArrowRightIcon, ChecklistIcon, PersonIcon } from '@primer/octicons-react'
import { ActionList } from '@primer/react'
import type { ReactNode } from 'react'
import type { DemoState, SymbolNode } from '../../types'
import { branchColor, historyFor, provenanceFor, relatedSymbols } from './graph'
import { KindIcon } from './KindIcon'

function RefGroup({
  title,
  icon,
  symbols,
  onSelect,
}: {
  title: string
  icon: ReactNode
  symbols: SymbolNode[]
  onSelect: (id: string) => void
}) {
  return (
    <div className="bh-ref-group">
      <div className="bh-ref-title">
        {icon} {title} <span className="bh-count">{symbols.length}</span>
      </div>
      {symbols.length === 0 ? (
        <div className="bh-muted bh-ref-empty">none</div>
      ) : (
        <ActionList>
          {symbols.map((symbol) => (
            <ActionList.Item key={symbol.id} onSelect={() => onSelect(symbol.id)}>
              <ActionList.LeadingVisual>
                <KindIcon kind={symbol.kind} size={14} />
              </ActionList.LeadingVisual>
              {symbol.name}
            </ActionList.Item>
          ))}
        </ActionList>
      )}
    </div>
  )
}

export function Inspector({
  state,
  symbol,
  onSelect,
}: {
  state: DemoState
  symbol: SymbolNode | null
  onSelect: (id: string) => void
}) {
  if (!symbol) return null

  const related = relatedSymbols(state.mainGraph, symbol.id)
  const { created, changeset, task } = provenanceFor(state, symbol.id)
  const history = historyFor(state, symbol.id)

  return (
    <div className="bh-inspector">
      <section>
        <div className="bh-pane-title">References</div>
        <RefGroup
          title="Called by"
          icon={<ArrowLeftIcon size={13} />}
          symbols={related.callers}
          onSelect={onSelect}
        />
        <RefGroup
          title="Calls"
          icon={<ArrowRightIcon size={13} />}
          symbols={related.callees}
          onSelect={onSelect}
        />
        {related.dependents.length > 0 && (
          <RefGroup
            title="Dependents"
            icon={<ArrowLeftIcon size={13} />}
            symbols={related.dependents}
            onSelect={onSelect}
          />
        )}
      </section>

      <section>
        <div className="bh-pane-title">Provenance</div>
        <div className="bh-prov">
          <div>
            <ChecklistIcon size={14} /> {task?.title ?? 'unknown task'}
          </div>
          <div>
            <PersonIcon size={14} /> by <strong>{changeset?.createdBy ?? created?.branchName ?? 'unknown'}</strong>
          </div>
          {created && (
            <div className="bh-muted">
              created on <code>{created.branchName}</code>
            </div>
          )}
        </div>
      </section>

      <section>
        <div className="bh-pane-title">History · semantic blame</div>
        {history.length === 0 ? (
          <div className="bh-muted">no operations recorded</div>
        ) : (
          <ul className="bh-history">
            {history.map((op) => (
              <li key={op.id}>
                <span className="bh-dot" style={{ background: branchColor(op.branchName) }} />
                <span className="bh-op">{op.opType}</span>
                <code>{op.branchName}</code>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  )
}
