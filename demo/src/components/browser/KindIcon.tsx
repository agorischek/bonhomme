import {
  CodeIcon,
  DotFillIcon,
  FileCodeIcon,
  FileIcon,
  PackageIcon,
  TagIcon,
} from '@primer/octicons-react'

const KIND_COLORS: Record<string, string> = {
  file: 'var(--fgColor-accent, #0969da)',
  class: '#8250df',
  method: '#1f883d',
  function: '#1f883d',
  property: '#bf8700',
  field: '#bf8700',
  blob: 'var(--fgColor-muted, #59636e)',
}

export function KindIcon({ kind, size = 16 }: { kind: string; size?: number }) {
  const color = KIND_COLORS[kind] ?? 'var(--fgColor-muted, #59636e)'
  const Icon =
    kind === 'file'
      ? FileCodeIcon
      : kind === 'class'
        ? PackageIcon
        : kind === 'method' || kind === 'function'
          ? CodeIcon
          : kind === 'property' || kind === 'field'
            ? TagIcon
            : kind === 'blob'
              ? FileIcon
              : DotFillIcon
  return (
    <span style={{ color, display: 'inline-flex' }}>
      <Icon size={size} />
    </span>
  )
}
