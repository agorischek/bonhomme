import {
  AlertTriangle,
  ArrowLeft,
  ArrowRight,
  Boxes,
  Braces,
  Code2,
  Database,
  Diff,
  FileCode2,
  GitCompareArrows,
  GitMerge,
  Layers3,
  Puzzle,
  ScanSearch,
  Terminal,
  Waypoints,
  Workflow,
} from 'lucide-react'
import { AnimatePresence, motion } from 'framer-motion'
import type { ReactNode } from 'react'
import { useCallback, useEffect, useState } from 'react'
import './BonhommePresentation.css'

type StepId = 'source' | 'symbols' | 'operations' | 'recovery' | 'conflict' | 'boundary'

type WalkthroughStep = {
  id: StepId
  eyebrow: string
  title: string
  summary: string
  icon: ReactNode
}

type CodeLine = {
  number: number
  text: string
  tone?: 'class' | 'method' | 'reference' | 'added' | 'changed' | 'removed'
}

type OperationRow = {
  pos: number
  branch: string
  op: string
  target: string
  detail: string
  status: 'replayed' | 'ready' | 'conflict'
}

const steps: WalkthroughStep[] = [
  {
    id: 'source',
    eyebrow: '1 / Source projection',
    title: 'Start with ordinary code, then stop treating it as the source of truth.',
    summary:
      'The TypeScript file is a rendered projection. Bonhomme keeps it readable for humans and ' +
      'tools, then maps its structure back to stable symbols.',
    icon: <FileCode2 aria-hidden="true" />,
  },
  {
    id: 'symbols',
    eyebrow: '2 / Symbolization',
    title: 'The language plugin turns syntax into symbols and references.',
    summary:
      'A class, method, property, file, and call edge become core primitives with stable IDs. ' +
      'Names can change without destroying identity.',
    icon: <Waypoints aria-hidden="true" />,
  },
  {
    id: 'operations',
    eyebrow: '3 / Primitive operation log',
    title: 'The durable artifact is an append-only stream of primitive mutations.',
    summary:
      'Core does not store a TypeScript AST. It stores small operations: create a symbol, update a ' +
      'symbol, create a reference, delete a reference.',
    icon: <Database aria-hidden="true" />,
  },
  {
    id: 'recovery',
    eyebrow: '4 / Slice recovery',
    title: 'An edited slice is diffed back into primitive operations.',
    summary:
      'Agents edit clean TypeScript. The plugin compares the edited slice to graph-backed ' +
      'provenance and emits the smallest semantic operation set it can defend.',
    icon: <GitCompareArrows aria-hidden="true" />,
  },
  {
    id: 'conflict',
    eyebrow: '5 / Semantic conflict',
    title: 'A conflict is about graph identity, not nearby lines.',
    summary:
      'Two branches can touch the same file safely. They conflict when their operations violate ' +
      'graph invariants, such as duplicate sibling symbols.',
    icon: <AlertTriangle aria-hidden="true" />,
  },
  {
    id: 'boundary',
    eyebrow: '6 / Core versus plugins',
    title: 'The core engine stays small; language plugins carry syntax knowledge.',
    summary:
      'The merge engine only needs symbols, references, operations, replay, and validation. ' +
      'TypeScript, Go, Rust, Python, C#, and Elixir plug in at the edges.',
    icon: <Puzzle aria-hidden="true" />,
  },
]

const sourceLines: CodeLine[] = [
  { number: 1, text: 'export class OrderService {', tone: 'class' },
  { number: 2, text: '  private orders: Order[] = []' },
  { number: 3, text: '' },
  { number: 4, text: '  displayName(): string {', tone: 'method' },
  { number: 5, text: '    return "OrderService"' },
  { number: 6, text: '  }' },
  { number: 7, text: '' },
  { number: 8, text: '  listOrders(): Order[] {', tone: 'method' },
  { number: 9, text: '    return this.orders', tone: 'reference' },
  { number: 10, text: '  }' },
  { number: 11, text: '}' },
]

