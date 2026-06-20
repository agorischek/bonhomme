import { TreeView } from '@primer/react'
import type { SymbolTreeNode } from './graph'
import { KindIcon } from './KindIcon'

function renderNodes(
  nodes: SymbolTreeNode[],
  selectedId: string | null,
  onSelect: (id: string) => void,
) {
  return nodes.map((node) => (
    <TreeView.Item
      key={node.id}
      id={node.id}
      current={node.id === selectedId}
      defaultExpanded={node.kind === 'file' || node.kind === 'class'}
      onSelect={() => onSelect(node.id)}
    >
      <TreeView.LeadingVisual>
        <KindIcon kind={node.kind} />
      </TreeView.LeadingVisual>
      {node.name}
      {node.children.length > 0 && (
        <TreeView.SubTree>{renderNodes(node.children, selectedId, onSelect)}</TreeView.SubTree>
      )}
    </TreeView.Item>
  ))
}

export function SymbolTree({
  roots,
  selectedId,
  onSelect,
}: {
  roots: SymbolTreeNode[]
  selectedId: string | null
  onSelect: (id: string) => void
}) {
  return (
    <TreeView aria-label="Symbol tree">{renderNodes(roots, selectedId, onSelect)}</TreeView>
  )
}
