import { Activity, CheckCircle2, CircleDotDashed, GitMerge, LoaderCircle, XCircle } from 'lucide-react'
import { SectionHeader } from './SectionHeader'
import type { BranchSummary } from '../types'

function statusLabel(status: BranchSummary['status']) {
  if (status === 'conflicted') return 'conflict'
  if (status === 'empty') return 'editing'
  if (status === 'ready') return 'awaiting merge'
  if (status === 'merged') return 'landed'
  return status
}

function statusTitle(status: BranchSummary['status']) {
  if (status === 'conflicted') return 'Merge found a semantic conflict that needs review'
  if (status === 'empty') return 'Agent is editing its isolated slice'
  if (status === 'ready') return 'Semantic operations were recovered and this branch is waiting to merge'
  if (status === 'merged') return 'Branch has landed in main'
  return status
}

function StatusIcon({ status }: { status: BranchSummary['status'] }) {
  if (status === 'conflicted') return <XCircle aria-hidden="true" />
  if (status === 'empty') return <LoaderCircle aria-hidden="true" />
  if (status === 'ready') return <CircleDotDashed aria-hidden="true" />
  if (status === 'merged') return <CheckCircle2 aria-hidden="true" />
  return <Activity aria-hidden="true" />
}

function AgentCard({
  branch,
  onMerge,
  disabled,
}: {
  branch: BranchSummary
  onMerge: () => void
  disabled: boolean
}) {
  const status = branch.status === 'conflicted' ? 'conflict' : branch.status
  const label = statusLabel(branch.status)
  const touched = branch.touchedSymbolNames.length
    ? branch.touchedSymbolNames.join(', ')
    : branch.createdMethodNames[0] ?? 'OrderService'
  const operationTypes = branch.operationTypes.length
    ? [...new Set(branch.operationTypes)].join(' + ')
    : 'working copy'
  const operationNoun = branch.ownOperationCount === 1 ? 'semantic op' : 'semantic ops'
  const operationLabel =
    branch.status === 'empty'
      ? 'editing isolated slice'
      : `${branch.ownOperationCount} ${operationNoun} recovered`
  const compactOperationLabel =
    branch.status === 'empty'
      ? 'editing slice'
      : `${branch.ownOperationCount} ${branch.ownOperationCount === 1 ? 'op' : 'ops'} · ${touched}`
  const canMerge = branch.status === 'ready'
  const cardTitle = `${branch.name}: ${label}; ${operationLabel}; ${operationTypes}; ${touched}`

  return (
    <article className={`agent-card ${status}`} title={cardTitle}>
      <div className="agent-topline">
        <strong>{branch.name}</strong>
        <div className="agent-actions">
          <span className="status-pill" title={statusTitle(branch.status)}>
            <StatusIcon status={branch.status} />
            {label}
          </span>
          {canMerge && (
            <button
              className="agent-merge-button"
              type="button"
              disabled={disabled}
              onClick={onMerge}
              title="Merge this branch into main"
            >
              <GitMerge aria-hidden="true" />
              Merge
            </button>
          )}
        </div>
      </div>
      <div className="agent-meta">
        <code>codex 5.3 spark</code>
        <span>{compactOperationLabel}</span>
      </div>
    </article>
  )
}

export function AgentEditors({
  agents,
  readyCount,
  conflictCount,
  onMerge,
  disabled,
}: {
  agents: BranchSummary[]
  readyCount: number
  conflictCount: number
  onMerge: (branch: BranchSummary) => void
  disabled: boolean
}) {
  return (
    <section className="section-block">
      <SectionHeader
        icon={<Activity />}
        title="Agents"
        detail={`${readyCount} awaiting merge, ${conflictCount} held for conflict`}
      />
      <div className="agent-grid">
        {agents.map((branch) => (
          <AgentCard
            key={branch.id}
            branch={branch}
            onMerge={() => onMerge(branch)}
            disabled={disabled}
          />
        ))}
      </div>
    </section>
  )
}