const editedLines: CodeLine[] = [
  { number: 1, text: 'export class OrderService {', tone: 'class' },
  { number: 2, text: '  private orders: Order[] = []' },
  { number: 3, text: '' },
  { number: 4, text: '  displayName(): string {', tone: 'method' },
  { number: 5, text: '    return "OrderService"' },
  { number: 6, text: '  }' },
  { number: 7, text: '' },
  { number: 8, text: '  listOrders(): Order[] {', tone: 'method' },
  { number: 9, text: '    return this.orders', tone: 'reference' },
  { number: 10, text: '  }' },
  { number: 11, text: '' },
  { number: 12, text: '  auditRisk(): RiskSignal {', tone: 'added' },
  { number: 13, text: '    return scoreOrders(this.listOrders())', tone: 'added' },
  { number: 14, text: '  }', tone: 'added' },
  { number: 15, text: '}' },
]

const conflictBaseLines: CodeLine[] = [
  { number: 8, text: '  listOrders(): Order[] {' },
  { number: 9, text: '    return this.orders' },
  { number: 10, text: '  }' },
  { number: 11, text: '' },
  { number: 12, text: '  auditRisk(): RiskSignal {', tone: 'added' },
  { number: 13, text: '    return scoreOrders(this.listOrders())', tone: 'added' },
  { number: 14, text: '  }', tone: 'added' },
]

const conflictOtherLines: CodeLine[] = [
  { number: 8, text: '  listOrders(): Order[] {' },
  { number: 9, text: '    return this.orders' },
  { number: 10, text: '  }' },
  { number: 11, text: '' },
  { number: 12, text: '  auditRisk(): RiskSignal {', tone: 'added' },
  { number: 13, text: '    return policy.score(this.orders)', tone: 'changed' },
  { number: 14, text: '  }', tone: 'added' },
]

const symbols = [
  {
    id: 'file:order-service',
    kind: 'file',
    name: 'src/OrderService.ts',
    parent: '-',
    pluginKind: 'module',
  },
  {
    id: 'sym:order-service',
    kind: 'class',
    name: 'OrderService',
    parent: 'file:order-service',
    pluginKind: 'ClassDeclaration',
  },
  {
    id: 'sym:orders',
    kind: 'property',
    name: 'orders',
    parent: 'sym:order-service',
    pluginKind: 'PropertyDefinition',
  },
  {
    id: 'sym:display-name',
    kind: 'method',
    name: 'displayName',
    parent: 'sym:order-service',
    pluginKind: 'MethodDefinition',
  },
  {
    id: 'sym:list-orders',
    kind: 'method',
    name: 'listOrders',
    parent: 'sym:order-service',
    pluginKind: 'MethodDefinition',
  },
]

const operations: OperationRow[] = [
  {
    pos: 181,
    branch: 'main',
    op: 'CreateSymbol',
    target: 'sym:order-service',
    detail: 'kind=class name=OrderService parent=file:order-service',
    status: 'replayed',
  },
  {
    pos: 182,
    branch: 'main',
    op: 'CreateSymbol',
    target: 'sym:list-orders',
    detail: 'kind=method name=listOrders parent=sym:order-service',
    status: 'replayed',
  },
  {
    pos: 183,
    branch: 'main',
    op: 'CreateReference',
    target: 'ref:list-orders->orders',
    detail: 'from=sym:list-orders to=sym:orders kind=reads',
    status: 'replayed',
  },
  {
    pos: 184,
    branch: 'agent-017',
    op: 'CreateSymbol',
    target: 'sym:audit-risk',
    detail: 'kind=method name=auditRisk parent=sym:order-service',
    status: 'ready',
  },
  {
    pos: 185,
    branch: 'agent-017',
    op: 'CreateReference',
    target: 'ref:audit-risk->list-orders',
    detail: 'from=sym:audit-risk to=sym:list-orders kind=calls',
    status: 'ready',
  },
]

