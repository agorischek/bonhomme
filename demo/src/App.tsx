import { AlertTriangle } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { api, wait } from './api'
import { AgentEditors } from './components/AgentEditors'
import { Browser } from './components/browser/Browser'
import { Logs } from './components/Logs'
import { MetricsStrip } from './components/MetricsStrip'
import { Toolbar } from './components/Toolbar'
import { Workbench } from './components/Workbench'
import type {
  BranchSummary,
  DemoState,
  MergeResult,
  SimulationResult,
  SymbolNode,
} from './types'
import './App.css'

function readyBranches(agents: BranchSummary[], conflictedBranches: Set<string>) {
  return agents
    .filter((branch) => branch.status === 'ready' && !conflictedBranches.has(branch.name))
    .sort((a, b) => a.name.localeCompare(b.name))
}

function graphRoots(state: DemoState | null): SymbolNode[] {
  return Object.values(state?.mainGraph.symbols ?? {})
    .filter((symbol) => !symbol.parentId)
    .sort((a, b) => a.ordinal - b.ordinal || a.name.localeCompare(b.name))
}

function App() {
  const [view, setView] = useState<'workbench' | 'browse'>('workbench')
  const [state, setState] = useState<DemoState | null>(null)
  const [agentCount, setAgentCount] = useState(36)
  const [includeConflicts, setIncludeConflicts] = useState(false)
  const [mergeResults, setMergeResults] = useState<MergeResult[]>([])
  const [simulationResult, setSimulationResult] = useState<SimulationResult | null>(null)
  const [conflictedBranches, setConflictedBranches] = useState<Set<string>>(new Set())
  const [isRunning, setIsRunning] = useState(false)
  const [isBusy, setIsBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const runningRef = useRef(false)

  const refresh = useCallback(async () => {
    const next = await api<DemoState>('/api/demo/state')
    setState(next)
    return next
  }, [])

  useEffect(() => {
    let cancelled = false
    api<DemoState>('/api/demo/state')
      .then((next) => {
        if (!cancelled) setState(next)
      })
      .catch((err: Error) => {
        if (!cancelled) setError(err.message)
      })

    return () => {
      cancelled = true
    }
  }, [])

  const agents = useMemo(
    () => state?.branches.filter((branch) => branch.status !== 'main') ?? [],
    [state],
  )
  const readyAgents = useMemo(
    () => readyBranches(agents, conflictedBranches),
    [agents, conflictedBranches],
  )
  const roots = useMemo(() => graphRoots(state), [state])
  const selectedFile = state?.renderedFiles[0]
  const recentOperations = [...(state?.operations ?? [])].reverse().slice(0, 28)
  const revisionLabel = `${state?.mainBranch.name ?? 'main'}@${
    state?.mainGraph.appliedOperations.length ?? 0
  }`

  async function runAction(action: () => Promise<void>) {
    setIsBusy(true)
    setError(null)
    try {
      await action()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setIsBusy(false)
    }
  }

  async function resetDemo() {
    await runAction(async () => {
      const next = await api<DemoState>('/api/demo/reset', { method: 'POST' })
      setState(next)
      setMergeResults([])
      setSimulationResult(null)
      setConflictedBranches(new Set())
    })
  }

  async function spawnAgents() {
    await runAction(async () => {
      const next = await api<DemoState>('/api/demo/spawn', {
        method: 'POST',
        body: JSON.stringify({ count: agentCount, includeConflicts }),
      })
      setState(next)
    })
  }

  async function mergeBranch(branch: BranchSummary) {
    const result = await api<MergeResult>(`/api/demo/merge/${branch.name}`, { method: 'POST' })
    setMergeResults((current) => [result, ...current].slice(0, 24))
    if (result.outcome === 'CONFLICT') {
      setConflictedBranches((current) => new Set(current).add(result.sourceBranch.name))
    }
    await refresh()
  }

  async function runMergeWave() {
    setIsRunning(true)
    setError(null)
    runningRef.current = true
    try {
      await mergeUntilPaused(state ?? (await refresh()))
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      runningRef.current = false
      setIsRunning(false)
    }
  }

  async function mergeUntilPaused(initialState: DemoState) {
    let current = initialState
    let localConflicts = new Set(conflictedBranches)
    while (runningRef.current) {
      const next = readyBranches(
        current.branches.filter((branch) => branch.status !== 'main'),
        localConflicts,
      )[0]
      if (!next) break

      const result = await api<MergeResult>(`/api/demo/merge/${next.name}`, { method: 'POST' })
      setMergeResults((existing) => [result, ...existing].slice(0, 24))
      if (result.outcome === 'CONFLICT') {
        localConflicts = new Set(localConflicts).add(result.sourceBranch.name)
        setConflictedBranches(new Set(localConflicts))
      }
      current = await refresh()
      await wait(180)
    }
  }

  function stopRun() {
    runningRef.current = false
    setIsRunning(false)
  }

  async function runSimulation() {
    await runAction(async () => {
      setMergeResults([])
      setConflictedBranches(new Set())
      const result = await api<SimulationResult>('/api/demo/simulate', {
        method: 'POST',
        body: JSON.stringify({ agentCount, includeConflicts }),
      })
      setSimulationResult(result)
      await refresh()
    })
  }

  return (
    <div className="app-shell">
      <nav
        className="view-tabs"
        style={{ display: 'flex', gap: 4, padding: '6px 16px', borderBottom: '1px solid #d1d9e0' }}
      >
        {(['workbench', 'browse'] as const).map((mode) => (
          <button
            key={mode}
            type="button"
            onClick={() => setView(mode)}
            aria-pressed={view === mode}
            style={{
              padding: '6px 14px',
              border: 'none',
              background: 'none',
              cursor: 'pointer',
              fontSize: 14,
              fontWeight: view === mode ? 600 : 400,
              color: view === mode ? '#0969da' : '#59636e',
              borderBottom: view === mode ? '2px solid #0969da' : '2px solid transparent',
            }}
          >
            {mode === 'workbench' ? 'Workbench' : 'Browse'}
          </button>
        ))}
      </nav>
      {view === 'browse' ? (
        <Browser />
      ) : (
        <>
      <Toolbar
        agentCount={agentCount}
        includeConflicts={includeConflicts}
        isBusy={isBusy}
        isRunning={isRunning}
        canRun={readyAgents.length > 0}
        onAgentCountChange={setAgentCount}
        onIncludeConflictsChange={setIncludeConflicts}
        onReset={() => void resetDemo()}
        onSpawn={() => void spawnAgents()}
        onStress={() => void runSimulation()}
        onRun={() => void runMergeWave()}
        onPause={stopRun}
      />

      {error && (
        <div className="error" role="alert">
          <AlertTriangle aria-hidden="true" />
          {error}
        </div>
      )}

      <main>
        <MetricsStrip metrics={state?.metrics} />
        <AgentEditors
          agents={agents}
          readyCount={readyAgents.length}
          conflictCount={conflictedBranches.size}
          conflictedBranches={conflictedBranches}
          onMerge={(branch) => void runAction(() => mergeBranch(branch))}
          disabled={isBusy || isRunning}
        />
        <Workbench
          graph={state?.mainGraph}
          graphRoots={roots}
          selectedFile={selectedFile}
          revisionLabel={revisionLabel}
        />
        <Logs
          operations={recentOperations}
          mergeResults={mergeResults}
          simulationResult={simulationResult}
        />
      </main>
        </>
      )}
    </div>
  )
}

export default App
