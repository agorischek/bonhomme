import { Activity, GitMerge } from 'lucide-react'
import { SectionHeader } from './SectionHeader'
import type { BranchSummary } from '../types'

function AgentCard({
  branch,
  conflicted,
  onMerge,
  disabled,
}: {
  branch: BranchSummary
  conflicted: boolean
  onMerge: () => void
  disabled: boolean
}) {
  const methodName = branch.createdMethodNames[0] ?? 'pendingMethod'
  const status = conflicted ? 'conflict' : branch.status

  return (
    <article className={`agent-card ${status}`}>
      <div className="agent-topline">
        <strong>{branch.name}</strong>
        <span>{status}</span>
      </div>
      <pre>{`slice OrderService\n+ ${methodName}(): string\n+ refs displayName(), listOrders()`}</pre>
      <button
        type="button"
        disabled={disabled || branch.status !== 'ready' || conflicted}
        onClick={onMerge}
        title="Merge this branch"
      >
        <GitMerge aria-hidden="true" />
        Merge
      </button>
    </article>
  )
}

export function AgentEditors({
  agents,
  readyCount,
  conflictCount,
  conflictedBranches,
  onMerge,
  disabled,
}: {
  agents: BranchSummary[]
  readyCount: number
  conflictCount: number
  conflictedBranches: Set<string>
  onMerge: (branch: BranchSummary) => void
  disabled: boolean
}) {
  return (
    <section className="section-block">
      <SectionHeader
        icon={<Activity />}
        title="Agent Editors"
        detail={`${readyCount} ready, ${conflictCount} conflict-held`}
      />
      <div className="agent-grid">
        {agents.map((branch) => (
          <AgentCard
            key={branch.id}
            branch={branch}
            conflicted={conflictedBranches.has(branch.name)}
            onMerge={() => onMerge(branch)}
            disabled={disabled}
          />
        ))}
      </div>
    </section>
  )
}
