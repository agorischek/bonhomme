import path from 'node:path'

const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i

export function apiConflictWorkspace(repoRoot, conflictDir) {
  const relative = path.relative(repoRoot, conflictDir)
  if (
    relative === '' ||
    relative === '..' ||
    relative.startsWith(`..${path.sep}`) ||
    path.isAbsolute(relative)
  ) {
    throw new Error(`API conflict workspace is outside the repository: ${conflictDir}`)
  }
  return relative.split(path.sep).join('/')
}

export function resolveApiConflictWorkspace(repoRoot, conflictId, checkoutDirectory) {
  if (!UUID_PATTERN.test(conflictId)) {
    throw new Error(`API returned an invalid conflict id: ${conflictId}`)
  }
  if (typeof checkoutDirectory !== 'string' || checkoutDirectory.length === 0) {
    throw new Error('API returned an invalid conflict workspace directory')
  }
  if (path.isAbsolute(checkoutDirectory)) {
    throw new Error(`API conflict workspace must be repository-relative: ${checkoutDirectory}`)
  }

  const expected = path.resolve(repoRoot, '.bonhomme', 'conflicts', conflictId)
  const resolved = path.resolve(repoRoot, checkoutDirectory)
  apiConflictWorkspace(repoRoot, resolved)
  if (resolved !== expected) {
    throw new Error(
      `API conflict workspace does not match conflict ${conflictId}: ${checkoutDirectory}`,
    )
  }
  return resolved
}
