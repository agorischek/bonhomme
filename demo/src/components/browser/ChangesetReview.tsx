import type { DemoState, OperationView } from '../../types'
import { KindIcon } from './KindIcon'

const OP_COLOR: Record<string, string> = {
  CreateSymbol: '#1f883d',
  UpdateSymbol: '#0969da',
  DeleteSymbol: '#cf222e',
  CreateReference: '#8250df',
  DeleteReference: '#bc4c00',
}

function OpRow({
  op,
  onSelect,
}: {
  op: OperationView
  onSelect: (id: string) => void
}) {
  const label = op.symbolName ?? op.symbolKind ?? op.symbolId?.slice(0, 8) ?? '—'
  return (
    <li className="bh-cs-op">
      <span className="bh-op-tag" style={{ background: OP_COLOR[op.opType] ?? '#59636e' }}>
        {op.opType.replace('Symbol', '').replace('Reference', 'Ref')}
      </span>
      {op.symbolKind && op.symbolKind !== 'symbol' && op.symbolKind !== 'reference' && (
        <KindIcon kind={op.symbolKind} size={13} />
      )}
      {op.symbolId ? (
        <button type="button" className="bh-cs-op-name" onClick={() => onSelect(op.symbolId!)}>
          {label}
        </button>
      ) : (
        <span className="bh-cs-op-name">{label}</span>
      )}
    </li>
  )
}

export function ChangesetReview({
  state,
  onSelectSymbol,
}: {
  state: DemoState
  onSelectSymbol: (id: string) => void
}) {
  const taskById = new Map(state.tasks.map((task) => [task.id, task]))
  const opsByChangeset = new Map<string, OperationView[]>()
  for (const op of state.operations) {
    const list = opsByChangeset.get(op.changesetId) ?? []
    list.push(op)
    opsByChangeset.set(op.changesetId, list)
  }

  const changesets = [...state.changesets].sort((a, b) =>
    a.createdAt === b.createdAt ? a.id.localeCompare(b.id) : a.createdAt.localeCompare(b.createdAt),
  )

  if (changesets.length === 0) {
    return <div className="bh-muted">No changesets yet.</div>
  }

  return (
    <div className="bh-changesets">
      {changesets.map((changeset) => {
        const ops = (opsByChangeset.get(changeset.id) ?? []).sort((a, b) => a.position - b.position)
        const task = taskById.get(changeset.taskId)
        return (
          <div className="bh-cs-card" key={changeset.id}>
            <div className="bh-cs-head">
              <strong>{changeset.title}</strong>
              <span className="bh-cs-author">{changeset.createdBy}</span>
            </div>
            {task && <div className="bh-muted bh-cs-task">{task.title}</div>}
            {ops.length > 0 ? (
              <ul className="bh-cs-ops">
                {ops.map((op) => (
                  <OpRow key={op.id} op={op} onSelect={onSelectSymbol} />
                ))}
              </ul>
            ) : (
              <div className="bh-muted">no operations</div>
            )}
          </div>
        )
      })}
    </div>
  )
}
