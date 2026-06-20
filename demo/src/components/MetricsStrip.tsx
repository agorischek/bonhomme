import { CheckCircle2, Database, Network, Split, Users } from 'lucide-react'
import type { ReactNode } from 'react'
import type { DemoMetrics } from '../types'

function Metric({ icon, label, value }: { icon: ReactNode; label: string; value: number }) {
  return (
    <div className="metric">
      {icon}
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  )
}

export function MetricsStrip({ metrics }: { metrics?: DemoMetrics }) {
  return (
    <section className="metrics" aria-label="Repository metrics">
      <Metric icon={<Database />} label="Postgres ops" value={metrics?.operationCount ?? 0} />
      <Metric icon={<Users />} label="Agent branches" value={metrics?.agentCount ?? 0} />
      <Metric icon={<CheckCircle2 />} label="Merged agents" value={metrics?.mergedAgentCount ?? 0} />
      <Metric icon={<Network />} label="References" value={metrics?.referenceCount ?? 0} />
      <Metric icon={<Split />} label="Graph symbols" value={metrics?.symbolCount ?? 0} />
    </section>
  )
}
