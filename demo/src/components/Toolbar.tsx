import {
  Activity,
  GitBranch,
  Pause,
  Play,
  RotateCcw,
  Users,
} from 'lucide-react'

export function Toolbar({
  agentCount,
  includeConflicts,
  isBusy,
  isRunning,
  canRun,
  onAgentCountChange,
  onIncludeConflictsChange,
  onReset,
  onSpawn,
  onStress,
  onRun,
  onPause,
}: {
  agentCount: number
  includeConflicts: boolean
  isBusy: boolean
  isRunning: boolean
  canRun: boolean
  onAgentCountChange: (value: number) => void
  onIncludeConflictsChange: (value: boolean) => void
  onReset: () => void
  onSpawn: () => void
  onStress: () => void
  onRun: () => void
  onPause: () => void
}) {
  return (
    <header className="topbar">
      <div className="brand">
        <GitBranch aria-hidden="true" />
        <div>
          <h1>bonhomme</h1>
          <p>Semantic operations source control for TypeScript agents</p>
        </div>
      </div>

      <div className="toolbar">
        <label className="field" title="Number of agent branches to create">
          <span>Agents</span>
          <input
            value={agentCount}
            min={1}
            max={120}
            type="number"
            onChange={(event) => onAgentCountChange(Number(event.target.value))}
          />
        </label>
        <label className="toggle" title="Create duplicate semantic method names for conflict review">
          <input
            checked={includeConflicts}
            type="checkbox"
            onChange={(event) => onIncludeConflictsChange(event.target.checked)}
          />
          <span>Conflict twins</span>
        </label>
        <button type="button" onClick={onReset} disabled={isBusy || isRunning} title="Reset demo repository">
          <RotateCcw aria-hidden="true" />
          Reset
        </button>
        <button type="button" onClick={onSpawn} disabled={isBusy || isRunning} title="Create agent branches">
          <Users aria-hidden="true" />
          Spawn
        </button>
        <button
          type="button"
          onClick={onStress}
          disabled={isBusy || isRunning}
          title="Run deterministic backend simulation"
        >
          <Activity aria-hidden="true" />
          Stress
        </button>
        {isRunning ? (
          <button type="button" onClick={onPause} className="danger" title="Pause live merge wave">
            <Pause aria-hidden="true" />
            Pause
          </button>
        ) : (
          <button
            type="button"
            onClick={onRun}
            disabled={isBusy || !canRun}
            className="primary"
            title="Merge ready agent branches one by one"
          >
            <Play aria-hidden="true" />
            Run
          </button>
        )}
      </div>
    </header>
  )
}
