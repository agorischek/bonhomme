import { GitBranchIcon } from '@primer/octicons-react'
import { Label } from '@primer/react'
import type { LabelProps } from '@primer/react'
import type { BranchStatus, DemoState } from '../../types'

const STATUS_VARIANT: Record<BranchStatus, LabelProps['variant']> = {
  main: 'secondary',
  empty: 'default',
  ready: 'accent',
  merged: 'success',
}

export function BranchDashboard({
  state,
  onSelectSymbol,
}: {
  state: DemoState
  onSelectSymbol: (id: string) => void
}) {
  const branches = [...state.branches].sort((a, b) => {
    if (a.status === 'main') return -1
    if (b.status === 'main') return 1
    return a.name.localeCompare(b.name)
  })

  const symbolByName = (name: string) =>
    Object.values(state.mainGraph.symbols).find((symbol) => symbol.name === name)

  return (
    <div className="bh-branches">
      {branches.map((branch) => (
        <div className="bh-branch-card" key={branch.id}>
          <div className="bh-branch-head">
            <GitBranchIcon size={15} />
            <strong>{branch.name}</strong>
            <Label variant={STATUS_VARIANT[branch.status]}>{branch.status}</Label>
          </div>
          <div className="bh-branch-meta">
            by <code>{branch.createdBy}</code> · {branch.ownOperationCount} ops ·{' '}
            {branch.createdSymbolCount} symbols
          </div>
          {branch.createdMethodNames.length > 0 && (
            <div className="bh-branch-chips">
              {branch.createdMethodNames.map((name) => {
                const symbol = symbolByName(name)
                return (
                  <button
                    type="button"
                    key={name}
                    className="bh-method-chip"
                    disabled={!symbol}
                    title={symbol ? `Go to ${name}` : `${name} (not on main)`}
                    onClick={() => symbol && onSelectSymbol(symbol.id)}
                  >
                    {name}
                  </button>
                )
              })}
            </div>
          )}
        </div>
      ))}
    </div>
  )
}