const recoveredOperations = [
  {
    op: 'CreateSymbol',
    why: 'New method under known parent',
    payload: 'kind=method name=auditRisk parent=sym:order-service',
  },
  {
    op: 'UpdateSymbol',
    why: 'Body text belongs to the new method symbol',
    payload: 'symbol=sym:audit-risk bodyHash=4f90c0',
  },
  {
    op: 'CreateReference',
    why: 'Call expression resolves to an existing method',
    payload: 'from=sym:audit-risk to=sym:list-orders kind=calls',
  },
]

const recoveryLogs = [
  '[slice] base graph position: 183',
  '[parse] TypeScript plugin parsed edited projection',
  '[match] OrderService matched by stored slice root',
  '[match] listOrders matched by stable symbol provenance',
  '[recover] auditRisk has no base symbol -> CreateSymbol',
  '[recover] this.listOrders() -> CreateReference(calls)',
]

const conflictRows = [
  {
    branch: 'agent-017',
    operation: 'CreateSymbol(method, "auditRisk")',
    parent: 'sym:order-service',
    outcome: 'accepted candidate',
  },
  {
    branch: 'agent-022',
    operation: 'CreateSymbol(method, "auditRisk")',
    parent: 'sym:order-service',
    outcome: 'duplicate sibling',
  },
]

const boundaryColumns = [
  {
    title: 'Core engine',
    icon: <Boxes aria-hidden="true" />,
    items: [
      'Repositories, branches, tasks, changesets',
      'Operation log and append rules',
      'Graph replay and invariant validation',
      'Operation-level merge outcomes',
      'Storage and graph/render cache',
    ],
  },
  {
    title: 'Language plugins',
    icon: <Braces aria-hidden="true" />,
    items: [
      'Parse source syntax into symbols',
      'Recover edits from slices',
      'Render compatibility source files',
      'Scan references and imports',
      'Run toolchain validation gates',
    ],
  },
  {
    title: 'Fallback handlers',
    icon: <Layers3 aria-hidden="true" />,
    items: [
      'JSON, YAML, TOML, Markdown, blobs',
      'Tree-sitter structural import',
      'Stable IDs for non-rich languages',
      'Conservative recovery paths',
      'No language-specific merge logic',
    ],
  },
]

