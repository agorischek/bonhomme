import {
  Activity,
  AlertTriangle,
  CheckCircle2,
  Code2,
  Database,
  GitBranch,
  GitMerge,
  Network,
  Pause,
  Play,
  RotateCcw,
  Split,
  Users,
} from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import './App.css'

const API_BASE = import.meta.env.VITE_BONHOMME_API ?? 'http://127.0.0.1:3030'

type BranchStatus = 'main' | 'empty' | 'ready' | 'merged'
type MergeOutcome = 'SAFE_MERGE' | 'CONFLICT'

interface BranchSummary {
  id: string
  name: string
  basePosition: number
  status: BranchStatus
  ownOperationCount: number
  createdSymbolCount: number
  createdMethodNames: string[]
  createdBy: string
}

interface OperationView {
  id: string
  branchId: string
  branchName: string
  changesetId: string
  position: number
  opType: string
  symbolId?: string
  symbolName?: string
  symbolKind?: string
}

interface SymbolNode {
  id: string
  parentId?: string | null
  kind: string
  name: string
  ordinal: number
}

interface ReferenceNode {
  id: string
  fromSymbolId: string
  toSymbolId: string
  kind: string
}

interface SemanticGraph {
  symbols: Record<string, SymbolNode>
  references: Record<string, ReferenceNode>
  appliedOperations: string[]
}

interface RenderedFile {
  path: string
  content: string
}

interface DemoMetrics {
  branchCount: number
  agentCount: number
  mergedAgentCount: number
  pendingAgentCount: number
  operationCount: number
  symbolCount: number
  referenceCount: number
}

interface DemoState {
  repository: { id: string; name: string }
  mainBranch: { id: string; name: string }
  branches: BranchSummary[]
  operations: OperationView[]
  mainGraph: SemanticGraph
  renderedFiles: RenderedFile[]
  metrics: DemoMetrics
}

interface MergeConflict {
  reason: string
  sourceOperationId: string
  targetOperationId?: string | null
  symbolId?: string | null
  detail: string
}

interface MergeResult {
  outcome: MergeOutcome
  conflicts: MergeConflict[]
  sourceBranch: { id: string; name: string }
  targetBranch: { id: string; name: string }
  appendedOperations: OperationView[]
  targetPosition: number
}

interface SimulationResult {
  repository: string
  agentCount: number
  attemptedMerges: number
  safeMerges: number
  conflicts: number
  appendedOperations: number
  finalOperations: number
  finalSymbols: number
  finalReferences: number
  replayDeterministic: boolean
  renderDeterministic: boolean
  tscValidated: boolean
}

