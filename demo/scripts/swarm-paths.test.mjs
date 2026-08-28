import assert from 'node:assert/strict'
import path from 'node:path'
import test from 'node:test'

import { apiConflictWorkspace, resolveApiConflictWorkspace } from './swarm-paths.mjs'

test('API conflict paths are repository-relative and cannot escape', () => {
  const root = path.resolve('/work/bonhomme')
  const conflict = path.join(root, '.bonhomme', 'conflicts', 'conflict-1')
  assert.equal(apiConflictWorkspace(root, conflict), '.bonhomme/conflicts/conflict-1')
  assert.throws(() => apiConflictWorkspace(root, root), /outside the repository/)
  assert.throws(
    () => apiConflictWorkspace(root, path.resolve(root, '..', 'outside')),
    /outside the repository/,
  )
})

test('API checkout paths are bound to the returned conflict before local mutation', () => {
  const root = path.resolve('/work/bonhomme')
  const conflictId = '123e4567-e89b-42d3-a456-426614174000'
  const relative = `.bonhomme/conflicts/${conflictId}`
  assert.equal(
    resolveApiConflictWorkspace(root, conflictId, relative),
    path.join(root, '.bonhomme', 'conflicts', conflictId),
  )
  assert.throws(
    () => resolveApiConflictWorkspace(root, conflictId, path.resolve(root, relative)),
    /must be repository-relative/,
  )
  assert.throws(
    () => resolveApiConflictWorkspace(root, conflictId, '../outside'),
    /outside the repository/,
  )
  assert.throws(
    () => resolveApiConflictWorkspace(root, conflictId, '.bonhomme/conflicts/someone-else'),
    /does not match conflict/,
  )
  assert.throws(
    () => resolveApiConflictWorkspace(root, '../../escape', relative),
    /invalid conflict id/,
  )
})