export function BonhommePresentation() {
  const [index, setIndex] = useState(0)
  const [direction, setDirection] = useState(1)
  const step = steps[index]
  const progress = ((index + 1) / steps.length) * 100

  const goTo = useCallback(
    (nextIndex: number) => {
      const bounded = Math.max(0, Math.min(steps.length - 1, nextIndex))
      setDirection(bounded >= index ? 1 : -1)
      setIndex(bounded)
    },
    [index],
  )

  const nextStep = useCallback(() => {
    setDirection(1)
    setIndex((current) => Math.min(current + 1, steps.length - 1))
  }, [])

  const previousStep = useCallback(() => {
    setDirection(-1)
    setIndex((current) => Math.max(current - 1, 0))
  }, [])

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === 'ArrowRight') nextStep()
      if (event.key === 'ArrowLeft') previousStep()
      if (event.key === 'Home') goTo(0)
      if (event.key === 'End') goTo(steps.length - 1)
    }

    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [goTo, nextStep, previousStep])

  useEffect(() => {
    window.scrollTo({ top: 0, left: 0 })
  }, [index])

  return (
    <section className="walkthrough-shell" aria-label="Bonhomme technical walkthrough">
      <header className="walkthrough-top">
        <div>
          <div className="walkthrough-kicker">
            <ScanSearch aria-hidden="true" />
            <span>Technical walkthrough</span>
          </div>
          <h1>How Bonhomme turns edited code into mergeable semantic operations</h1>
          <p>
            Trace one TypeScript service through source, symbols, operations, slice recovery,
            conflicts, and the core/plugin boundary.
          </p>
        </div>
        <div className="walkthrough-facts" aria-label="Trace facts">
          <Fact label="projection" value="OrderService.ts" />
          <Fact label="graph nodes" value="5" />
          <Fact label="primitive ops" value="7" />
        </div>
      </header>

      <div className="walkthrough-grid">
        <aside className="walkthrough-rail" aria-label="Walkthrough steps">
          {steps.map((item, itemIndex) => (
            <button
              key={item.id}
              type="button"
              className={itemIndex === index ? 'active' : ''}
              aria-current={itemIndex === index}
              onClick={() => goTo(itemIndex)}
            >
              <span className="rail-icon">{item.icon}</span>
              <span>
                <strong>{item.eyebrow}</strong>
                <small>{item.title}</small>
              </span>
            </button>
          ))}
        </aside>

        <main className="walkthrough-main">
          <AnimatePresence custom={direction} mode="wait">
            <motion.article
              key={step.id}
              custom={direction}
              className={`walkthrough-step walkthrough-step--${step.id}`}
              variants={{
                enter: (dir: number) => ({ opacity: 0, x: dir > 0 ? 28 : -28 }),
                center: { opacity: 1, x: 0 },
                exit: (dir: number) => ({ opacity: 0, x: dir > 0 ? -28 : 28 }),
              }}
              initial="enter"
              animate="center"
              exit="exit"
              transition={{ duration: 0.24, ease: [0.22, 1, 0.36, 1] }}
            >
              <StepHeader step={step} />
              <StepContent id={step.id} />
            </motion.article>
          </AnimatePresence>
        </main>
      </div>

      <footer className="walkthrough-controls">
        <div className="walkthrough-progress" aria-hidden="true">
          <span style={{ width: `${progress}%` }} />
        </div>
        <button
          type="button"
          aria-label="Previous walkthrough step"
          title="Previous"
          onClick={previousStep}
          disabled={index === 0}
        >
          <ArrowLeft aria-hidden="true" />
        </button>
        <div className="walkthrough-position">
          {index + 1} / {steps.length}
        </div>
        <button
          type="button"
          aria-label="Next walkthrough step"
          title="Next"
          onClick={nextStep}
          disabled={index === steps.length - 1}
        >
          <ArrowRight aria-hidden="true" />
        </button>
      </footer>
    </section>
  )
}

function StepHeader({ step }: { step: WalkthroughStep }) {
  return (
    <header className="step-header">
      <div className="step-eyebrow">
        {step.icon}
        <span>{step.eyebrow}</span>
      </div>
      <h2>{step.title}</h2>
      <p>{step.summary}</p>
    </header>
  )
}

function StepContent({ id }: { id: StepId }) {
  if (id === 'source') return <SourceStep />
  if (id === 'symbols') return <SymbolsStep />
  if (id === 'operations') return <OperationsStep />
  if (id === 'recovery') return <RecoveryStep />
  if (id === 'conflict') return <ConflictStep />
  return <BoundaryStep />
}

function SourceStep() {
  return (
    <div className="two-column source-step">
      <CodeWindow title="rendered/src/OrderService.ts" lines={sourceLines} />
      <div className="analysis-stack">
        <TraceCard
          icon={<FileCode2 aria-hidden="true" />}
          title="Rendered file"
          body="This is compatibility output. Editors, tests, and tsc can use it, but Bonhomme can regenerate it from operations."
        />
        <TraceCard
          icon={<Code2 aria-hidden="true" />}
          title="Plugin view"
          body="The TypeScript plugin recognizes classes, methods, properties, call expressions, and import surfaces."
        />
        <TraceCard
          icon={<Database aria-hidden="true" />}
          title="Core view"
          body="Core receives only generic symbols, references, containment, and metadata. It does not know what a TypeScript class is."
        />
      </div>
    </div>
  )
}

