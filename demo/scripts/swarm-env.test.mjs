import assert from 'node:assert/strict'
import test from 'node:test'

import {
  boundedDiagnosticTail,
  codexEnvironment,
  createStreamingRedactor,
  redactSubprocessOutput,
} from './swarm-env.mjs'

test('Codex subprocesses receive tool credentials but never Bonhomme service credentials', () => {
  const filtered = codexEnvironment({
    PATH: '/bin',
    CODEX_HOME: '/codex',
    OPENAI_API_KEY: 'agent-tool-key',
    LC_CTYPE: 'en_US.UTF-8',
    BONHOMME_API_TOKEN: 'service-token',
    BONHOMME_API_TENANTS: 'tenant-a',
    DATABASE_URL: 'postgres://service-database',
    PGPASSWORD: 'database-password',
  })

  assert.deepEqual(filtered, {
    PATH: '/bin',
    CODEX_HOME: '/codex',
    OPENAI_API_KEY: 'agent-tool-key',
    LC_CTYPE: 'en_US.UTF-8',
  })
})

test('subprocess logs redact bearer and configured credential values', () => {
  const output = redactSubprocessOutput(
    'Authorization: Bearer header-token; key=tool-secret; db=postgres://private',
    {
      OPENAI_API_KEY: 'tool-secret',
      DATABASE_URL: 'postgres://private',
    },
  )
  assert.equal(
    output,
    'Authorization: Bearer [REDACTED] key=[REDACTED:OPENAI_API_KEY]; db=[REDACTED:DATABASE_URL]',
  )
})

test('subprocess logs redact credential-bearing service URLs', () => {
  const proxy = 'http://worker:proxy-secret@example.invalid:8080'
  const base = 'https://service:base-secret@example.invalid/v1'
  const output = redactSubprocessOutput(`proxy=${proxy}; base=${base}`, {
    HTTPS_PROXY: proxy,
    OPENAI_BASE_URL: base,
  })
  assert.equal(
    output,
    'proxy=[REDACTED:HTTPS_PROXY]; base=[REDACTED:OPENAI_BASE_URL]',
  )
})

test('streaming redaction handles secrets and bearer tokens split across chunks', () => {
  const proxy = 'http://worker:proxy-secret@example.invalid:8080'
  const redactor = createStreamingRedactor({ HTTPS_PROXY: proxy })
  const chunks = [
    'proxy=http://worker:proxy-',
    'secret@example.invalid:8080 Authorization: Bear',
    'er header-',
    'token\nfinished',
  ]
  const output = chunks.map((chunk) => redactor.push(chunk)).join('') + redactor.finish()
  assert.equal(
    output,
    'proxy=[REDACTED:HTTPS_PROXY] Authorization: Bearer [REDACTED]\nfinished',
  )
})

test('diagnostic tails stay bounded', () => {
  assert.equal(boundedDiagnosticTail('1234', '5678', 6), '345678')
})
