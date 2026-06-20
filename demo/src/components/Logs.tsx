import { Database, GitMerge } from 'lucide-react'
import { SectionHeader } from './SectionHeader'
import type { MergeResult, OperationView, SimulationResult } from '../types'

function shortId(id?: string) {
  return id ? id.slice(0, 8) : 'unknown'
}

function OperationLog({ operations }: { operations: OperationView[] }) {
  return (
    <div className="operation-log">
      {operations.map((operation) => (
        <div className="operation-row" key={operation.id}>
          <span className="branch-chip">{operation.branchName}</span>
          <span className="op-type">{operation.opType}</span>
          <span className="op-target">
            {operation.symbolName ?? shortId(operation.symbolId ?? operation.id)}
          </span>
        </div>
      ))}
    </div>
  )
}

function SimulationRow({ result }: { result: SimulationResult }) {
  return (
    <div className="simulation-row">
      <strong>
        {result.safeMerges}/{result.attemptedMerges} safe merges
      </strong>
      <span>
        {result.finalOperations} ops, {result.finalSymbols} symbols, {result.finalReferences} refs
      </span>
      <code>
        replay {result.replayDeterministic ? 'ok' : 'failed'} / render{' '}
        {result.renderDeterministic ? 'ok' : 'failed'} / {result.validator}{' '}
        {result.toolchainValidated ? 'ok' : 'failed'}
      </code>
    </div>
  )
}

function MergeReview({
  simulationResult,
  mergeResults,
}: {
  simulationResult: SimulationResult | null
  mergeResults: MergeResult[]
}) {
  return (
    <div className="merge-log">
      {simulationResult && <SimulationRow result={simulationResult} />}
      {mergeResults.length === 0 && !simulationResult && (
        <p className="muted">Run a merge wave to watch safe merges and conflicts.</p>
      )}
      {mergeResults.map((result) => (
        <div
          className="merge-row"
          key={`${result.sourceBranch.id}-${result.targetPosition}-${result.outcome}`}
        >
          <span className={`status-dot ${result.outcome === 'SAFE_MERGE' ? 'merged' : 'conflict'}`} />
          <div>
            <strong>{result.sourceBranch.name}</strong>
            <p>
              {result.outcome === 'SAFE_MERGE'
                ? `${result.appendedOperations.length} operations appended to main`
                : result.conflicts[0]?.detail ?? 'semantic conflict'}
            </p>
          </div>
        </div>
      ))}
    </div>
  )
}

export function Logs({
  operations,
  mergeResults,
  simulationResult,
}: {
  operations: OperationView[]
  mergeResults: MergeResult[]
  simulationResult: SimulationResult | null
}) {
  return (
    <section className="logs">
      <div className="section-block">
        <SectionHeader icon={<Database />} title="Operation Log" detail="Append-only" />
        <OperationLog operations={operations} />
      </div>

      <div className="section-block">
        <SectionHeader icon={<GitMerge />} title="Merge Review" detail="Operations, not lines" />
        <MergeReview simulationResult={simulationResult} mergeResults={mergeResults} />
      </div>
    </section>
  )
}