function SymbolsStep() {
  return (
    <div className="symbol-layout">
      <div className="symbol-graph" aria-label="Symbol graph">
        <SymbolNode kind="file" name="src/OrderService.ts" id="file:order-service" />
        <div className="graph-edge">contains</div>
        <SymbolNode kind="class" name="OrderService" id="sym:order-service" featured />
        <div className="symbol-children">
          <SymbolNode kind="property" name="orders" id="sym:orders" />
          <SymbolNode kind="method" name="displayName" id="sym:display-name" />
          <SymbolNode kind="method" name="listOrders" id="sym:list-orders" />
        </div>
        <div className="reference-edge">listOrders reads orders</div>
      </div>
      <div className="symbol-table">
        <div className="panel-title">
          <Waypoints aria-hidden="true" />
          Materialized graph rows
        </div>
        {symbols.map((symbol) => (
          <div key={symbol.id} className="symbol-row">
            <span>{symbol.kind}</span>
            <strong>{symbol.name}</strong>
            <code>{symbol.id}</code>
            <small>{symbol.pluginKind}</small>
          </div>
        ))}
      </div>
    </div>
  )
}

function OperationsStep() {
  return (
    <div className="operations-layout">
      <OperationTable rows={operations} />
      <div className="analysis-stack">
        <PrimitiveCard
          title="Operation shape"
          code={`CreateSymbol {
  symbolId: "sym:audit-risk",
  parentId: "sym:order-service",
  kind: "method",
  name: "auditRisk"
}`}
        />
        <PrimitiveCard
          title="Reference shape"
          code={`CreateReference {
  from: "sym:audit-risk",
  to: "sym:list-orders",
  kind: "calls"
}`}
        />
      </div>
    </div>
  )
}

function RecoveryStep() {
  return (
    <div className="recovery-layout">
      <div className="diff-grid">
        <CodeWindow title="base slice" lines={sourceLines} compact />
        <CodeWindow title="edited slice" lines={editedLines} compact />
      </div>
      <div className="recovery-pipeline">
        {['slice provenance', 'plugin parse', 'symbol matcher', 'primitive ops'].map(
          (item, itemIndex) => (
            <motion.div
              key={item}
              className="pipeline-step"
              initial={{ opacity: 0, y: 8 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ delay: itemIndex * 0.06 }}
            >
              <span>{itemIndex + 1}</span>
              {item}
            </motion.div>
          ),
        )}
      </div>
      <div className="recovery-bottom">
        <div className="recovered-ops">
          <div className="panel-title">
            <Diff aria-hidden="true" />
            Recovered primitive operations
          </div>
          {recoveredOperations.map((item) => (
            <div key={item.op + item.payload} className="recovered-row">
              <strong>{item.op}</strong>
              <span>{item.why}</span>
              <code>{item.payload}</code>
            </div>
          ))}
        </div>
        <TerminalWindow title="slice apply trace" lines={recoveryLogs} />
      </div>
    </div>
  )
}

function ConflictStep() {
  return (
    <div className="conflict-layout">
      <div className="diff-grid">
        <CodeWindow title="agent-017 slice" lines={conflictBaseLines} compact />
        <CodeWindow title="agent-022 slice" lines={conflictOtherLines} compact />
      </div>
      <div className="conflict-board">
        <div className="panel-title panel-title--danger">
          <AlertTriangle aria-hidden="true" />
          Duplicate sibling check
        </div>
        {conflictRows.map((row) => (
          <div key={row.branch} className="conflict-row">
            <strong>{row.branch}</strong>
            <code>{row.operation}</code>
            <span>{row.parent}</span>
            <em>{row.outcome}</em>
          </div>
        ))}
      </div>
      <div className="conflict-result">
        <GitMerge aria-hidden="true" />
        <div>
          <strong>CONFLICT</strong>
          <p>
            Core refuses to guess because both operations create the same method name under the same
            parent symbol. That is true even if the text bodies differ cleanly.
          </p>
        </div>
      </div>
    </div>
  )
}

