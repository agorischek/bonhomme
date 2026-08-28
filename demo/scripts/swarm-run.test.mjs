import assert from 'node:assert/strict'
import { promises as fs } from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import test from 'node:test'

import { run } from './swarm-run.mjs'

test('logged subprocess output streams to a private file with a bounded redacted error tail', async () => {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'bonhomme-swarm-run-'))
  const logPath = path.join(directory, 'worker.log')
  const proxy = 'http://worker:proxy-secret@example.invalid:8080'
  const script = [
    "process.stdout.write('x'.repeat(96 * 1024))",
    "process.stderr.write('\\nproxy=' + process.env.HTTPS_PROXY)",
    "process.stderr.write('\\nAuthorization: Bearer header-token')",
    'process.exitCode = 7',
  ].join(';')

  await assert.rejects(
    run(process.execPath, ['-e', script], {
      cwd: directory,
      env: { PATH: process.env.PATH, HTTPS_PROXY: proxy },
      replaceEnv: true,
      logPath,
    }),
    (error) => {
      assert.ok(error.message.length <= 64 * 1024)
      assert.doesNotMatch(error.message, /proxy-secret|header-token/)
      return true
    },
  )

  const log = await fs.readFile(logPath, 'utf8')
  assert.ok(log.length > 96 * 1024)
  assert.doesNotMatch(log, /proxy-secret|header-token/)
  assert.match(log, /\[REDACTED:HTTPS_PROXY\]/)
  assert.match(log, /Bearer \[REDACTED\]/)
  if (process.platform !== 'win32') {
    const mode = (await fs.stat(logPath)).mode & 0o777
    assert.equal(mode, 0o600)
  }

  await fs.rm(directory, { recursive: true, force: true })
})