async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${API_BASE}${path}`, {
    headers: { 'content-type': 'application/json', ...(init?.headers ?? {}) },
    ...init,
  })
  if (!response.ok) {
    const body = await response.json().catch(() => ({ error: response.statusText }))
    throw new Error(body.error ?? response.statusText)
  }
  return response.json()
}

const wait = (ms: number) => new Promise((resolve) => window.setTimeout(resolve, ms))

function App() {
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
    () =>
      agents
        .filter((branch) => branch.status === 'ready' && !conflictedBranches.has(branch.name))
        .sort((a, b) => a.name.localeCompare(b.name)),
    [agents, conflictedBranches],
  )
  const selectedFile = state?.renderedFiles[0]
  const recentOperations = [...(state?.operations ?? [])].reverse().slice(0, 28)
  const graphRoots = useMemo(() => {
    const symbols = Object.values(state?.mainGraph.symbols ?? {})
    return symbols
      .filter((symbol) => !symbol.parentId)
      .sort((a, b) => a.ordinal - b.ordinal || a.name.localeCompare(b.name))
  }, [state])

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
      let current = state ?? (await refresh())
      let localConflicts = new Set(conflictedBranches)
      while (runningRef.current) {
        const next = current.branches
          .filter((branch) => branch.status === 'ready' && !localConflicts.has(branch.name))
          .sort((a, b) => a.name.localeCompare(b.name))[0]

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
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      runningRef.current = false
      setIsRunning(false)
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
              onChange={(event) => setAgentCount(Number(event.target.value))}
            />
          </label>
          <label className="toggle" title="Create duplicate semantic method names for conflict review">
            <input
              checked={includeConflicts}
              type="checkbox"
              onChange={(event) => setIncludeConflicts(event.target.checked)}
            />
            <span>Conflict twins</span>
          </label>
          <button type="button" onClick={resetDemo} disabled={isBusy || isRunning} title="Reset demo repository">
            <RotateCcw aria-hidden="true" />
            Reset
          </button>
          <button type="button" onClick={spawnAgents} disabled={isBusy || isRunning} title="Create agent branches">
            <Users aria-hidden="true" />
            Spawn
          </button>
          <button type="button" onClick={runSimulation} disabled={isBusy || isRunning} title="Run deterministic backend simulation">
            <Activity aria-hidden="true" />
            Stress
          </button>
          {isRunning ? (
            <button type="button" onClick={stopRun} className="danger" title="Pause live merge wave">
              <Pause aria-hidden="true" />
              Pause
            </button>
          ) : (
            <button
              type="button"
              onClick={() => void runMergeWave()}
              disabled={isBusy || readyAgents.length === 0}
              className="primary"
              title="Merge ready agent branches one by one"
            >
              <Play aria-hidden="true" />
              Run
            </button>
          )}
        </div>
      </header>

      {error && (
        <div className="error" role="alert">
          <AlertTriangle aria-hidden="true" />
          {error}
        </div>
      )}

      <main>
        <section className="metrics" aria-label="Repository metrics">
          <Metric icon={<Database />} label="Postgres ops" value={state?.metrics.operationCount ?? 0} />
          <Metric icon={<Users />} label="Agent branches" value={state?.metrics.agentCount ?? 0} />
          <Metric icon={<CheckCircle2 />} label="Merged agents" value={state?.metrics.mergedAgentCount ?? 0} />
          <Metric icon={<Network />} label="References" value={state?.metrics.referenceCount ?? 0} />
          <Metric icon={<Split />} label="Graph symbols" value={state?.metrics.symbolCount ?? 0} />
        </section>

        <section className="section-block">
          <SectionHeader
            icon={<Activity />}
            title="Agent Editors"
            detail={`${readyAgents.length} ready, ${conflictedBranches.size} conflict-held`}
          />
          <div className="agent-grid">
            {agents.map((branch) => (
              <AgentCard
                key={branch.id}
                branch={branch}
                conflicted={conflictedBranches.has(branch.name)}
                onMerge={() => void runAction(() => mergeBranch(branch))}
                disabled={isBusy || isRunning}
              />
            ))}
          </div>
        </section>

        <section className="workbench">
          <div className="section-block graph-block">
            <SectionHeader icon={<Network />} title="Semantic Graph" detail="Materialized from replay" />
            <div className="graph-tree">
              {graphRoots.map((symbol) => (
                <GraphNode key={symbol.id} symbol={symbol} graph={state?.mainGraph} depth={0} />
              ))}
            </div>
          </div>

          <div className="section-block code-block">
            <SectionHeader
              icon={<Code2 />}
              title={selectedFile?.path ?? 'Rendered TypeScript'}
              detail={`${state?.mainBranch.name ?? 'main'}@${state?.mainGraph.appliedOperations.length ?? 0}`}
            />
            <pre className="code-pane">{selectedFile?.content ?? 'Waiting for bonhomme API...'}</pre>
          </div>
        </section>

        <section className="logs">
          <div className="section-block">
            <SectionHeader icon={<Database />} title="Operation Log" detail="Append-only" />
            <div className="operation-log">
              {recentOperations.map((operation) => (
                <div className="operation-row" key={operation.id}>
                  <span className="branch-chip">{operation.branchName}</span>
                  <span className="op-type">{operation.opType}</span>
                  <span className="op-target">{operation.symbolName ?? shortId(operation.symbolId ?? operation.id)}</span>
                </div>
              ))}
            </div>
          </div>

          <div className="section-block">
            <SectionHeader icon={<GitMerge />} title="Merge Review" detail="Operations, not lines" />
            <div className="merge-log">
              {simulationResult && (
                <div className="simulation-row">
                  <strong>{simulationResult.safeMerges}/{simulationResult.attemptedMerges} safe merges</strong>
                  <span>
                    {simulationResult.finalOperations} ops, {simulationResult.finalSymbols} symbols,{' '}
                    {simulationResult.finalReferences} refs
                  </span>
                  <code>
                    replay {simulationResult.replayDeterministic ? 'ok' : 'failed'} / render{' '}
                    {simulationResult.renderDeterministic ? 'ok' : 'failed'} / tsc{' '}
                    {simulationResult.tscValidated ? 'ok' : 'failed'}
                  </code>
                </div>
              )}
              {mergeResults.length === 0 && !simulationResult && (
                <p className="muted">Run a merge wave to watch safe merges and conflicts.</p>
              )}
              {mergeResults.map((result) => (
                <div className="merge-row" key={`${result.sourceBranch.id}-${result.targetPosition}-${result.outcome}`}>
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
          </div>
        </section>
      </main>
    </div>
  )
}

function Metric({ icon, label, value }: { icon: React.ReactNode; label: string; value: number }) {
  return (
    <div className="metric">
      {icon}
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  )
}

function SectionHeader({ icon, title, detail }: { icon: React.ReactNode; title: string; detail: string }) {
  return (
    <div className="section-head">
      <div>
        {icon}
        <h2>{title}</h2>
      </div>
      <span>{detail}</span>
    </div>
  )
}

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
      <button type="button" disabled={disabled || branch.status !== 'ready' || conflicted} onClick={onMerge} title="Merge this branch">
        <GitMerge aria-hidden="true" />
        Merge
      </button>
    </article>
  )
}

function GraphNode({
  symbol,
  graph,
  depth,
}: {
  symbol: SymbolNode
  graph?: SemanticGraph
  depth: number
}) {
  const children = Object.values(graph?.symbols ?? {})
    .filter((child) => child.parentId === symbol.id)
    .sort((a, b) => a.ordinal - b.ordinal || a.name.localeCompare(b.name))

  return (
    <div>
      <div className="graph-node" style={{ paddingLeft: `${depth * 18}px` }}>
        <span>{symbol.kind}</span>
        <strong>{symbol.name}</strong>
        <code>{shortId(symbol.id)}</code>
      </div>
      {children.map((child) => (
        <GraphNode key={child.id} symbol={child} graph={graph} depth={depth + 1} />
      ))}
    </div>
  )
}

function shortId(id: string) {
  return id ? id.slice(0, 8) : 'unknown'
}

export default App