function BoundaryStep() {
  return (
    <div className="boundary-layout">
      {boundaryColumns.map((column) => (
        <section key={column.title} className="boundary-column">
          <div className="boundary-title">
            {column.icon}
            <h3>{column.title}</h3>
          </div>
          <ul>
            {column.items.map((item) => (
              <li key={item}>{item}</li>
            ))}
          </ul>
        </section>
      ))}
      <div className="boundary-callout">
        <Workflow aria-hidden="true" />
        <div>
          <strong>The aha</strong>
          <p>
            Language plugins are translators. The core engine remains deterministic and auditable
            because it merges the same primitive model no matter which language produced it.
          </p>
        </div>
      </div>
    </div>
  )
}

function Fact({ label, value }: { label: string; value: string }) {
  return (
    <div className="fact">
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  )
}

function TraceCard({ icon, title, body }: { icon: ReactNode; title: string; body: string }) {
  return (
    <div className="trace-card">
      {icon}
      <div>
        <h3>{title}</h3>
        <p>{body}</p>
      </div>
    </div>
  )
}

function SymbolNode({
  kind,
  name,
  id,
  featured = false,
}: {
  kind: string
  name: string
  id: string
  featured?: boolean
}) {
  return (
    <motion.div
      className={`symbol-node ${featured ? 'symbol-node--featured' : ''}`}
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.22 }}
    >
      <span>{kind}</span>
      <strong>{name}</strong>
      <code>{id}</code>
    </motion.div>
  )
}

function OperationTable({ rows }: { rows: OperationRow[] }) {
  return (
    <div className="operation-table">
      <div className="panel-title">
        <RowsIcon />
        Append-only operation log
      </div>
      <div className="operation-head">
        <span>pos</span>
        <span>branch</span>
        <span>operation</span>
        <span>target</span>
        <span>status</span>
      </div>
      {rows.map((row) => (
        <div key={`${row.pos}-${row.target}`} className={`operation-row operation-row--${row.status}`}>
          <code>{row.pos}</code>
          <strong>{row.branch}</strong>
          <span>{row.op}</span>
          <code>{row.target}</code>
          <em>{row.status}</em>
          <small>{row.detail}</small>
        </div>
      ))}
    </div>
  )
}

function RowsIcon() {
  return <Database aria-hidden="true" />
}

function PrimitiveCard({ title, code }: { title: string; code: string }) {
  return (
    <div className="primitive-card">
      <div className="panel-title">
        <Code2 aria-hidden="true" />
        {title}
      </div>
      <pre>
        <code>{code}</code>
      </pre>
    </div>
  )
}

function CodeWindow({
  title,
  lines,
  compact = false,
}: {
  title: string
  lines: CodeLine[]
  compact?: boolean
}) {
  return (
    <div className={`walkthrough-code ${compact ? 'walkthrough-code--compact' : ''}`}>
      <div className="code-title">
        <span />
        <span />
        <span />
        <strong>{title}</strong>
      </div>
      <pre>
        {lines.map((line) => (
          <code key={`${title}-${line.number}`} className={line.tone ? `tone-${line.tone}` : ''}>
            <span>{line.number}</span>
            <b>{line.text || ' '}</b>
          </code>
        ))}
      </pre>
    </div>
  )
}

function TerminalWindow({ title, lines }: { title: string; lines: string[] }) {
  return (
    <div className="walkthrough-terminal">
      <div className="panel-title">
        <Terminal aria-hidden="true" />
        {title}
      </div>
      <div className="terminal-lines">
        {lines.map((line, lineIndex) => (
          <motion.code
            key={line}
            initial={{ opacity: 0, y: 7 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ delay: lineIndex * 0.05 }}
          >
            {line}
          </motion.code>
        ))}
      </div>
    </div>
  )
}
