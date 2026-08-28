import { AlertTriangle } from 'lucide-react'
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { api, wait } from './api'
import { AgentEditors } from './components/AgentEditors'
import { Toolbar } from './components/Toolbar'
import { Workbench } from './components/Workbench'
import type {
  BranchSummary,
  DemoState,
  MergeResult,
  RealSwarmResponse,
  RealSwarmStatus,
  SimulationResult,
  SymbolNode,
} from './types'
import './App.css'

const Browser = lazy(() =>
  import('./components/browser/Browser').then((module) => ({ default: module.Browser })),
)
const BonhommePresentation = lazy(() =>
  import('./presentation/BonhommePresentation').then((module) => ({
    default: module.BonhommePresentation,
  })),
)

function readyBranches(agents: BranchSummary[]) {
  return agents
    .filter((branch) => branch.status === 'ready')
    .sort((a, b) => a.name.localeCompare(b.name))
}

function graphRoots(state: DemoState | null): SymbolNode[] {
  return Object.values(state?.mainGraph.symbols ?? {})
    .filter((symbol) => !symbol.parentId)
    .sort((a, b) => a.ordinal - b.ordinal || a.name.localeCompare(b.name))
}

function App() {
  const [view, setView] = useState<'presentation' | 'workbench' | 'browse'>('presentation')
  const [state, setState] = useState<DemoState | null>(null)
  const [agentCount, setAgentCount] = useState(30)
  const [includeConflicts, setIncludeConflicts] = useState(true)
  const [mergeResults, setMergeResults] = useState<MergeResult[]>([])
  const [simulationResult, setSimulationResult] = useState<SimulationResult | null>(null)
  const [isRunning, setIsRunning] = useState(false)
  const [isBusy, setIsBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const runningRef = useRef(false)
  const realSwarmAbortRef = useRef<AbortController | null>(null)
  const previousMainBranchIdRef = useRef<string | null>(null)

  const refresh = useCallback(async () => {
    const next = await api<DemoState>('/api/demo/state')
    setState(next)
    return next
  }, [])

  useEffect(() => {
    let cancelled = false
    let loaded = false
    let timer: number | undefined
    let controller: AbortController | undefined

    async function poll() {
      controller = new AbortController()
      try {
        const next = await api<DemoState>('/api/demo/state', { signal: controller.signal })
        if (!cancelled) {
          loaded = true
          setState(next)
        }
      } catch (err) {
        if (
          !cancelled &&
          !loaded &&
          !(err instanceof DOMException && err.name === 'AbortError')
        ) {
          setError(err instanceof Error ? err.message : String(err))
        }
      } finally {
        if (!cancelled) timer = window.setTimeout(() => void poll(), 1200)
      }
    }

    void poll()
    return () => {
      cancelled = true
      controller?.abort()
      if (timer !== undefined) window.clearTimeout(timer)
    }
  }, [])

  useEffect(() => {
    if (!state?.mainBranch.id) return
    const previous = previousMainBranchIdRef.current
    previousMainBranchIdRef.current = state.mainBranch.id
    if (previous && previous !== state.mainBranch.id) {
      runningRef.current = false
      setIsRunning(false)
      setMergeResults([])
      setSimulationResult(null)
    }
  }, [state?.mainBranch.id])

  useEffect(
    () => () => {
      realSwarmAbortRef.current?.abort()
    },
    [view],
  )

  const agents = useMemo(
    () => state?.branches.filter((branch) => branch.status !== 'main') ?? [],
    [state],
  )
  const readyAgents = useMemo(() => readyBranches(agents), [agents])
  const conflictCount = agents.filter((branch) => branch.status === 'conflicted').length
  const hasEditingAgents = agents.some((branch) => branch.status === 'empty')
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
    while (runningRef.current) {
      const next = readyBranches(current.branches.filter((branch) => branch.status !== 'main'))[0]
      if (!next) break

      const result = await api<MergeResult>(`/api/demo/merge/${next.name}`, { method: 'POST' })
      setMergeResults((existing) => [result, ...existing].slice(0, 24))
      current = await refresh()
      await wait(180)
    }
  }

  function stopRun() {
    runningRef.current = false
    setIsRunning(false)
  }

  async function runRealSwarmDemo() {
    const requestedAgents = Number.isFinite(agentCount)
      ? Math.max(1, Math.min(120, Math.round(agentCount)))
      : 30
    setIsBusy(true)
    setError(null)
    setAgentCount(requestedAgents)
    setMergeResults([])
    setSimulationResult(null)
    runningRef.current = false
    realSwarmAbortRef.current?.abort()
    const controller = new AbortController()
    realSwarmAbortRef.current = controller
    const pollDeadline = Date.now() + 30 * 60_000

    try {
      const response = await api<RealSwarmResponse>('/api/demo/real-swarm', {
        method: 'POST',
        signal: controller.signal,
        body: JSON.stringify({
          agents: requestedAgents,
          concurrency: requestedAgents,
          includeConflicts,
          model: 'gpt-5.3-codex-spark',
          merge: true,
        }),
      })
      setState(response.state)
      while (!controller.signal.aborted) {
        await wait(800)
        if (Date.now() >= pollDeadline) {
          throw new Error('Timed out waiting for the real swarm after 30 minutes')
        }
        const status = await api<RealSwarmStatus>('/api/demo/real-swarm/status', {
          signal: controller.signal,
        })
        if (!status.running) {
          await refresh()
          if (status.error) throw new Error(status.error)
          break
        }
      }
    } catch (err) {
      if (!controller.signal.aborted) {
        setError(err instanceof Error ? err.message : String(err))
      }
    } finally {
      if (realSwarmAbortRef.current === controller) {
        realSwarmAbortRef.current = null
        setIsBusy(false)
      }
    }
  }

  async function runSimulation() {
    await runAction(async () => {
      setMergeResults([])
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
        {(['presentation', 'workbench', 'browse'] as const).map((mode) => (
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
            {mode === 'presentation' ? 'Presentation' : mode === 'workbench' ? 'Workbench' : 'Browse'}
          </button>
        ))}
      </nav>
      {view === 'presentation' ? (
        <Suspense fallback={<div className="view-loading">Loading presentation…</div>}>
          <BonhommePresentation />
        </Suspense>
      ) : view === 'browse' ? (
        <Suspense fallback={<div className="view-loading">Loading browser…</div>}>
          <Browser />
        </Suspense>
      ) : (
        <>
      <Toolbar
        agentCount={agentCount}
        includeConflicts={includeConflicts}
        isBusy={isBusy}
        hasEditingAgents={hasEditingAgents}
        isRunning={isRunning}
        canRun={readyAgents.length > 0}
        onAgentCountChange={setAgentCount}
        onIncludeConflictsChange={setIncludeConflicts}
        onReset={() => void resetDemo()}
        onSpawn={() => void spawnAgents()}
        onStress={() => void runSimulation()}
        onRun={() => void runMergeWave()}
        onSwarm={() => void runRealSwarmDemo()}
        onPause={stopRun}
      />

      {error && (
        <div className="error" role="alert">
          <AlertTriangle aria-hidden="true" />
          {error}
        </div>
      )}

      <main className="workbench-main">
        <div className="agents-column">
          <AgentEditors
            agents={agents}
            readyCount={readyAgents.length}
            conflictCount={conflictCount}
            onMerge={(branch) => void runAction(() => mergeBranch(branch))}
            disabled={isBusy || isRunning}
          />
        </div>
        <Workbench
          graph={state?.mainGraph}
          graphRoots={roots}
          selectedFile={selectedFile}
          revisionLabel={revisionLabel}
          metrics={state?.metrics}
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
