const CODEX_ENV_KEYS = new Set([
  'PATH',
  'HOME',
  'USER',
  'LOGNAME',
  'SHELL',
  'TMPDIR',
  'TMP',
  'TEMP',
  'LANG',
  'LC_ALL',
  'TERM',
  'CODEX_HOME',
  'OPENAI_API_KEY',
  'OPENAI_BASE_URL',
  'HTTP_PROXY',
  'HTTPS_PROXY',
  'NO_PROXY',
  'SSL_CERT_FILE',
  'SSL_CERT_DIR',
])

export function codexEnvironment(source = process.env) {
  return Object.fromEntries(
    Object.entries(source).filter(([key]) => CODEX_ENV_KEYS.has(key) || key.startsWith('LC_')),
  )
}

const LOG_SECRET_KEYS = [
  'BONHOMME_API_TOKEN',
  'OPENAI_API_KEY',
  'OPENAI_BASE_URL',
  'HTTP_PROXY',
  'HTTPS_PROXY',
  'DATABASE_URL',
  'PGPASSWORD',
]

const BEARER_PREFIX_WINDOW = 'Bearer '.length

export function redactSubprocessOutput(output, source = process.env) {
  let redacted = String(output).replace(/Bearer\s+[^\s"']+/gi, 'Bearer [REDACTED]')
  return redactConfiguredValues(redacted, source)
}

function redactConfiguredValues(output, source) {
  let redacted = String(output)
  for (const key of LOG_SECRET_KEYS) {
    const value = source[key]
    if (typeof value === 'string' && value.length >= 4) {
      redacted = redacted.split(value).join(`[REDACTED:${key}]`)
    }
  }
  return redacted
}

export function createStreamingRedactor(source = process.env) {
  const longestSecret = LOG_SECRET_KEYS.reduce((longest, key) => {
    const value = source[key]
    return typeof value === 'string' && value.length >= 4
      ? Math.max(longest, value.length)
      : longest
  }, 0)
  const literalWindow = Math.max(0, longestSecret - 1)
  let bearerBuffer = ''
  let literalBuffer = ''
  let insideBearerToken = false

  const redactLiterals = (value, final) => {
    const redacted = redactConfiguredValues(`${literalBuffer}${value}`, source)
    if (final || literalWindow === 0) {
      literalBuffer = ''
      return redacted
    }
    const emitLength = Math.max(0, redacted.length - literalWindow)
    literalBuffer = redacted.slice(emitLength)
    return redacted.slice(0, emitLength)
  }

  const redactBearerTokens = (value, final) => {
    let input = `${bearerBuffer}${value}`
    bearerBuffer = ''
    let output = ''

    while (input.length > 0) {
      if (insideBearerToken) {
        const delimiter = input.search(/[\s"']/)
        if (delimiter === -1) {
          return output
        }
        output += input[delimiter]
        input = input.slice(delimiter + 1)
        insideBearerToken = false
        continue
      }

      const match = /Bearer\s+/i.exec(input)
      if (match) {
        output += input.slice(0, match.index)
        output += 'Bearer [REDACTED]'
        input = input.slice(match.index + match[0].length)
        insideBearerToken = true
        continue
      }

      if (final) {
        output += input
      } else {
        const emitLength = Math.max(0, input.length - BEARER_PREFIX_WINDOW)
        output += input.slice(0, emitLength)
        bearerBuffer = input.slice(emitLength)
      }
      return output
    }

    return output
  }

  return {
    push(chunk) {
      return redactLiterals(redactBearerTokens(String(chunk), false), false)
    },
    finish() {
      return redactLiterals(redactBearerTokens('', true), true)
    },
  }
}

export function boundedDiagnosticTail(previous, chunk, limit = 64 * 1024) {
  const combined = `${previous}${chunk}`
  return combined.length <= limit ? combined : combined.slice(combined.length - limit)
}
