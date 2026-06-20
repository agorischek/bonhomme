export type BranchStatus = 'main' | 'empty' | 'ready' | 'merged'
export type MergeOutcome = 'SAFE_MERGE' | 'CONFLICT'

export interface BranchSummary {
  id: string
  name: string
  basePosition: number
  status: BranchStatus
  ownOperationCount: number
  createdSymbolCount: number
  createdMethodNames: string[]
  createdBy: string
}

export interface OperationView {
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

export interface SymbolNode {
  id: string
  parentId?: string | null
  kind: string
  name: string
  ordinal: number
  body?: string | null
  metadata?: Record<string, unknown>
}

export interface Task {
  id: string
  repositoryId: string
  title: string
  createdAt: string
}

export interface ChangeSet {
  id: string
  repositoryId: string
  taskId: string
  branchId: string
  title: string
  createdBy: string
  createdAt: string
}

export interface ReferenceNode {
  id: string
  fromSymbolId: string
  toSymbolId: string
  kind: string
}

export interface SemanticGraph {
  symbols: Record<string, SymbolNode>
  references: Record<string, ReferenceNode>
  appliedOperations: string[]
}

export interface RenderedFile {
  path: string
  content: string
}

export interface DemoMetrics {
  branchCount: number
  agentCount: number
  mergedAgentCount: number
  pendingAgentCount: number
  operationCount: number
  symbolCount: number
  referenceCount: number
}

export interface DemoState {
  repository: { id: string; name: string }
  mainBranch: { id: string; name: string }
  branches: BranchSummary[]
  tasks: Task[]
  changesets: ChangeSet[]
  operations: OperationView[]
  mainGraph: SemanticGraph
  renderedFiles: RenderedFile[]
  metrics: DemoMetrics
}

export interface MergeConflict {
  reason: string
  sourceOperationId: string
  targetOperationId?: string | null
  symbolId?: string | null
  detail: string
}

export interface MergeResult {
  outcome: MergeOutcome
  conflicts: MergeConflict[]
  sourceBranch: { id: string; name: string }
  targetBranch: { id: string; name: string }
  appendedOperations: OperationView[]
  targetPosition: number
}

export interface SimulationResult {
  repository: string
  language: string
  validator: string
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
  toolchainValidated: boolean
  tscValidated: boolean
}
