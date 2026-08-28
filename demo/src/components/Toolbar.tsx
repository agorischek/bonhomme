import { Activity, GitBranch, Pause, Play, Rocket, RotateCcw, Users } from 'lucide-react'

export function Toolbar({
  agentCount,
  includeConflicts,
  isBusy,
  hasEditingAgents,
  isRunning,
  canRun,
  onAgentCountChange,
  onIncludeConflictsChange,
  onReset,
  onSpawn,
  onStress,
  onRun,
  onSwarm,
  onPause,
}: {
  agentCount: number
  includeConflicts: boolean
  isBusy: boolean
  hasEditingAgents: boolean
  isRunning: boolean
  canRun: boolean
  onAgentCountChange: (value: number) => void
  onIncludeConflictsChange: (value: boolean) => void
  onReset: () => void
  onSpawn: () => void
  onStress: () => void
  onRun: () => void
  onSwarm: () => void
  onPause: () => void
}) {
  const cannotStartNewWork = isBusy || hasEditingAgents || isRunning

  return (
    <header className="topbar">
      <div className="brand">
        <GitBranch aria-hidden="true" />
        <div>
          <h1>bonhomme</h1>
          <p>Semantic merge queue for Codex agent swarms</p>
        </div>
      </div>

      <div className="toolbar">
        <div className="toolbar-group">
          <span className="toolbar-label">Scenario</span>
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
            <span>Conflicts</span>
          </label>
        </div>

        <div className="toolbar-group">
          <span className="toolbar-label">Create</span>
          <button
            type="button"
            onClick={onSpawn}
            disabled={cannotStartNewWork}
            title="Create deterministic branches awaiting merge"
          >
            <Users aria-hidden="true" />
            Simulate
          </button>
          <button
            type="button"
            onClick={onSwarm}
            disabled={cannotStartNewWork}
            className="launch"
            title="Reset, launch real Codex workers, and let each one merge when finished"
          >
            <Rocket aria-hidden="true" />
            Real Codex
          </button>
        </div>

        <div className="toolbar-group">
          <span className="toolbar-label">Check</span>
          <button
            type="button"
            onClick={onStress}
            disabled={cannotStartNewWork}
            title="Run deterministic backend simulation"
          >
            <Activity aria-hidden="true" />
            Stress test
          </button>
        </div>

        <div className="toolbar-group">
          <span className="toolbar-label">Queue</span>
          {isRunning ? (
            <button type="button" onClick={onPause} className="danger" title="Pause live merge wave">
              <Pause aria-hidden="true" />
              Pause
            </button>
          ) : (
            <button
              type="button"
              onClick={onRun}
              disabled={isBusy || hasEditingAgents || !canRun}
              className="primary"
              title="Manually merge queued agent branches one by one"
            >
              <Play aria-hidden="true" />
              Merge queue
            </button>
          )}
        </div>

        <div className="toolbar-group toolbar-group-reset">
          <button type="button" onClick={onReset} disabled={cannotStartNewWork} title="Reset demo repository">
            <RotateCcw aria-hidden="true" />
            Reset
          </button>
        </div>
      </div>
    </header>
  )
}
