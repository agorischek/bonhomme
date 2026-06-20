import type { ReactNode } from 'react'

export function SectionHeader({
  icon,
  title,
  detail,
}: {
  icon: ReactNode
  title: string
  detail: string
}) {
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
